use crate::*;

#[cfg(feature = "json")]
use alloc::string::{String, ToString};
use alloc::{collections::VecDeque, vec::Vec};
use core::convert::TryInto;
use core::time::Duration;
use std::collections::HashMap;
use std::time::Instant;

const MAX_PENDING_MSGS: usize = 64;
const PENDING_HIGH_WATER_MARK: usize = 8;

#[derive(Clone)]
struct PendingMsg {
    header: UbusMsgHeader,
    data: Vec<u8>,
}

enum DispatchAction {
    Ping(UbusMsgHeader),
    ReplyData(UbusMsgHeader, u32, Vec<u8>),
    ReplyStatus(UbusMsgHeader, u32, UbusStatus),
    Handled,
    Ignored,
}

#[derive(Default)]
struct InvokeFields<'a> {
    obj_id: Option<u32>,
    method: Option<&'a str>,
    data: &'a [u8],
    no_reply: bool,
    user: Option<&'a str>,
    group: Option<&'a str>,
}

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

/// Method schema descriptor for registering server-side objects without heap allocations.
#[derive(Copy, Clone, Debug)]
pub struct MethodSpec<'a> {
    pub name: &'a str,
    pub policy: &'a [(&'a str, BlobMsgType)],
}

/// Object descriptor passed to [`Connection::add_object`].
#[derive(Copy, Clone, Debug)]
pub struct ObjectSpec<'a> {
    pub path: Option<&'a str>,
    pub methods: &'a [MethodSpec<'a>],
}

/// Registered server-side object identifier returned by [`Connection::add_object`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RegisteredObject {
    pub id: u32,
    pub ty: u32,
}

/// Zero-copy view of an incoming `INVOKE` request dispatched to a server-side object.
#[derive(Copy, Clone, Debug)]
pub struct IncomingRequest<'a> {
    pub header: UbusMsgHeader,
    pub obj_id: u32,
    pub method: &'a str,
    pub data: &'a [u8],
    pub no_reply: bool,
    pub user: Option<&'a str>,
    pub group: Option<&'a str>,
}

/// Reply returned by a server request handler in [`Connection::dispatch_one`] / [`Connection::serve`].
#[derive(Clone, Debug)]
pub enum RequestReply {
    Status(UbusStatus),
    Data(Vec<u8>),
}

/// Zero-copy view of an incoming server or subscriber event dispatched by [`Connection::dispatch_event`].
#[derive(Copy, Clone, Debug)]
pub enum IncomingEvent<'a> {
    /// Incoming RPC invocation (`INVOKE`) or object notification delivered to a subscriber.
    Invoke(IncomingRequest<'a>),
    /// Subscriber activity change (`NOTIFY` with `ACTIVE`) for a published object.
    SubscribeActive { obj_id: u32, active: bool },
    /// Notification (`REMOVE_OBJECT`) to a subscriber that a subscribed remote object was removed.
    ObjectRemoved { subscriber_id: u32, target_id: u32 },
}

#[derive(Clone)]
pub struct Connection<T: IO> {
    io: T,
    peer: u32,
    sequence: u16,
    timeout_ms: u32,
    buffer: Vec<u8>,
    pending: VecDeque<PendingMsg>,
}

impl<T: IO> Connection<T> {
    /// Create a new ubus connection from an existing IO
    pub fn new(io: T) -> Result<Self, UbusError> {
        let mut conn = Self {
            io,
            peer: 0,
            sequence: 0,
            timeout_ms: 0,
            buffer: Vec::new(),
            pending: VecDeque::new(),
        };
        conn.read_hello()?;
        Ok(conn)
    }

    /// Borrow the underlying I/O transport.
    pub fn io(&self) -> &T {
        &self.io
    }

    /// Mutably borrow the underlying I/O transport.
    pub fn io_mut(&mut self) -> &mut T {
        &mut self.io
    }

    /// Consume the connection and return the underlying I/O transport.
    pub fn into_inner(self) -> T {
        self.io
    }

    /// Return the peer ID assigned by `ubusd` during the `HELLO` handshake.
    pub fn peer_id(&self) -> u32 {
        self.peer
    }

    /// Number of out-of-band messages queued while awaiting synchronous RPC replies.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Current heap capacity (in bytes) of the internal I/O frame buffer.
    pub fn buffer_capacity(&self) -> usize {
        self.buffer.capacity()
    }

    /// Release all unused capacity from the internal I/O and pending-message buffers.
    pub fn shrink_buffers(&mut self) {
        self.buffer.clear();
        self.buffer.shrink_to_fit();
        self.pending.shrink_to_fit();
    }

    fn reclaim_buffer(&mut self) {
        crate::ubusmsg::shrink_buffer_to_high_water(&mut self.buffer, 0);
    }

    fn reclaim_buffers(&mut self) {
        self.reclaim_buffer();
        if self.pending.is_empty() && self.pending.capacity() > PENDING_HIGH_WATER_MARK {
            self.pending.shrink_to_fit();
        }
    }

    /// Set the read and write I/O timeout for subsequent operations on this connection.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<(), UbusError> {
        let ms = timeout.map_or(0, |d| u32::try_from(d.as_millis()).unwrap_or(u32::MAX).max(1));
        self.io.set_read_timeout(timeout)?;
        self.io.set_write_timeout(timeout)?;
        self.timeout_ms = ms;
        Ok(())
    }

    /// Get the currently configured I/O timeout, if any.
    pub fn timeout(&self) -> Option<Duration> {
        (self.timeout_ms != 0).then(|| Duration::from_millis(u64::from(self.timeout_ms)))
    }

    fn deadline(&self) -> Option<Instant> {
        self.timeout().map(|d| Instant::now() + d)
    }

    fn finish_deadline(&mut self, deadline: Option<Instant>) {
        if deadline.is_some() {
            let _ = self.io.set_read_timeout(self.timeout());
        }
        self.reclaim_buffers();
    }

    #[inline]
    fn with_deadline<R>(
        &mut self,
        f: impl FnOnce(&mut Self, Option<Instant>) -> Result<R, UbusError>,
    ) -> Result<R, UbusError> {
        let deadline = self.deadline();
        let res = f(self, deadline);
        self.finish_deadline(deadline);
        res
    }

    fn read_hello(&mut self) -> Result<(), UbusError> {
        self.with_deadline(|this, deadline| {
            let (header, _tag) = this.read_raw_until(deadline)?;
            valid_data!(header.cmd_type == UbusCmdType::HELLO, "Expected hello");
            this.peer = header.peer.into();
            Ok(())
        })
    }

    fn read_raw_until(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<(UbusMsgHeader, BlobTag), UbusError> {
        if let Some(dl) = deadline {
            let now = Instant::now();
            if now >= dl {
                return Err(UbusError::Timeout);
            }
            let remaining = dl.duration_since(now).max(Duration::from_micros(1));
            self.io.set_read_timeout(Some(remaining))?;
        }
        UbusMsg::read_into_vec(&mut self.io, &mut self.buffer)
    }

    fn enqueue_raw_pending(&mut self, header: UbusMsgHeader, payload_len: usize) {
        if self.pending.len() >= MAX_PENDING_MSGS {
            self.pending.pop_front();
        }
        self.pending.push_back(PendingMsg {
            header,
            data: self.buffer[UbusMsg::PREFIX_LEN..UbusMsg::PREFIX_LEN + payload_len].to_vec(),
        });
    }

    fn next_for_sequence(
        &mut self,
        expected_seq: BEu16,
        deadline: Option<Instant>,
    ) -> Result<UbusMsg<'_>, UbusError> {
        let (header, tag) = loop {
            let (hdr, tag) = self.read_raw_until(deadline)?;
            if hdr.sequence == expected_seq {
                break (hdr, tag);
            }
            match hdr.cmd_type {
                UbusCmdType::PING => self.send_ping_reply(hdr)?,
                UbusCmdType::INVOKE | UbusCmdType::NOTIFY | UbusCmdType::REMOVE_OBJECT => {
                    self.enqueue_raw_pending(hdr, tag.inner_len());
                }
                _ => {}
            }
        };
        let total_len = UbusMsg::PREFIX_LEN + tag.inner_len();
        let blob = Blob::from_tag_and_data(tag, &self.buffer[UbusMsg::PREFIX_LEN..total_len])?;
        Ok(UbusMsg { header, blob })
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

    fn reply_header(req: UbusMsgHeader, cmd_type: UbusCmdType) -> UbusMsgHeader {
        UbusMsgHeader {
            version: UbusMsgVersion::CURRENT,
            cmd_type,
            sequence: req.sequence,
            peer: req.peer,
        }
    }

    fn send_frame(
        &mut self,
        header: UbusMsgHeader,
        attrs: &[UbusMsgAttr<'_>],
    ) -> Result<(), UbusError> {
        let req_cap = UbusMsg::PREFIX_LEN + attrs.iter().map(UbusMsgAttr::wire_len).sum::<usize>();
        if self.buffer.len() < req_cap {
            self.buffer.resize(req_cap, 0);
        }
        let res = (|| {
            let mut msg = UbusMsgBuilder::new(&mut self.buffer, &header)?;
            for &attr in attrs {
                msg.put(attr)?;
            }
            let len = msg.finish().len();
            self.io.put(&self.buffer[..len])
        })();
        self.reclaim_buffer();
        res
    }

    fn send_request(
        &mut self,
        obj_id: u32,
        cmd: UbusCmdType,
        attrs: &[UbusMsgAttr<'_>],
    ) -> Result<BEu16, UbusError> {
        let header = self.header_by_obj_cmd(obj_id, cmd);
        self.send_frame(header, attrs)?;
        Ok(header.sequence)
    }

    fn send_call_frame(
        &mut self,
        peer: u32,
        cmd: UbusCmdType,
        obj_id: u32,
        method: &str,
        data: &[u8],
        no_reply: bool,
    ) -> Result<BEu16, UbusError> {
        let attrs = [
            UbusMsgAttr::ObjId(obj_id),
            UbusMsgAttr::Method(method),
            UbusMsgAttr::Data(data),
            UbusMsgAttr::NoReply(true),
        ];
        self.send_request(peer, cmd, &attrs[..if no_reply { 4 } else { 3 }])
    }

    fn extract_status(blob_data: &[u8]) -> Result<i32, UbusError> {
        BlobIter::<UbusMsgAttr>::new(blob_data)
            .find_map(|attr| match attr {
                UbusMsgAttr::Status(s) => Some(s),
                _ => None,
            })
            .or_invalid("Invalid status message")
    }

    fn parse_status(blob_data: &[u8]) -> Result<(), UbusError> {
        match Self::extract_status(blob_data)? {
            0 => Ok(()),
            status => Err(UbusError::Status(status)),
        }
    }

    fn parse_method_policy(payload: Payload<'_>) -> Result<HashMap<&str, BlobMsgType>, UbusError> {
        let mut policy = HashMap::new();
        for arg_blob in BlobIter::<Blob>::new(payload.into()) {
            let (arg_name, arg_payload) = arg_blob.into_named_payload()?;
            let typeid: i32 = arg_payload.try_into()?;
            policy.insert(arg_name, BlobMsgType::from(typeid as u32));
        }
        Ok(policy)
    }

    fn parse_invoke_fields(blob_data: &[u8]) -> InvokeFields<'_> {
        BlobIter::<UbusMsgAttr>::new(blob_data).fold(InvokeFields::default(), |mut f, attr| {
            match attr {
                UbusMsgAttr::ObjId(id) => f.obj_id = Some(id),
                UbusMsgAttr::Method(m) => f.method = Some(m),
                UbusMsgAttr::Data(d) => f.data = d,
                UbusMsgAttr::NoReply(b) => f.no_reply = b,
                UbusMsgAttr::User(u) => f.user = Some(u),
                UbusMsgAttr::Group(g) => f.group = Some(g),
                _ => {}
            }
            f
        })
    }

    fn wait_status_until(
        &mut self,
        seq: BEu16,
        deadline: Option<Instant>,
    ) -> Result<(), UbusError> {
        loop {
            let message = self.next_for_sequence(seq, deadline)?;
            if message.header.cmd_type == UbusCmdType::STATUS {
                return Self::parse_status(message.blob.data);
            }
        }
    }

    fn wait_status(&mut self, seq: BEu16) -> Result<(), UbusError> {
        self.with_deadline(|this, deadline| this.wait_status_until(seq, deadline))
    }

    fn send_lookup_request(&mut self, obj_path: &str) -> Result<BEu16, UbusError> {
        let attr = (!obj_path.is_empty()).then_some(UbusMsgAttr::ObjPath(obj_path));
        self.send_request(0, UbusCmdType::LOOKUP, attr.as_slice())
    }

    // Get next message from ubus channel (blocking!)
    pub fn next_message(&mut self) -> Result<UbusMsg<'_>, UbusError> {
        UbusMsg::from_io_vec(&mut self.io, &mut self.buffer)
    }

    pub fn send(&mut self, message: UbusMsgBuilder) -> Result<(), UbusError> {
        self.io.put(message.into())
    }

    /// Send a `PING` message to `ubusd` and wait for `STATUS(OK)`.
    pub fn ping(&mut self) -> Result<(), UbusError> {
        let seq = self.send_request(0, UbusCmdType::PING, &[])?;
        self.wait_status(seq)
    }

    fn invoke_dyn(
        &mut self,
        obj: u32,
        method: &str,
        args: &[u8],
        on_result: &mut dyn FnMut(BlobIter<Blob<'_>>),
    ) -> Result<(), UbusError> {
        let seq = self.send_call_frame(obj, UbusCmdType::INVOKE, obj, method, args, false)?;
        self.with_deadline(|this, deadline| loop {
            let message = this.next_for_sequence(seq, deadline)?;
            match message.header.cmd_type {
                UbusCmdType::STATUS => return Self::parse_status(message.blob.data),
                UbusCmdType::DATA => {
                    let data = BlobIter::<UbusMsgAttr>::new(message.blob.data)
                        .find_map(|attr| match attr {
                            UbusMsgAttr::Data(data) => Some(data),
                            _ => None,
                        })
                        .or_invalid("Invalid data message")?;
                    on_result(BlobIter::new(data));
                }
                _ => {}
            }
        })
    }

    pub fn invoke(
        &mut self,
        obj: u32,
        method: &str,
        args: &[u8],
        mut on_result: impl FnMut(BlobIter<Blob>),
    ) -> Result<(), UbusError> {
        self.invoke_dyn(obj, method, args, &mut on_result)
    }

    /// Invoke `method` on `obj` with a temporary timeout, restoring the previous timeout afterwards.
    pub fn invoke_with_timeout(
        &mut self,
        obj: u32,
        method: &str,
        args: &[u8],
        timeout: Option<Duration>,
        on_result: impl FnMut(BlobIter<Blob>),
    ) -> Result<(), UbusError> {
        let prev = self.timeout();
        self.set_timeout(timeout)?;
        let res = self.invoke(obj, method, args, on_result);
        let _ = self.set_timeout(prev);
        res
    }

    /// Invoke `method` on `obj` with `NO_REPLY` enabled and return immediately without waiting for status.
    pub fn invoke_no_reply(
        &mut self,
        obj: u32,
        method: &str,
        args: &[u8],
    ) -> Result<(), UbusError> {
        self.send_call_frame(obj, UbusCmdType::INVOKE, obj, method, args, true)?;
        Ok(())
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
        result.unwrap_or_else(|| from_blob::<R>(&[]))
    }

    /// Serialize `args` directly into blob bytes via serde, invoke `method`, and deserialize the response into `R`.
    pub fn invoke_serde<A: serde::Serialize, R: serde::de::DeserializeOwned>(
        &mut self,
        obj: u32,
        method: &str,
        args: &A,
    ) -> Result<R, UbusError> {
        let encoded = to_blob(args)?;
        self.invoke_typed(obj, method, &encoded)
    }

    #[cfg(feature = "json")]
    fn encode_signature_args(
        nested: BlobIter<'_, Blob<'_>>,
        method: &str,
        args: &str,
    ) -> Result<Vec<u8>, UbusError> {
        if args.is_empty() {
            return Ok(Vec::new());
        }
        let mut method_payload = None;
        for method_blob in nested {
            let (m_name, m_payload) = method_blob.into_named_payload()?;
            if m_name == method {
                method_payload = Some(m_payload);
                break;
            }
        }
        match method_payload {
            Some(m_payload) => {
                let policy_bytes: &[u8] = m_payload.into();
                encode_json_with_policy(args, &mut |key| {
                    if policy_bytes.is_empty() {
                        return FieldPolicy::Dynamic;
                    }
                    BlobIter::<Blob>::new(policy_bytes)
                        .find_map(|arg_blob| {
                            let (arg_name, arg_payload) = arg_blob.into_named_payload().ok()?;
                            (arg_name == key)
                                .then(|| TryInto::<i32>::try_into(arg_payload).ok())
                                .flatten()
                                .map(|id| FieldPolicy::Typed(BlobMsgType::from(id as u32)))
                        })
                        .unwrap_or(FieldPolicy::Skip)
                })
            }
            None if args.trim_start().starts_with('{') => {
                Err(UbusError::InvalidMethod(method.to_string()))
            }
            None => encode_json_with_policy(args, &mut |_| FieldPolicy::Skip),
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

        let obj_id = self.with_deadline(|this, deadline| loop {
            let message = this.next_for_sequence(seq, deadline)?;
            match message.header.cmd_type {
                UbusCmdType::STATUS => {
                    Self::parse_status(message.blob.data)?;
                    return target_id.or_invalid("Invalid status message");
                }
                UbusCmdType::DATA => {
                    for attr in BlobIter::<UbusMsgAttr>::new(message.blob.data) {
                        match attr {
                            UbusMsgAttr::ObjId(id) => target_id = Some(id),
                            UbusMsgAttr::Signature(nested) => {
                                encoded_args =
                                    Some(Self::encode_signature_args(nested, method, args));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        })?;

        let args_bytes = encoded_args.transpose()?.unwrap_or_default();
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
        let mut json = Ok(String::new());
        self.invoke(obj_id, method, &args_bytes, |bi| {
            let res = if pretty {
                serde_json::to_string_pretty(&bi.as_table())
            } else {
                serde_json::to_string(&bi.as_table())
            };
            json = res.map_err(UbusError::ParseArguments);
        })?;
        json
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

    fn lookup_cb_dyn(
        &mut self,
        obj_path: &str,
        on_object: &mut dyn FnMut(ObjectResult<'_>),
        on_signature: &mut dyn FnMut(SignatureResult<'_>),
    ) -> Result<(), UbusError> {
        let seq = self.send_lookup_request(obj_path)?;
        self.with_deadline(|this, deadline| loop {
            let message = this.next_for_sequence(seq, deadline)?;
            match message.header.cmd_type {
                UbusCmdType::STATUS => return Self::parse_status(message.blob.data),
                UbusCmdType::DATA => {
                    let mut obj_path: Option<&str> = None;
                    let mut obj_id: Option<u32> = None;
                    let mut obj_type: Option<u32> = None;
                    for attr in BlobIter::<UbusMsgAttr>::new(message.blob.data) {
                        match attr {
                            UbusMsgAttr::ObjPath(path) => obj_path = Some(path),
                            UbusMsgAttr::ObjId(id) => obj_id = Some(id),
                            UbusMsgAttr::ObjType(ty) => obj_type = Some(ty),
                            UbusMsgAttr::Signature(nested) => {
                                let object = ObjectResult {
                                    path: obj_path.or_invalid("Missing obj_path")?,
                                    id: obj_id.or_invalid("Missing obj_id")?,
                                    ty: obj_type.or_invalid("Missing obj_type")?,
                                };
                                on_object(object);

                                for method_blob in nested {
                                    let (name, payload) = method_blob.into_named_payload()?;
                                    let args = Self::parse_method_policy(payload)?;
                                    on_signature(SignatureResult { object, name, args });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        })
    }

    pub fn lookup_cb(
        &mut self,
        obj_path: &str,
        mut on_object: impl FnMut(ObjectResult),
        mut on_signature: impl FnMut(SignatureResult),
    ) -> Result<(), UbusError> {
        self.lookup_cb_dyn(obj_path, &mut on_object, &mut on_signature)
    }

    fn lookup_id_until(
        &mut self,
        obj_path: &str,
        deadline: Option<Instant>,
    ) -> Result<u32, UbusError> {
        let seq = self.send_lookup_request(obj_path)?;
        let mut obj_id = 0u32;

        loop {
            let message = self.next_for_sequence(seq, deadline)?;
            match message.header.cmd_type {
                UbusCmdType::STATUS => {
                    Self::parse_status(message.blob.data)?;
                    return Ok(obj_id);
                }
                UbusCmdType::DATA => {
                    if let Some(id) = BlobIter::<UbusMsgAttr>::new(message.blob.data)
                        .filter_map(|attr| match attr {
                            UbusMsgAttr::ObjId(id) => Some(id),
                            _ => None,
                        })
                        .last()
                    {
                        obj_id = id;
                    }
                }
                _ => {}
            }
        }
    }

    /// Look up only the object ID for `obj_path` without parsing or allocating method signatures.
    pub fn lookup_id(&mut self, obj_path: &str) -> Result<u32, UbusError> {
        self.with_deadline(|this, deadline| this.lookup_id_until(obj_path, deadline))
    }

    fn lookup_dyn(
        &mut self,
        obj_path: &str,
        on_object: &mut dyn FnMut(UbusObject<'_>),
    ) -> Result<(), UbusError> {
        let seq = self.send_lookup_request(obj_path)?;
        self.with_deadline(|this, deadline| loop {
            let message = this.next_for_sequence(seq, deadline)?;
            match message.header.cmd_type {
                UbusCmdType::STATUS => return Self::parse_status(message.blob.data),
                UbusCmdType::DATA => {
                    let mut obj = UbusObject::default();
                    for attr in BlobIter::<UbusMsgAttr>::new(message.blob.data) {
                        match attr {
                            UbusMsgAttr::ObjPath(path) => obj.path = path,
                            UbusMsgAttr::ObjId(id) => obj.id = id,
                            UbusMsgAttr::ObjType(ty) => obj.ty = ty,
                            UbusMsgAttr::Signature(nested) => {
                                for method_blob in nested {
                                    let (name, payload) = method_blob.into_named_payload()?;
                                    let policy = Self::parse_method_policy(payload)?;
                                    obj.methods.insert(name, Method { name, policy });
                                }
                            }
                            _ => {}
                        }
                    }
                    on_object(obj);
                }
                _ => {}
            }
        })
    }

    pub fn lookup(
        &mut self,
        obj_path: &str,
        mut on_object: impl FnMut(UbusObject),
    ) -> Result<(), UbusError> {
        self.lookup_dyn(obj_path, &mut on_object)
    }

    fn encode_signature(methods: &[MethodSpec<'_>]) -> Result<Vec<u8>, UbusError> {
        let mut buf = Vec::new();
        let table_id = BlobMsgType::TABLE.value();
        let int32_id = BlobMsgType::INT32.value();
        for m in methods {
            let m_start = BlobMsgBuilder::begin_extended_in(&mut buf, table_id, m.name)?;
            for (arg_name, arg_ty) in m.policy {
                BlobMsgBuilder::write_extended_in(
                    &mut buf,
                    int32_id,
                    arg_name,
                    &(arg_ty.value() as i32).to_be_bytes(),
                )?;
            }
            BlobMsgBuilder::end_extended_in(&mut buf, m_start, table_id)?;
        }
        Ok(buf)
    }

    /// Register a ubus server object (or anonymous object if `spec.path` is `None`).
    pub fn add_object(&mut self, spec: &ObjectSpec<'_>) -> Result<RegisteredObject, UbusError> {
        let sig_bytes = Self::encode_signature(spec.methods)?;
        let path_attr = spec.path.map(UbusMsgAttr::ObjPath);
        let sig_attr =
            (!sig_bytes.is_empty()).then_some(UbusMsgAttr::Signature(BlobIter::new(&sig_bytes)));
        let (attrs_buf, len) = match (path_attr, sig_attr) {
            (Some(a), Some(b)) => ([a, b], 2),
            (Some(a), None) | (None, Some(a)) => ([a, a], 1),
            (None, None) => ([UbusMsgAttr::ObjId(0); 2], 0),
        };
        let seq = self.send_request(0, UbusCmdType::ADD_OBJECT, &attrs_buf[..len])?;
        let mut obj_id: Option<u32> = None;
        let mut obj_ty: u32 = 0;

        self.with_deadline(|this, deadline| loop {
            let message = this.next_for_sequence(seq, deadline)?;
            match message.header.cmd_type {
                UbusCmdType::DATA => {
                    for attr in BlobIter::<UbusMsgAttr>::new(message.blob.data) {
                        match attr {
                            UbusMsgAttr::ObjId(id) => obj_id = Some(id),
                            UbusMsgAttr::ObjType(ty) => obj_ty = ty,
                            _ => {}
                        }
                    }
                }
                UbusCmdType::STATUS => {
                    Self::parse_status(message.blob.data)?;
                    let id = obj_id.or_invalid("Missing obj_id in add_object")?;
                    return Ok(RegisteredObject { id, ty: obj_ty });
                }
                _ => {}
            }
        })
    }

    /// Unregister a previously registered object from `ubusd`.
    pub fn remove_object(&mut self, obj_id: u32) -> Result<(), UbusError> {
        let seq = self.send_request(0, UbusCmdType::REMOVE_OBJECT, &[UbusMsgAttr::ObjId(obj_id)])?;
        self.wait_status(seq)
    }

    /// Send a `DATA` reply chunk for an incoming request.
    pub fn reply_data(
        &mut self,
        req_header: UbusMsgHeader,
        obj_id: u32,
        data: &[u8],
    ) -> Result<(), UbusError> {
        self.send_frame(
            Self::reply_header(req_header, UbusCmdType::DATA),
            &[UbusMsgAttr::ObjId(obj_id), UbusMsgAttr::Data(data)],
        )
    }

    /// Send a final `STATUS` reply for an incoming request.
    pub fn reply_status(
        &mut self,
        req_header: UbusMsgHeader,
        obj_id: u32,
        status: UbusStatus,
    ) -> Result<(), UbusError> {
        self.send_frame(
            Self::reply_header(req_header, UbusCmdType::STATUS),
            &[
                UbusMsgAttr::Status(status.value()),
                UbusMsgAttr::ObjId(obj_id),
            ],
        )
    }

    fn send_ping_reply(&mut self, req_header: UbusMsgHeader) -> Result<(), UbusError> {
        self.send_frame(
            Self::reply_header(req_header, UbusCmdType::STATUS),
            &[UbusMsgAttr::Status(UbusStatus::OK.value())],
        )
    }

    fn prepare_dispatch(
        header: UbusMsgHeader,
        blob_data: &[u8],
        handler: &mut dyn FnMut(IncomingEvent<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<DispatchAction, UbusError> {
        match header.cmd_type {
            UbusCmdType::PING => Ok(DispatchAction::Ping(header)),
            UbusCmdType::INVOKE => {
                let f = Self::parse_invoke_fields(blob_data);
                let obj_id = f.obj_id.or_invalid("Missing obj_id in invoke")?;
                let method = f.method.or_invalid("Missing method in invoke")?;
                let req = IncomingRequest {
                    header,
                    obj_id,
                    method,
                    data: f.data,
                    no_reply: f.no_reply,
                    user: f.user,
                    group: f.group,
                };
                let result = handler(IncomingEvent::Invoke(req));
                if f.no_reply {
                    Ok(DispatchAction::Handled)
                } else {
                    match result {
                        Ok(RequestReply::Data(reply_bytes)) => {
                            Ok(DispatchAction::ReplyData(header, obj_id, reply_bytes))
                        }
                        Ok(RequestReply::Status(st)) | Err(st) => {
                            Ok(DispatchAction::ReplyStatus(header, obj_id, st))
                        }
                    }
                }
            }
            UbusCmdType::NOTIFY => {
                let (obj_id, active) = BlobIter::<UbusMsgAttr>::new(blob_data).fold(
                    (None, None),
                    |(obj_id, active), attr| match attr {
                        UbusMsgAttr::ObjId(id) => (Some(id), active),
                        UbusMsgAttr::Active(a) => (obj_id, Some(a)),
                        _ => (obj_id, active),
                    },
                );
                match (obj_id, active) {
                    (Some(obj_id), Some(active)) => {
                        let _ = handler(IncomingEvent::SubscribeActive { obj_id, active });
                        Ok(DispatchAction::Handled)
                    }
                    _ => Ok(DispatchAction::Ignored),
                }
            }
            UbusCmdType::REMOVE_OBJECT => {
                let (subscriber_id, target_id) = BlobIter::<UbusMsgAttr>::new(blob_data).fold(
                    (None, None),
                    |(sub_id, target_id), attr| match attr {
                        UbusMsgAttr::ObjId(id) => (Some(id), target_id),
                        UbusMsgAttr::Target(id) => (sub_id, Some(id)),
                        _ => (sub_id, target_id),
                    },
                );
                match (subscriber_id, target_id) {
                    (Some(subscriber_id), Some(target_id)) => {
                        let _ = handler(IncomingEvent::ObjectRemoved {
                            subscriber_id,
                            target_id,
                        });
                        Ok(DispatchAction::Handled)
                    }
                    _ => Ok(DispatchAction::Ignored),
                }
            }
            _ => Ok(DispatchAction::Ignored),
        }
    }

    fn execute_dispatch(&mut self, action: DispatchAction) -> Result<bool, UbusError> {
        match action {
            DispatchAction::Ping(header) => {
                self.send_ping_reply(header)?;
                Ok(false)
            }
            DispatchAction::ReplyData(header, obj_id, reply_bytes) => {
                self.reply_data(header, obj_id, &reply_bytes)?;
                self.reply_status(header, obj_id, UbusStatus::OK)?;
                Ok(true)
            }
            DispatchAction::ReplyStatus(header, obj_id, st) => {
                self.reply_status(header, obj_id, st)?;
                Ok(true)
            }
            DispatchAction::Handled => Ok(true),
            DispatchAction::Ignored => Ok(false),
        }
    }

    fn dispatch_pending_dyn(
        &mut self,
        handler: &mut dyn FnMut(IncomingEvent<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<usize, UbusError> {
        let mut count = 0;
        while let Some(pending) = self.pending.pop_front() {
            let action = Self::prepare_dispatch(pending.header, &pending.data, handler)?;
            count += usize::from(self.execute_dispatch(action)?);
        }
        self.reclaim_buffers();
        Ok(count)
    }

    /// Drain and dispatch all queued out-of-band messages without blocking on I/O.
    /// Returns the number of events delivered to `handler`.
    pub fn dispatch_pending(
        &mut self,
        mut handler: impl FnMut(IncomingEvent<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<usize, UbusError> {
        self.dispatch_pending_dyn(&mut handler)
    }

    fn dispatch_event_dyn(
        &mut self,
        handler: &mut dyn FnMut(IncomingEvent<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<bool, UbusError> {
        let action = if let Some(pending) = self.pending.pop_front() {
            Self::prepare_dispatch(pending.header, &pending.data, handler)?
        } else {
            let message = self.next_message()?;
            Self::prepare_dispatch(message.header, message.blob.data, handler)?
        };
        let res = self.execute_dispatch(action);
        self.reclaim_buffers();
        res
    }

    /// Read and dispatch one incoming message (draining queued pending messages first), handling
    /// RPC invocations, `ACTIVE` status updates, `REMOVE_OBJECT` subscriber notifications, and `PING` heartbeats.
    /// Returns `Ok(true)` if an invocation or pub/sub event was delivered to `handler`.
    pub fn dispatch_event(
        &mut self,
        mut handler: impl FnMut(IncomingEvent<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<bool, UbusError> {
        self.dispatch_event_dyn(&mut handler)
    }

    /// Run a blocking dispatch loop delivering all server invocations and pub/sub events to `handler`.
    pub fn serve_events(
        &mut self,
        mut handler: impl FnMut(IncomingEvent<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<(), UbusError> {
        loop {
            self.dispatch_event_dyn(&mut handler)?;
        }
    }

    /// Read and dispatch one incoming message. Returns `Ok(true)` if an `INVOKE` request was handled.
    pub fn dispatch_one(
        &mut self,
        mut handler: impl FnMut(&IncomingRequest<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<bool, UbusError> {
        let mut handled_invoke = false;
        self.dispatch_event_dyn(&mut |ev| match ev {
            IncomingEvent::Invoke(req) => {
                handled_invoke = true;
                handler(&req)
            }
            IncomingEvent::SubscribeActive { .. } | IncomingEvent::ObjectRemoved { .. } => {
                Ok(RequestReply::Status(UbusStatus::OK))
            }
        })?;
        Ok(handled_invoke)
    }

    /// Run a blocking server dispatch loop calling `handler` for each incoming `INVOKE` request.
    pub fn serve(
        &mut self,
        mut handler: impl FnMut(&IncomingRequest<'_>) -> Result<RequestReply, UbusStatus>,
    ) -> Result<(), UbusError> {
        loop {
            self.dispatch_one(&mut handler)?;
        }
    }

    /// Register an anonymous subscriber object (via `ADD_OBJECT`) for [`Connection::subscribe`].
    pub fn register_subscriber(&mut self) -> Result<RegisteredObject, UbusError> {
        self.add_object(&ObjectSpec {
            path: None,
            methods: &[],
        })
    }

    /// Subscribe `subscriber_id` to notifications published by `target_obj_id`.
    pub fn subscribe(&mut self, subscriber_id: u32, target_obj_id: u32) -> Result<(), UbusError> {
        let seq = self.send_request(
            0,
            UbusCmdType::SUBSCRIBE,
            &[
                UbusMsgAttr::ObjId(subscriber_id),
                UbusMsgAttr::Target(target_obj_id),
            ],
        )?;
        self.wait_status(seq)
    }

    /// Unsubscribe `subscriber_id` from notifications published by `target_obj_id`.
    pub fn unsubscribe(
        &mut self,
        subscriber_id: u32,
        target_obj_id: u32,
    ) -> Result<(), UbusError> {
        let seq = self.send_request(
            0,
            UbusCmdType::UNSUBSCRIBE,
            &[
                UbusMsgAttr::ObjId(subscriber_id),
                UbusMsgAttr::Target(target_obj_id),
            ],
        )?;
        self.wait_status(seq)
    }

    /// Send a fire-and-forget notification (`NOTIFY` with `NO_REPLY`) to all subscribers of `obj_id`.
    pub fn notify_no_reply(
        &mut self,
        obj_id: u32,
        notify_type: &str,
        data: &[u8],
    ) -> Result<(), UbusError> {
        self.send_call_frame(0, UbusCmdType::NOTIFY, obj_id, notify_type, data, true)?;
        Ok(())
    }

    /// Send a fire-and-forget notification by serializing `data` via serde into blob bytes.
    pub fn notify_no_reply_serde<A: serde::Serialize>(
        &mut self,
        obj_id: u32,
        notify_type: &str,
        data: &A,
    ) -> Result<(), UbusError> {
        let encoded = to_blob(data)?;
        self.notify_no_reply(obj_id, notify_type, &encoded)
    }

    /// Send a fire-and-forget notification by encoding a JSON object string into blob bytes.
    #[cfg(feature = "json")]
    pub fn notify_no_reply_json(
        &mut self,
        obj_id: u32,
        notify_type: &str,
        json: &str,
    ) -> Result<(), UbusError> {
        let encoded = json_to_blob(json)?;
        self.notify_no_reply(obj_id, notify_type, &encoded)
    }

    fn notify_dyn(
        &mut self,
        obj_id: u32,
        notify_type: &str,
        data: &[u8],
        on_data: &mut dyn FnMut(u32, BlobIter<Blob<'_>>),
        on_status: &mut dyn FnMut(u32, UbusStatus),
    ) -> Result<(), UbusError> {
        let seq = self.send_call_frame(0, UbusCmdType::NOTIFY, obj_id, notify_type, data, false)?;
        let mut expected_subs: usize = 0;
        let mut completed_subs: usize = 0;
        let mut server_done = false;

        self.with_deadline(|this, deadline| loop {
            let message = this.next_for_sequence(seq, deadline)?;

            match message.header.cmd_type {
                UbusCmdType::DATA => {
                    let (subs_delta, data_obj_id, payload) =
                        BlobIter::<UbusMsgAttr>::new(message.blob.data).fold(
                            (0usize, None, None),
                            |(cnt, id, data), attr| match attr {
                                UbusMsgAttr::Subscribers(subs) => (cnt + subs.count(), id, data),
                                UbusMsgAttr::ObjId(new_id) => (cnt, Some(new_id), data),
                                UbusMsgAttr::Data(d) => (cnt, id, Some(d)),
                                _ => (cnt, id, data),
                            },
                        );
                    expected_subs += subs_delta;
                    if let Some(d) = payload {
                        let sub_id = data_obj_id.unwrap_or_else(|| u32::from(message.header.peer));
                        on_data(sub_id, BlobIter::new(d));
                    }
                }
                UbusCmdType::STATUS => {
                    let status_code = Self::extract_status(message.blob.data)?;
                    let peer = u32::from(message.header.peer);
                    if peer == 0 {
                        if status_code != 0 {
                            return Err(UbusError::Status(status_code));
                        }
                        server_done = true;
                    } else {
                        completed_subs += 1;
                        on_status(peer, UbusStatus::from(status_code));
                    }
                    if server_done && completed_subs >= expected_subs {
                        return Ok(());
                    }
                }
                _ => {}
            }
        })
    }

    /// Publish a notification (`NOTIFY`) from `obj_id` to all current subscribers and collect
    /// per-subscriber `DATA` replies and final `STATUS` responses.
    pub fn notify(
        &mut self,
        obj_id: u32,
        notify_type: &str,
        data: &[u8],
        mut on_data: impl FnMut(u32, BlobIter<Blob>),
        mut on_status: impl FnMut(u32, UbusStatus),
    ) -> Result<(), UbusError> {
        self.notify_dyn(obj_id, notify_type, data, &mut on_data, &mut on_status)
    }

    /// Publish a notification by serializing `data` via serde into blob bytes.
    pub fn notify_serde<A: serde::Serialize>(
        &mut self,
        obj_id: u32,
        notify_type: &str,
        data: &A,
        on_data: impl FnMut(u32, BlobIter<Blob>),
        on_status: impl FnMut(u32, UbusStatus),
    ) -> Result<(), UbusError> {
        let encoded = to_blob(data)?;
        self.notify(obj_id, notify_type, &encoded, on_data, on_status)
    }

    /// Publish a notification by encoding a JSON object string into blob bytes.
    #[cfg(feature = "json")]
    pub fn notify_json(
        &mut self,
        obj_id: u32,
        notify_type: &str,
        json: &str,
        on_data: impl FnMut(u32, BlobIter<Blob>),
        on_status: impl FnMut(u32, UbusStatus),
    ) -> Result<(), UbusError> {
        let encoded = json_to_blob(json)?;
        self.notify(obj_id, notify_type, &encoded, on_data, on_status)
    }

    /// Broadcast an event (`ubus send`) with pre-encoded blob `data` for `event_type`.
    pub fn send_event(&mut self, event_type: &str, data: &[u8]) -> Result<(), UbusError> {
        let mut payload = Vec::with_capacity(32 + event_type.len() + data.len());
        BlobMsgBuilder::write_extended_str_in(&mut payload, "id", event_type)?;
        BlobMsgBuilder::write_extended_in(&mut payload, BlobMsgType::TABLE.value(), "data", data)?;
        self.invoke(UBUS_SYSTEM_OBJECT_EVENT, "send", &payload, |_| {})
    }

    /// Broadcast an event by serializing `data` via serde into blob bytes.
    pub fn send_event_serde<A: serde::Serialize>(
        &mut self,
        event_type: &str,
        data: &A,
    ) -> Result<(), UbusError> {
        let encoded = to_blob(data)?;
        self.send_event(event_type, &encoded)
    }

    /// Broadcast an event by encoding a JSON object string directly into blob bytes.
    #[cfg(feature = "json")]
    pub fn send_event_json(&mut self, event_type: &str, json: &str) -> Result<(), UbusError> {
        let encoded = json_to_blob(json)?;
        self.send_event(event_type, &encoded)
    }

    fn register_event_listener(
        &mut self,
        pattern: &str,
        deadline: Option<Instant>,
    ) -> Result<RegisteredObject, UbusError> {
        let reg = self.register_subscriber()?;
        let reg_res = (|| -> Result<(), UbusError> {
            let mut reg_args = Vec::with_capacity(32 + pattern.len());
            BlobMsgBuilder::write_extended_in(
                &mut reg_args,
                BlobMsgType::INT32.value(),
                "object",
                &(reg.id as i32).to_be_bytes(),
            )?;
            BlobMsgBuilder::write_extended_str_in(&mut reg_args, "pattern", pattern)?;
            let seq = self.send_call_frame(
                UBUS_SYSTEM_OBJECT_EVENT,
                UbusCmdType::INVOKE,
                UBUS_SYSTEM_OBJECT_EVENT,
                "register",
                &reg_args,
                false,
            )?;
            self.wait_status_until(seq, deadline)
        })();
        if let Err(e) = reg_res {
            self.finish_deadline(deadline);
            if !matches!(e, UbusError::Timeout) {
                let _ = self.remove_object(reg.id);
            }
            return Err(e);
        }
        Ok(reg)
    }

    fn poll_registered_events(
        &mut self,
        reg_id: u32,
        deadline: Option<Instant>,
        on_event: &mut dyn FnMut(&str, BlobIter<Blob<'_>>) -> bool,
    ) -> Result<(), UbusError> {
        let pending_len = self.pending.len();
        for _ in 0..pending_len {
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            if pending.header.cmd_type == UbusCmdType::INVOKE {
                let f = Self::parse_invoke_fields(&pending.data);
                if f.obj_id == Some(reg_id) {
                    let keep_going = on_event(f.method.unwrap_or(""), BlobIter::new(f.data));
                    if !f.no_reply {
                        self.reply_status(pending.header, reg_id, UbusStatus::OK)?;
                    }
                    if !keep_going {
                        self.reclaim_buffers();
                        return Ok(());
                    }
                    continue;
                }
            }
            self.pending.push_back(pending);
        }
        self.reclaim_buffers();

        loop {
            let (hdr, tag) = self.read_raw_until(deadline)?;
            match hdr.cmd_type {
                UbusCmdType::PING => self.send_ping_reply(hdr)?,
                UbusCmdType::INVOKE => {
                    let payload_len = tag.inner_len();
                    let f = Self::parse_invoke_fields(
                        &self.buffer[UbusMsg::PREFIX_LEN..UbusMsg::PREFIX_LEN + payload_len],
                    );
                    if f.obj_id == Some(reg_id) {
                        let no_reply = f.no_reply;
                        let keep_going = on_event(f.method.unwrap_or(""), BlobIter::new(f.data));
                        if !no_reply {
                            self.reply_status(hdr, reg_id, UbusStatus::OK)?;
                        }
                        if !keep_going {
                            self.reclaim_buffer();
                            return Ok(());
                        }
                    } else {
                        self.enqueue_raw_pending(hdr, payload_len);
                    }
                }
                UbusCmdType::NOTIFY | UbusCmdType::REMOVE_OBJECT => {
                    self.enqueue_raw_pending(hdr, tag.inner_len());
                }
                _ => {}
            }
            self.reclaim_buffer();
        }
    }

    fn match_object_add_event(data: &[u8], obj_path: &str) -> Option<u32> {
        let (added_id, matched_path) = BlobIter::<Blob>::new(data)
            .filter_map(|blob| blob.into_named_payload().ok())
            .fold((None, false), |(id, matched), (name, payload)| match name {
                "id" => (
                    TryInto::<i32>::try_into(payload).ok().map(|v| v as u32),
                    matched,
                ),
                "path" => (
                    id,
                    matched || TryInto::<&str>::try_into(payload).ok() == Some(obj_path),
                ),
                _ => (id, matched),
            });
        matched_path.then_some(added_id).flatten()
    }

    /// Listen for ubus events matching `pattern` (`ubus listen`) until `on_event` returns `false`.
    pub fn listen_events(
        &mut self,
        pattern: &str,
        mut on_event: impl FnMut(&str, BlobIter<Blob>) -> bool,
    ) -> Result<(), UbusError> {
        let reg_deadline = self.deadline();
        let reg = self.register_event_listener(pattern, reg_deadline)?;
        self.finish_deadline(reg_deadline);

        let listen_res = self.poll_registered_events(reg.id, None, &mut on_event);
        let remove_res = self.remove_object(reg.id);
        listen_res?;
        remove_res
    }

    /// Replace the underlying transport with `io`, reset session state, and complete the `HELLO` handshake.
    pub fn reconnect_with(&mut self, mut io: T) -> Result<(), UbusError> {
        if let Some(t) = self.timeout() {
            io.set_read_timeout(Some(t))?;
            io.set_write_timeout(Some(t))?;
        }
        self.io = io;
        self.peer = 0;
        self.sequence = 0;
        self.pending.clear();
        self.buffer.clear();
        self.reclaim_buffers();
        self.read_hello()
    }

    /// Reconnect using `io` and re-register each object specification in `objects`.
    pub fn reconnect_and_restore(
        &mut self,
        io: T,
        objects: &[ObjectSpec<'_>],
    ) -> Result<Vec<RegisteredObject>, UbusError> {
        self.reconnect_with(io)?;
        objects.iter().map(|spec| self.add_object(spec)).collect()
    }

    fn monitor_dyn(
        &mut self,
        on_msg: &mut dyn FnMut(MonitorMessage<'_>) -> bool,
    ) -> Result<(), UbusError> {
        let seq = self.send_request(0, UbusCmdType::MONITOR, &[])?;
        self.wait_status(seq)?;

        loop {
            let (hdr, tag) = self.read_raw_until(None)?;
            match hdr.cmd_type {
                UbusCmdType::PING => self.send_ping_reply(hdr)?,
                UbusCmdType::MONITOR => {
                    let total_len = UbusMsg::PREFIX_LEN + tag.inner_len();
                    let keep_going = {
                        let mon = MonitorMessage::from_blob_data(
                            &self.buffer[UbusMsg::PREFIX_LEN..total_len],
                        )?;
                        on_msg(mon)
                    };
                    self.reclaim_buffer();
                    if !keep_going {
                        return Ok(());
                    }
                }
                _ => self.reclaim_buffer(),
            }
        }
    }

    /// Enter `ubusd` monitor mode (`MONITOR`) and stream bus traffic to `on_msg` until it returns `false`.
    pub fn monitor(
        &mut self,
        mut on_msg: impl FnMut(MonitorMessage<'_>) -> bool,
    ) -> Result<(), UbusError> {
        self.monitor_dyn(&mut on_msg)
    }

    /// Wait for `obj_path` to appear on the ubus bus, returning its object ID once registered.
    pub fn wait_for(
        &mut self,
        obj_path: &str,
        timeout: Option<Duration>,
    ) -> Result<u32, UbusError> {
        let deadline = timeout.or_else(|| self.timeout()).map(|d| Instant::now() + d);
        let reg = self.register_event_listener("ubus.object.add", deadline)?;

        let wait_res = (|| -> Result<u32, UbusError> {
            match self.lookup_id_until(obj_path, deadline) {
                Ok(id) if id != 0 => return Ok(id),
                Ok(_) => {}
                Err(UbusError::Status(code)) if UbusStatus::from(code) == UbusStatus::NOT_FOUND => {}
                Err(e) => return Err(e),
            }

            let mut found_id = None;
            self.poll_registered_events(reg.id, deadline, &mut |method, bi| {
                if method == "ubus.object.add" {
                    found_id = Self::match_object_add_event(bi.as_bytes(), obj_path);
                }
                found_id.is_none()
            })?;
            found_id.or_invalid("Missing matched object id")
        })();

        self.finish_deadline(deadline);
        match wait_res {
            Ok(id) => {
                self.remove_object(reg.id)?;
                Ok(id)
            }
            Err(e) => {
                if !matches!(e, UbusError::Timeout) {
                    let _ = self.remove_object(reg.id);
                }
                Err(e)
            }
        }
    }
}
