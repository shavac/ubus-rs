use crate::*;
use core::convert::{TryFrom, TryInto};
use core::panic;
use std::collections::HashMap;
use std::vec::Vec;

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
    pub args: HashMap<&'a str, BlobMsgType>,
}
#[derive(Debug)]
pub struct Signature<'a> {
    pub name: &'a str,
    pub args: HashMap<&'a str, BlobMsgType>,
}

#[derive(Default, Debug)]
pub struct ObjectSignatures<'a> {
    pub path: &'a str,
    pub id: u32,
    pub ty: u32,
    pub signatures: HashMap<&'a str, Signature<'a>>,
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
        let mut conn = Self {
            io,
            peer: 0,
            sequence: 0,
            buffer: [0u8; 64 * 1024],
        };

        // ubus server should say hello on connect
        let message = conn.next_message()?;

        // Verify the header is what we expect
        valid_data!(
            message.header.cmd_type == UbusMsgType::HELLO,
            "Expected hello"
        );

        // Record our peer id
        conn.peer = message.header.peer.into();

        Ok(conn)
    }

    fn header_by_obj_cmd(&mut self, obj_id: u32, cmd: UbusMsgType) -> UbusMsgHeader {
        self.sequence += 1;
        UbusMsgHeader {
            version: UbusMsgVersion::CURRENT,
            cmd_type: cmd,
            sequence: self.sequence.into(),
            peer: obj_id.into(),
        }
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
        args: Vec<BlobMsg>,
        mut on_result: impl FnMut(BlobIter<Blob>),
    ) -> Result<(), Error<T::Error>> {
        let mut buffer = [0u8; 1024];
        let header = self.header_by_obj_cmd(obj, UbusMsgType::INVOKE);
        let mut message = UbusMsgBuilder::new(&mut buffer, &header).unwrap();
        message.put(UbusMsgAttr::ObjId(obj))?;
        message.put(UbusMsgAttr::Method(method))?;
        let mut data = Vec::new();
        for blobmsg in args {
            let blob = BlobMsgBuilder::try_from(blobmsg)?;
            data.extend_from_slice(blob.data());
        }
        message.put(UbusMsgAttr::Data(&data))?;

        self.send(message)?;
        'message: loop {
            let message = self.next_message()?;
            if message.header.sequence != header.sequence {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            match message.header.cmd_type {
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
                            on_result(BlobIter::new(data));
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

    pub fn lookup_cb(
        &mut self,
        obj_path: &str,
        mut on_object: impl FnMut(ObjectResult),
        mut on_signature: impl FnMut(SignatureResult),
    ) -> Result<(), Error<T::Error>> {
        let mut buffer = [0u8; 1024];
        let header = self.header_by_obj_cmd(0, UbusMsgType::LOOKUP);
        let mut request = UbusMsgBuilder::new(&mut buffer, &header).unwrap();
        if obj_path.len() != 0 {
            request.put(UbusMsgAttr::ObjPath(obj_path)).unwrap();
        }
        self.send(request)?;

        loop {
            let message = self.next_message()?;
            if message.header.sequence != header.sequence {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            if message.header.cmd_type == UbusMsgType::STATUS {
                for attr in attrs {
                    if let UbusMsgAttr::Status(0) = attr {
                        return Ok(());
                    } else if let UbusMsgAttr::Status(status) = attr {
                        return Err(Error::Status(status));
                    }
                }
                return Err(Error::InvalidData("Invalid status message"));
            }

            if message.header.cmd_type != UbusMsgType::DATA {
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
                            if let BlobMsgPayload::Table(table) = signature.1 {
                                on_signature(SignatureResult {
                                    object,
                                    name: signature.0,
                                    args: table
                                        .iter()
                                        .map(|(k, v)| {
                                            if let BlobMsgPayload::Int32(typeid) = *v {
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
        self.lookup(obj_path, |obj| obj_id = obj.id)?;
        Ok(obj_id)
    }

    pub fn lookup(
        &mut self,
        obj_path: &str,
        mut on_object: impl FnMut(ObjectSignatures),
    ) -> Result<(), Error<T::Error>> {
        let mut buffer = [0u8; 1024];
        let header = self.header_by_obj_cmd(0, UbusMsgType::LOOKUP);
        let mut request = UbusMsgBuilder::new(&mut buffer, &header).unwrap();
        if obj_path.len() != 0 {
            request.put(UbusMsgAttr::ObjPath(obj_path)).unwrap();
        }
        self.send(request)?;

        loop {
            let message = self.next_message()?;
            if message.header.sequence != header.sequence {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            if message.header.cmd_type == UbusMsgType::STATUS {
                for attr in attrs {
                    if let UbusMsgAttr::Status(0) = attr {
                        return Ok(());
                    } else if let UbusMsgAttr::Status(status) = attr {
                        return Err(Error::Status(status));
                    }
                }
                return Err(Error::InvalidData("Invalid status message"));
            }

            if message.header.cmd_type != UbusMsgType::DATA {
                continue;
            }
            let mut obj = ObjectSignatures::default();
            for attr in attrs {
                match attr {
                    UbusMsgAttr::ObjPath(path) => obj.path = path,
                    UbusMsgAttr::ObjId(id) => obj.id = id,
                    UbusMsgAttr::ObjType(ty) => obj.ty = ty,
                    UbusMsgAttr::Signature(nested) => {
                        for item in nested {
                            let signature = Signature {
                                name: item.0,
                                args: if let BlobMsgPayload::Table(table) = item.1 {
                                    table
                                        .iter()
                                        .map(|(k, v)| {
                                            if let BlobMsgPayload::Int32(typeid) = *v {
                                                (*k, BlobMsgType::from(typeid as u32))
                                            } else {
                                                panic!()
                                            }
                                        })
                                        .collect()
                                } else {
                                    panic!()
                                },
                            };
                            obj.signatures.insert(item.0, signature);
                        }
                    }
                    _ => continue,
                }
            }
            on_object(obj);
        }
    }
}
