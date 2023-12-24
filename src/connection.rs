use core::convert::TryInto;
use std::{collections::HashMap, println};

use crate::*;

#[derive(Copy, Clone)]
pub struct ObjectResult<'a> {
    pub path: &'a str,
    pub id: u32,
    pub ty: u32,
}
impl core::fmt::Debug for ObjectResult<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{} @0x{:08x} type={:08x}", self.path, self.id, self.ty)
    }
}

pub struct SignatureResult<'a> {
    pub object: ObjectResult<'a>,
    pub name: &'a str,
    //pub args: &'a mut dyn Iterator<Item = (&'a str, BlobMsgType)>,
    pub args: HashMap<&'a str, BlobMsgType>,
}

pub struct Connection<T: IO> {
    io: T,
    peer: u32,
    sequence: u16,
    buffer: [u8; 64 * 1024],
}

impl<T: IO> Connection<T> {
    /// Create a new ubus connection from an existing IO
    pub fn new(io: T) -> Result<Self, Error<T::Error>> {
        let mut new = Self {
            io,
            peer: 0,
            sequence: 0,
            buffer: [0u8; 64 * 1024],
        };

        // ubus server should say hello on connect
        let message = new.next_message()?;

        // Verify the header is what we expect
        valid_data!(
            message.header.message == UbusMsgType::HELLO,
            "Expected hello"
        );

        // Record our peer id
        new.peer = message.header.peer.into();

        Ok(new)
    }

    // Get next message from ubus channel (blocking!)
    pub fn next_message(&mut self) -> Result<UbusMsg, Error<T::Error>> {
        UbusMsg::from_io(&mut self.io, &mut self.buffer)
    }

    pub fn send(&mut self, message: UbusMsgBuilder) -> Result<(), Error<T::Error>> {
        self.io.put(message.into())
    }

    pub fn invoke(
        &mut self,
        obj: u32,
        method: &str,
        args: Option<&BlobMsgData>,
        mut on_result: impl FnMut(BlobIter<BlobMsg>),
    ) -> Result<(), Error<T::Error>> {
        self.sequence += 1;
        let sequence = self.sequence.into();

        let mut buffer = [0u8; 1024];
        let mut message = UbusMsgBuilder::new(
            &mut buffer,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                message: UbusMsgType::INVOKE,
                sequence,
                peer: obj.into(),
            },
        )
        .unwrap();

        message.put(UbusMsgAttr::ObjId(obj))?;
        message.put(UbusMsgAttr::Method(method))?;
        if let Some(args) = args {
            let data = BlobMsg{
                 name: Some("data"),
                data: todo!(),
                 //data: *args,
            };
            //message.put(UbusMsgAttr::Data(data.into()))?;
        } else {
            message.put(UbusMsgAttr::Data(&[]))?;
        }
        self.send(message)?;
        'message: loop {
            let message = self.next_message()?;
            if message.header.sequence != sequence {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            match message.header.message {
                UbusMsgType::STATUS => {
                    for attr in attrs {
                        if let UbusMsgAttr::Status(0) = attr {
                            return Ok(());
                        } else if let UbusMsgAttr::Status(status) = attr {
                            return Err(Error::Status(status));
                        }
                    }
                    return Err(Error::InvalidData("Invalid status message"));
                }
                UbusMsgType::DATA => {
                    for attr in attrs {
                        if let UbusMsgAttr::Data(data) = attr {
                            on_result(BlobIter::<BlobMsg>::new(data));
                            continue 'message;
                        }
                    }
                    return Err(Error::InvalidData("Invalid data message"));
                }
                unknown => {
                    std::dbg!(unknown);
                }
            }
        }
    }

    pub fn lookup(
        &mut self,
        obj_path: &str,
        mut on_object: impl FnMut(ObjectResult),
        mut on_signature: impl FnMut(SignatureResult),
    ) -> Result<(), Error<T::Error>> {
        self.sequence += 1;
        let sequence = self.sequence.into();

        let mut buffer = [0u8; 1024];
        let mut message = UbusMsgBuilder::new(
            &mut buffer,
            UbusMsgHeader {
                version: UbusMsgVersion::CURRENT,
                message: UbusMsgType::LOOKUP,
                sequence,
                peer: 0.into(),
            },
        )
        .unwrap();
        if obj_path.len() != 0 {
            message.put(UbusMsgAttr::ObjPath(obj_path)).unwrap();
        }
        self.send(message)?;

        loop {
            let message = self.next_message()?;
            if message.header.sequence != sequence {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            if message.header.message == UbusMsgType::STATUS {
                for attr in attrs {
                    if let UbusMsgAttr::Status(0) = attr {
                        return Ok(());
                    } else if let UbusMsgAttr::Status(status) = attr {
                        return Err(Error::Status(status));
                    }
                }
                return Err(Error::InvalidData("Invalid status message"));
            }

            if message.header.message != UbusMsgType::DATA {
                continue;
            }

            let mut obj_path: Option<&str> = None;
            let mut obj_id: Option<u32> = None;
            let mut obj_type: Option<u32> = None;
            for attr in attrs {
                match attr {
                    UbusMsgAttr::ObjPath(path) => obj_path = Some(path),
                    UbusMsgAttr::ObjId(id) => obj_id = Some(id),
                    UbusMsgAttr::ObjType(ty) => obj_type = Some(ty),
                    UbusMsgAttr::Signature(nested) => {
                        let object = ObjectResult {
                            path: obj_path.unwrap(),
                            id: obj_id.unwrap(),
                            ty: obj_type.unwrap(),
                        };
                        on_object(object);

                        for signature in nested {
                            if let BlobMsgData::Table(table) = signature.data {
                                on_signature(SignatureResult {
                                    object,
                                    name: signature.name.unwrap(),
                                    args: table
                                        .iter()
                                        .map(|(k, v)| {
                                            if let BlobMsgData::Int32(typeid) = *v {
                                                (*k, BlobMsgType::from(typeid as u32))
                                            } else {
                                                panic!()
                                            }
                                        })
                                        .collect(),
                                })
                            }
                        }
                    }
                    _ => continue,
                }
            }
        }
    }

    pub fn lookup_id(&mut self, obj_path: &str) -> Result<u32, Error<T::Error>> {
        let mut obj_id = 0u32;
        self.lookup(obj_path, |obj| obj_id = obj.id, |_| {})?;
        Ok(obj_id)
    }
}
