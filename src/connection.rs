use crate::*;

#[cfg(feature = "json")]
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::convert::TryInto;
use std::collections::HashMap;
use storage_endian::BEu16;

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

#[derive(Clone)]
pub struct Connection<T: IO> {
    io: T,
    peer: u32,
    sequence: u16,
    buffer: Vec<u8>,
}

impl<T: IO> Connection<T> {
    /// Create a new ubus connection from an existing IO
    pub fn new(io: T) -> Result<Self, UbusError> {
        let mut conn = Self {
            io,
            peer: 0,
            sequence: 0,
            buffer: Vec::new(),
        };

        // ubus server should say hello on connect
        let message = conn.next_message()?;

        // Verify the header is what we expect
        valid_data!(
            message.header.cmd_type == UbusCmdType::HELLO,
            "Expected hello"
        );

        // Record our peer id
        conn.peer = message.header.peer.into();

        Ok(conn)
    }

    fn header_by_obj_cmd(&mut self, obj_id: u32, cmd: UbusCmdType) -> UbusMsgHeader {
        self.sequence = self.sequence.wrapping_add(1);
        UbusMsgHeader {
            version: UbusMsgVersion::CURRENT,
            cmd_type: cmd,
            sequence: self.sequence.into(),
            peer: obj_id.into(),
        }
    }

    /// Build and send a LOOKUP request reusing `self.buffer` without an extra 1KB stack allocation.
    fn send_lookup_request(&mut self, obj_path: &str) -> Result<BEu16, UbusError> {
        let header = self.header_by_obj_cmd(0, UbusCmdType::LOOKUP);
        let req_cap = UbusMsgHeader::SIZE
            + BlobTag::SIZE
            + if obj_path.is_empty() {
                0
            } else {
                BlobTag::SIZE + obj_path.len() + 4
            };
        if self.buffer.len() < req_cap {
            self.buffer.resize(req_cap, 0);
        }
        let mut request = UbusMsgBuilder::new(&mut self.buffer, &header)?;
        if !obj_path.is_empty() {
            request.put(UbusMsgAttr::ObjPath(obj_path))?;
        }
        let len = request.finish().len();
        self.io.put(&self.buffer[..len])?;
        Ok(header.sequence)
    }

    // Get next message from ubus channel (blocking!)
    pub fn next_message(&mut self) -> Result<UbusMsg<'_>, UbusError> {
        UbusMsg::from_io_vec(&mut self.io, &mut self.buffer)
    }

    pub fn send(&mut self, message: UbusMsgBuilder) -> Result<(), UbusError> {
        self.io.put(message.into())
    }

    pub fn invoke(
        &mut self,
        obj: u32,
        method: &str,
        args: &[u8],
        mut on_result: impl FnMut(BlobIter<Blob>),
    ) -> Result<(), UbusError> {
        let header = self.header_by_obj_cmd(obj, UbusCmdType::INVOKE);
        let req_cap = UbusMsgHeader::SIZE
            + BlobTag::SIZE
            + (BlobTag::SIZE + 4)
            + (BlobTag::SIZE + method.len() + 4)
            + (BlobTag::SIZE + args.len() + 4);
        if self.buffer.len() < req_cap {
            self.buffer.resize(req_cap, 0);
        }
        let mut message = UbusMsgBuilder::new(&mut self.buffer, &header)?;
        message.put(UbusMsgAttr::ObjId(obj))?;
        message.put(UbusMsgAttr::Method(method))?;
        message.put(UbusMsgAttr::Data(args))?;
        let len = message.finish().len();
        self.io.put(&self.buffer[..len])?;

        'message: loop {
            let message = self.next_message()?;
            if message.header.sequence != header.sequence {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            match message.header.cmd_type {
                UbusCmdType::STATUS => {
                    for attr in attrs {
                        if let UbusMsgAttr::Status(0) = attr {
                            return Ok(());
                        } else if let UbusMsgAttr::Status(status) = attr {
                            return Err(UbusError::Status(status));
                        }
                    }
                    return Err(UbusError::InvalidData("Invalid status message"));
                }
                UbusCmdType::DATA => {
                    for attr in attrs {
                        if let UbusMsgAttr::Data(data) = attr {
                            on_result(BlobIter::new(data));
                            continue 'message;
                        }
                    }
                    return Err(UbusError::InvalidData("Invalid data message"));
                }
                _ => {}
            }
        }
    }

    /// Invoke `method` on `obj` with pre-encoded blob `args` and deserialize the response directly into `R`.
    pub fn invoke_typed<R: serde::de::DeserializeOwned>(
        &mut self,
        obj: u32,
        method: &str,
        args: &[u8],
    ) -> Result<R, UbusError> {
        let mut result: Option<Result<R, UbusError>> = None;
        self.invoke(obj, method, args, |bi| {
            result = Some(bi.deserialize::<R>());
        })?;
        match result {
            Some(res) => res,
            None => from_blob::<R>(&[]),
        }
    }

    #[cfg(feature = "json")]
    fn lookup_and_encode_args(
        &mut self,
        obj_path: &str,
        method: &str,
        args: &str,
    ) -> Result<(u32, Vec<u8>), UbusError> {
        let seq = self.send_lookup_request(obj_path)?;
        let mut target_id: Option<u32> = None;
        let mut encoded_args: Option<Result<Vec<u8>, UbusError>> = None;

        loop {
            let message = self.next_message()?;
            if message.header.sequence != seq {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            if message.header.cmd_type == UbusCmdType::STATUS {
                for attr in attrs {
                    if let UbusMsgAttr::Status(0) = attr {
                        break;
                    } else if let UbusMsgAttr::Status(status) = attr {
                        return Err(UbusError::Status(status));
                    }
                }
                if target_id.is_some() {
                    break;
                }
                return Err(UbusError::InvalidData("Invalid status message"));
            }

            if message.header.cmd_type != UbusCmdType::DATA {
                continue;
            }

            for attr in attrs {
                match attr {
                    UbusMsgAttr::ObjId(id) => target_id = Some(id),
                    UbusMsgAttr::Signature(nested) => {
                        if args.is_empty() {
                            encoded_args = Some(Ok(Vec::new()));
                            continue;
                        }
                        let mut method_payload = None;
                        for method_blob in nested {
                            let (m_name, m_payload) = method_blob.into_named_payload()?;
                            if m_name == method {
                                method_payload = Some(m_payload);
                                break;
                            }
                        }
                        if let Some(m_payload) = method_payload {
                            let policy_bytes: &[u8] = m_payload.into();
                            encoded_args = Some(encode_json_with_policy(args, |key| {
                                for arg_blob in BlobIter::<Blob>::new(policy_bytes) {
                                    if let Ok((arg_name, arg_payload)) =
                                        arg_blob.into_named_payload()
                                        && arg_name == key
                                        && let Ok(typeid) = TryInto::<i32>::try_into(arg_payload)
                                    {
                                        return Some(BlobMsgType::from(typeid as u32));
                                    }
                                }
                                None
                            }));
                        } else if args.trim_start().starts_with('{') {
                            encoded_args = Some(Err(UbusError::InvalidMethod(method.to_string())));
                        } else {
                            encoded_args = Some(encode_json_with_policy(args, |_| None));
                        }
                    }
                    _ => {}
                }
            }
        }

        let obj_id = target_id.ok_or(UbusError::InvalidData("Object not found"))?;
        let args_bytes = match encoded_args {
            Some(res) => res?,
            None => Vec::new(),
        };
        Ok((obj_id, args_bytes))
    }

    #[cfg(feature = "json")]
    fn call_json(
        &mut self,
        obj_path: &str,
        method: &str,
        args: &str,
        pretty: bool,
    ) -> Result<String, UbusError> {
        let (obj_id, args_bytes) = self.lookup_and_encode_args(obj_path, method, args)?;
        let mut json = String::new();
        let mut ser_err = None;
        self.invoke(obj_id, method, &args_bytes, |bi| {
            let res = if pretty {
                serde_json::to_string_pretty(&bi.as_table())
            } else {
                serde_json::to_string(&bi.as_table())
            };
            match res {
                Ok(s) => json = s,
                Err(e) => ser_err = Some(UbusError::ParseArguments(e)),
            }
        })?;
        if let Some(e) = ser_err {
            return Err(e);
        }
        Ok(json)
    }

    /// Look up an object, encode JSON arguments directly against the target method's wire policy,
    /// invoke the method, and serialize the response via serde as compact JSON without intermediate DOM trees.
    #[cfg(feature = "json")]
    pub fn call(
        &mut self,
        obj_path: &str,
        method: &str,
        args: &str,
    ) -> Result<String, UbusError> {
        self.call_json(obj_path, method, args, false)
    }

    /// Same as [`Connection::call`], but formats the returned JSON with pretty indentation.
    #[cfg(feature = "json")]
    pub fn call_pretty(
        &mut self,
        obj_path: &str,
        method: &str,
        args: &str,
    ) -> Result<String, UbusError> {
        self.call_json(obj_path, method, args, true)
    }

    /// Look up an object, invoke `method`, and deserialize the binary blob response directly
    /// into `R` via serde without intermediate JSON formatting or parsing.
    #[cfg(feature = "json")]
    pub fn call_typed<R: serde::de::DeserializeOwned>(
        &mut self,
        obj_path: &str,
        method: &str,
        args: &str,
    ) -> Result<R, UbusError> {
        let (obj_id, args_bytes) = self.lookup_and_encode_args(obj_path, method, args)?;
        self.invoke_typed(obj_id, method, &args_bytes)
    }

    #[cfg(feature = "json")]
    pub fn lookup_object_json(&mut self, obj_path: &str) -> Result<String, UbusError> {
        let mut obj_json = String::new();
        self.lookup(obj_path, |obj| {
            obj_json = serde_json::to_string_pretty(&obj).unwrap();
        })?;
        Ok(obj_json)
    }

    pub fn lookup_cb(
        &mut self,
        obj_path: &str,
        mut on_object: impl FnMut(ObjectResult),
        mut on_signature: impl FnMut(SignatureResult),
    ) -> Result<(), UbusError> {
        let seq = self.send_lookup_request(obj_path)?;

        loop {
            let message = self.next_message()?;
            if message.header.sequence != seq {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            if message.header.cmd_type == UbusCmdType::STATUS {
                for attr in attrs {
                    if let UbusMsgAttr::Status(0) = attr {
                        return Ok(());
                    } else if let UbusMsgAttr::Status(status) = attr {
                        return Err(UbusError::Status(status));
                    }
                }
                return Err(UbusError::InvalidData("Invalid status message"));
            }

            if message.header.cmd_type != UbusCmdType::DATA {
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
                            path: obj_path.ok_or(UbusError::InvalidData("Missing obj_path"))?,
                            id: obj_id.ok_or(UbusError::InvalidData("Missing obj_id"))?,
                            ty: obj_type.ok_or(UbusError::InvalidData("Missing obj_type"))?,
                        };
                        on_object(object);

                        for method_blob in nested {
                            let (method_name, method_payload) =
                                method_blob.into_named_payload()?;
                            let mut args = HashMap::new();
                            for arg_blob in BlobIter::<Blob>::new(method_payload.into()) {
                                let (arg_name, arg_payload) = arg_blob.into_named_payload()?;
                                let typeid: i32 = arg_payload.try_into()?;
                                args.insert(arg_name, BlobMsgType::from(typeid as u32));
                            }
                            on_signature(SignatureResult {
                                object,
                                name: method_name,
                                args,
                            });
                        }
                    }
                    _ => continue,
                }
            }
        }
    }

    /// Look up only the object ID for `obj_path` without parsing or allocating method signatures.
    pub fn lookup_id(&mut self, obj_path: &str) -> Result<u32, UbusError> {
        let seq = self.send_lookup_request(obj_path)?;
        let mut obj_id = 0u32;

        loop {
            let message = self.next_message()?;
            if message.header.sequence != seq {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            if message.header.cmd_type == UbusCmdType::STATUS {
                for attr in attrs {
                    if let UbusMsgAttr::Status(0) = attr {
                        return Ok(obj_id);
                    } else if let UbusMsgAttr::Status(status) = attr {
                        return Err(UbusError::Status(status));
                    }
                }
                return Err(UbusError::InvalidData("Invalid status message"));
            }

            if message.header.cmd_type != UbusCmdType::DATA {
                continue;
            }

            for attr in attrs {
                if let UbusMsgAttr::ObjId(id) = attr {
                    obj_id = id;
                }
            }
        }
    }

    pub fn lookup(
        &mut self,
        obj_path: &str,
        mut on_object: impl FnMut(UbusObject),
    ) -> Result<(), UbusError> {
        let seq = self.send_lookup_request(obj_path)?;

        loop {
            let message = self.next_message()?;
            if message.header.sequence != seq {
                continue;
            }

            let attrs = BlobIter::<UbusMsgAttr>::new(message.blob.data);

            if message.header.cmd_type == UbusCmdType::STATUS {
                for attr in attrs {
                    if let UbusMsgAttr::Status(0) = attr {
                        return Ok(());
                    } else if let UbusMsgAttr::Status(status) = attr {
                        return Err(UbusError::Status(status));
                    }
                }
                return Err(UbusError::InvalidData("Invalid status message"));
            }

            if message.header.cmd_type != UbusCmdType::DATA {
                continue;
            }
            let mut obj = UbusObject::default();
            for attr in attrs {
                match attr {
                    UbusMsgAttr::ObjPath(path) => obj.path = path,
                    UbusMsgAttr::ObjId(id) => obj.id = id,
                    UbusMsgAttr::ObjType(ty) => obj.ty = ty,
                    UbusMsgAttr::Signature(nested) => {
                        for method_blob in nested {
                            let (method_name, method_payload) =
                                method_blob.into_named_payload()?;
                            let mut policy = HashMap::new();
                            for arg_blob in BlobIter::<Blob>::new(method_payload.into()) {
                                let (arg_name, arg_payload) = arg_blob.into_named_payload()?;
                                let typeid: i32 = arg_payload.try_into()?;
                                policy.insert(arg_name, BlobMsgType::from(typeid as u32));
                            }
                            obj.methods.insert(
                                method_name,
                                Method {
                                    name: method_name,
                                    policy,
                                },
                            );
                        }
                    }
                    _ => continue,
                }
            }
            on_object(obj);
        }
    }
}
