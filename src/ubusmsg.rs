use crate::{Blob, BlobBuilder, BlobIter, BlobTag, IO, Payload, UbusError};
use alloc::vec::Vec;
use core::convert::{TryFrom, TryInto};
use core::mem::{size_of, transmute};
use serde::{Deserialize, Serialize};
use storage_endian::{BEu16, BEu32};

/// Maximum allowed size of a single ubus message (64 KiB).
pub const MAX_MSG_SIZE: usize = 64 * 1024;

/// Built-in ubusd system object IDs.
pub const UBUS_SYSTEM_OBJECT_EVENT: u32 = 1;
pub const UBUS_SYSTEM_OBJECT_ACL: u32 = 2;

values!(pub UbusMsgVersion(u8) {
    CURRENT = 0x00,
});

values!(pub UbusCmdType(u8) {
    HELLO           = 0x00,
    STATUS          = 0x01,
    DATA            = 0x02,
    PING            = 0x03,
    LOOKUP          = 0x04,
    INVOKE          = 0x05,
    ADD_OBJECT      = 0x06,
    REMOVE_OBJECT   = 0x07,
    SUBSCRIBE       = 0x08,
    UNSUBSCRIBE     = 0x09,
    NOTIFY          = 0x0a,
    MONITOR         = 0x0b,
    INVOKE_FD       = 0x0c,
});

values!(pub UbusStatus(i32) {
    OK                = 0,
    INVALID_COMMAND   = 1,
    INVALID_ARGUMENT  = 2,
    METHOD_NOT_FOUND  = 3,
    NOT_FOUND         = 4,
    NO_DATA           = 5,
    PERMISSION_DENIED = 6,
    TIMEOUT           = 7,
    NOT_SUPPORTED     = 8,
    UNKNOWN_ERROR     = 9,
    CONNECTION_FAILED = 10,
    NO_MEMORY         = 11,
    PARSE_ERROR       = 12,
    SYSTEM_ERROR      = 13,
});

impl UbusStatus {
    /// Human-readable status description matching `libubus` `ubus_strerror`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OK => "Success",
            Self::INVALID_COMMAND => "Invalid command",
            Self::INVALID_ARGUMENT => "Invalid argument",
            Self::METHOD_NOT_FOUND => "Method not found",
            Self::NOT_FOUND => "Not found",
            Self::NO_DATA => "No response",
            Self::PERMISSION_DENIED => "Permission denied",
            Self::TIMEOUT => "Request timed out",
            Self::NOT_SUPPORTED => "Operation not supported",
            Self::UNKNOWN_ERROR => "Unknown error",
            Self::CONNECTION_FAILED => "Connection failed",
            Self::NO_MEMORY => "Out of memory",
            Self::PARSE_ERROR => "Parsing message data failed",
            Self::SYSTEM_ERROR => "System error",
            _ => "Unknown status",
        }
    }
}

impl core::fmt::Display for UbusStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

values!(pub BlobAttrId(u32) {
    UNSPEC      = 0x00,
    STATUS      = 0x01,
    OBJPATH     = 0x02,
    OBJID       = 0x03,
    METHOD      = 0x04,
    OBJTYPE     = 0x05,
    SIGNATURE   = 0x06,
    DATA        = 0x07,
    TARGET      = 0x08,
    ACTIVE      = 0x09,
    NO_REPLY    = 0x0a,
    SUBSCRIBERS = 0x0b,
    USER        = 0x0c,
    GROUP       = 0x0d,
});

values!(pub MonitorAttrId(u32) {
    CLIENT = 0,
    PEER   = 1,
    SEND   = 2,
    SEQ    = 3,
    TYPE   = 4,
    STATUS = 5,
    DATA   = 6,
});

/// Zero-copy view of a bus traffic frame delivered in `MONITOR` mode (`ubus monitor`).
#[derive(Copy, Clone, Debug)]
pub struct MonitorMessage<'a> {
    pub client: u32,
    pub peer: u32,
    pub send: bool,
    pub seq: u32,
    pub cmd_type: UbusCmdType,
    pub status: Option<UbusStatus>,
    pub data: &'a [u8],
}

impl<'a> MonitorMessage<'a> {
    /// Parse a `MONITOR` message payload into a zero-copy [`MonitorMessage`].
    pub fn from_blob_data(data: &'a [u8]) -> Result<Self, UbusError> {
        let mut msg = Self {
            client: 0,
            peer: 0,
            send: false,
            seq: 0,
            cmd_type: UbusCmdType::HELLO,
            status: None,
            data: &[],
        };
        for blob in BlobIter::<Blob>::new(data) {
            let payload = Payload::from(blob.data);
            match MonitorAttrId::from(blob.tag.id()) {
                MonitorAttrId::CLIENT => msg.client = payload.try_into()?,
                MonitorAttrId::PEER => msg.peer = payload.try_into()?,
                MonitorAttrId::SEND => msg.send = payload.try_into()?,
                MonitorAttrId::SEQ => msg.seq = payload.try_into()?,
                MonitorAttrId::TYPE => {
                    msg.cmd_type = UbusCmdType::from(TryInto::<u32>::try_into(payload)? as u8);
                }
                MonitorAttrId::STATUS => {
                    msg.status = Some(UbusStatus::from(TryInto::<i32>::try_into(payload)?));
                }
                MonitorAttrId::DATA => msg.data = payload.into(),
                _ => {}
            }
        }
        Ok(msg)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct UbusMsgHeader {
    pub version: UbusMsgVersion,
    pub cmd_type: UbusCmdType,
    pub sequence: BEu16,
    pub peer: BEu32,
}

impl UbusMsgHeader {
    pub const SIZE: usize = size_of::<Self>();

    /// Create MessageHeader from a byte array
    #[inline]
    pub fn from_bytes(buffer: [u8; Self::SIZE]) -> Self {
        unsafe { transmute(buffer) }
    }
    /// Dump out bytes of MessageHeader
    #[inline]
    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        unsafe { transmute(self) }
    }
}

#[derive(Copy, Clone)]
pub struct UbusMsg<'a> {
    pub header: UbusMsgHeader,
    pub blob: Blob<'a>,
}

impl<'a> UbusMsg<'a> {
    pub(crate) const PREFIX_LEN: usize = UbusMsgHeader::SIZE + BlobTag::SIZE;

    fn parse_prefix(prefix: &[u8]) -> Result<(UbusMsgHeader, BlobTag), UbusError> {
        let (header_bytes, tag_bytes) = prefix.split_at(UbusMsgHeader::SIZE);
        let header = UbusMsgHeader::from_bytes(header_bytes.try_into().unwrap());
        valid_data!(header.version == UbusMsgVersion::CURRENT, "Wrong version");
        let tag = BlobTag::from_bytes(tag_bytes.try_into().unwrap());
        tag.is_valid()?;
        Ok((header, tag))
    }

    pub fn from_io<T: IO>(io: &mut T, buffer: &'a mut [u8]) -> Result<Self, UbusError> {
        valid_data!(buffer.len() >= Self::PREFIX_LEN, "Message buffer too small");
        let (pre_buffer, buffer) = buffer.split_at_mut(Self::PREFIX_LEN);

        // Read in the message header and the following blob tag
        io.get(pre_buffer)?;

        let (header, tag) = Self::parse_prefix(pre_buffer)?;
        valid_data!(
            buffer.len() >= tag.inner_len(),
            "Message payload exceeds buffer"
        );
        let data = &mut buffer[..tag.inner_len()];

        // Receive data into slice
        io.get(data)?;

        Ok(UbusMsg {
            header,
            blob: Blob::from_tag_and_data(tag, data)?,
        })
    }

    /// Read a message frame into `buffer` and return its parsed `(UbusMsgHeader, BlobTag)`.
    pub(crate) fn read_into_vec<T: IO>(
        io: &mut T,
        buffer: &mut Vec<u8>,
    ) -> Result<(UbusMsgHeader, BlobTag), UbusError> {
        if buffer.len() < Self::PREFIX_LEN {
            buffer.resize(Self::PREFIX_LEN, 0);
        }

        io.get(&mut buffer[..Self::PREFIX_LEN])?;

        let (header, tag) = Self::parse_prefix(&buffer[..Self::PREFIX_LEN])?;
        let total_len = Self::PREFIX_LEN + tag.inner_len();
        valid_data!(total_len <= MAX_MSG_SIZE, "Ubus message exceeds max size");

        if buffer.len() < total_len {
            buffer.resize(total_len, 0);
        } else {
            buffer.truncate(total_len);
            if buffer.capacity() > 4096 && total_len <= 1024 {
                buffer.shrink_to(4096);
            }
        }

        io.get(&mut buffer[Self::PREFIX_LEN..total_len])?;
        Ok((header, tag))
    }

    /// Read a message into a dynamically-sized `Vec<u8>` buffer, growing only as needed
    /// and shrinking back if a large spike buffer is no longer needed.
    pub fn from_io_vec<T: IO>(io: &mut T, buffer: &'a mut Vec<u8>) -> Result<Self, UbusError> {
        let (header, tag) = Self::read_into_vec(io, buffer)?;
        let total_len = Self::PREFIX_LEN + tag.inner_len();
        let blob = Blob::from_tag_and_data(tag, &buffer[Self::PREFIX_LEN..total_len])?;

        Ok(UbusMsg { header, blob })
    }
}

impl core::fmt::Debug for UbusMsg<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(
            f,
            "Message({:?} seq={} peer={:08x}, size={})",
            self.header.cmd_type,
            self.header.sequence,
            self.header.peer,
            self.blob.data.len()
        )
    }
}

pub struct UbusMsgBuilder<'a> {
    buffer: &'a mut [u8],
    offset: usize,
}

impl<'a> UbusMsgBuilder<'a> {
    pub fn new(buffer: &'a mut [u8], header: &UbusMsgHeader) -> Result<Self, UbusError> {
        valid_data!(
            buffer.len() >= UbusMsg::PREFIX_LEN,
            "Builder buffer is too small"
        );
        buffer[..UbusMsgHeader::SIZE].copy_from_slice(&header.to_bytes());
        Ok(Self {
            buffer,
            offset: UbusMsg::PREFIX_LEN,
        })
    }

    pub fn put(&mut self, attr: UbusMsgAttr) -> Result<(), UbusError> {
        let mut blob = BlobBuilder::from_bytes(&mut self.buffer[self.offset..]);

        match attr {
            UbusMsgAttr::Status(val) => blob.push_u32(BlobAttrId::STATUS.value(), val as u32)?,
            UbusMsgAttr::ObjPath(val) => blob.push_str(BlobAttrId::OBJPATH.value(), val)?,
            UbusMsgAttr::ObjId(val) => blob.push_u32(BlobAttrId::OBJID.value(), val)?,
            UbusMsgAttr::Method(val) => blob.push_str(BlobAttrId::METHOD.value(), val)?,
            UbusMsgAttr::ObjType(val) => blob.push_u32(BlobAttrId::OBJTYPE.value(), val)?,
            UbusMsgAttr::Signature(iter) => {
                blob.push_slice(BlobAttrId::SIGNATURE.value(), iter.as_bytes())?;
            }
            UbusMsgAttr::Data(val) => blob.push_slice(BlobAttrId::DATA.value(), val)?,
            UbusMsgAttr::Target(val) => blob.push_u32(BlobAttrId::TARGET.value(), val)?,
            UbusMsgAttr::Active(val) => blob.push_bool(BlobAttrId::ACTIVE.value(), val)?,
            UbusMsgAttr::NoReply(val) => blob.push_bool(BlobAttrId::NO_REPLY.value(), val)?,
            UbusMsgAttr::Subscribers(iter) => {
                blob.push_slice(BlobAttrId::SUBSCRIBERS.value(), iter.as_bytes())?;
            }
            UbusMsgAttr::User(val) => blob.push_str(BlobAttrId::USER.value(), val)?,
            UbusMsgAttr::Group(val) => blob.push_str(BlobAttrId::GROUP.value(), val)?,
            UbusMsgAttr::Unknown(id, val) => blob.push_slice(id.value(), val)?,
        }

        self.offset += blob.len();
        Ok(())
    }

    pub fn finish(self) -> &'a [u8] {
        // Update tag with correct size
        let tag = BlobTag::new(0, self.offset - UbusMsgHeader::SIZE, false).unwrap();
        self.buffer[UbusMsgHeader::SIZE..UbusMsg::PREFIX_LEN].copy_from_slice(&tag.to_bytes());
        &self.buffer[..self.offset]
    }
}

impl<'a> From<UbusMsgBuilder<'a>> for &'a [u8] {
    fn from(val: UbusMsgBuilder<'a>) -> Self {
        val.finish()
    }
}

#[derive(Copy, Clone, Debug)]
pub enum UbusMsgAttr<'a> {
    Status(i32),
    ObjPath(&'a str),
    ObjId(u32),
    Method(&'a str),
    ObjType(u32),
    Signature(BlobIter<'a, Blob<'a>>),
    Data(&'a [u8]),
    Target(u32),
    Active(bool),
    NoReply(bool),
    Subscribers(BlobIter<'a, Blob<'a>>),
    User(&'a str),
    Group(&'a str),
    Unknown(BlobAttrId, &'a [u8]),
}

impl UbusMsgAttr<'_> {
    pub(crate) fn wire_len(&self) -> usize {
        let payload_len = match self {
            Self::Status(_) | Self::ObjId(_) | Self::ObjType(_) | Self::Target(_) => 4,
            Self::Active(_) | Self::NoReply(_) => 1,
            Self::ObjPath(s) | Self::Method(s) | Self::User(s) | Self::Group(s) => s.len() + 1,
            Self::Signature(it) | Self::Subscribers(it) => it.as_bytes().len(),
            Self::Data(b) | Self::Unknown(_, b) => b.len(),
        };
        let unpadded = BlobTag::SIZE + payload_len;
        unpadded + BlobTag::align_padding(unpadded)
    }
}

impl<'a> TryFrom<Blob<'a>> for UbusMsgAttr<'a> {
    type Error = UbusError;

    fn try_from(blob: Blob<'a>) -> Result<Self, Self::Error> {
        let payload = Payload::from(blob.data);
        let attr = match blob.tag.id().into() {
            BlobAttrId::STATUS => UbusMsgAttr::Status(payload.try_into()?),
            BlobAttrId::OBJPATH => UbusMsgAttr::ObjPath(payload.try_into()?),
            BlobAttrId::OBJID => UbusMsgAttr::ObjId(payload.try_into()?),
            BlobAttrId::METHOD => UbusMsgAttr::Method(payload.try_into()?),
            BlobAttrId::OBJTYPE => UbusMsgAttr::ObjType(payload.try_into()?),
            BlobAttrId::SIGNATURE => UbusMsgAttr::Signature(payload.into()),
            BlobAttrId::DATA => UbusMsgAttr::Data(payload.into()),
            BlobAttrId::TARGET => UbusMsgAttr::Target(payload.try_into()?),
            BlobAttrId::ACTIVE => UbusMsgAttr::Active(payload.try_into()?),
            BlobAttrId::NO_REPLY => UbusMsgAttr::NoReply(payload.try_into()?),
            BlobAttrId::SUBSCRIBERS => UbusMsgAttr::Subscribers(payload.into()),
            BlobAttrId::USER => UbusMsgAttr::User(payload.try_into()?),
            BlobAttrId::GROUP => UbusMsgAttr::Group(payload.try_into()?),
            id => UbusMsgAttr::Unknown(id, blob.data),
        };
        Ok(attr)
    }
}
