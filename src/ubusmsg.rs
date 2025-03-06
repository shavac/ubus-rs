use crate::{Blob, BlobBuilder, BlobIter, BlobMsgPayload, BlobTag, IO, Payload, UbusError};
use core::convert::TryInto;
use core::mem::{size_of, transmute};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use storage_endian::{BEu16, BEu32};

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
    NOTIFY          = 0x10,
    MONITOR         = 0x11,
});

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
    pub fn from_bytes(buffer: [u8; Self::SIZE]) -> Self {
        unsafe { transmute(buffer) }
    }
    // Dump out bytes of MessageHeader
    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        unsafe { core::mem::transmute(self) }
    }
}

#[derive(Copy, Clone)]
pub struct UbusMsg<'a> {
    pub header: UbusMsgHeader,
    pub blob: Blob<'a>,
}

impl<'a> UbusMsg<'a> {
    pub fn from_io<T: IO>(io: &mut T, buffer: &'a mut [u8]) -> Result<Self, UbusError> {
        let (pre_buffer, buffer) = buffer.split_at_mut(UbusMsgHeader::SIZE + BlobTag::SIZE);

        // Read in the message header and the following blob tag
        io.get(pre_buffer)?;

        let (header, tag) = pre_buffer.split_at(UbusMsgHeader::SIZE);

        let header = UbusMsgHeader::from_bytes(header.try_into().unwrap());
        valid_data!(header.version == UbusMsgVersion::CURRENT, "Wrong version");

        let tag = BlobTag::from_bytes(tag.try_into().unwrap());
        tag.is_valid()?;

        // Get a slice the size of the blob's data bytes (do we need to worry about padding here?)
        let data = &mut buffer[..tag.inner_len()];

        // Receive data into slice
        io.get(data)?;

        // Create the blob from our parts
        let blob = Blob::from_tag_and_data(tag, data).unwrap();

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
            buffer.len() >= (UbusMsgHeader::SIZE + BlobTag::SIZE),
            "Builder buffer is too small"
        );

        let header_buf = &mut buffer[..UbusMsgHeader::SIZE];
        let header_buf: &mut [u8; UbusMsgHeader::SIZE] = header_buf.try_into().unwrap();
        *header_buf = header.to_bytes();

        let offset = UbusMsgHeader::SIZE + BlobTag::SIZE;

        Ok(Self { buffer, offset })
    }

    pub fn put(&mut self, attr: UbusMsgAttr) -> Result<(), UbusError> {
        let mut blob = BlobBuilder::from_bytes(&mut self.buffer[self.offset..]);

        match attr {
            UbusMsgAttr::Status(val) => blob.push_u32(BlobAttrId::STATUS.value(), val as u32)?,
            UbusMsgAttr::ObjPath(val) => blob.push_str(BlobAttrId::OBJPATH.value(), val)?,
            UbusMsgAttr::ObjId(val) => blob.push_u32(BlobAttrId::OBJID.value(), val)?,
            UbusMsgAttr::Method(val) => blob.push_str(BlobAttrId::METHOD.value(), val)?,
            //UbusMsgAttr::ObjType(val) => blob.push_u32(BlobAttrId::STATUS.value(), val)?,
            UbusMsgAttr::ObjType(val) => blob.push_u32(BlobAttrId::OBJTYPE.value(), val)?,
            UbusMsgAttr::Signature(_) => unimplemented!(),
            UbusMsgAttr::Data(val) => blob.push_bytes(BlobAttrId::DATA.value(), val)?,
            UbusMsgAttr::Target(val) => blob.push_u32(BlobAttrId::TARGET.value(), val)?,
            UbusMsgAttr::Active(val) => blob.push_bool(BlobAttrId::ACTIVE.value(), val)?,
            UbusMsgAttr::NoReply(val) => blob.push_bool(BlobAttrId::NO_REPLY.value(), val)?,
            UbusMsgAttr::Subscribers(_) => unimplemented!(),
            UbusMsgAttr::User(val) => blob.push_str(BlobAttrId::USER.value(), val)?,
            UbusMsgAttr::Group(val) => blob.push_str(BlobAttrId::GROUP.value(), val)?,
            UbusMsgAttr::Unknown(id, val) => blob.push_bytes(id.value(), val)?,
        };

        self.offset += blob.len();
        Ok(())
    }

    pub fn finish(self) -> &'a [u8] {
        // Update tag with correct size
        let tag = BlobTag::new(0, self.offset - UbusMsgHeader::SIZE, false).unwrap();
        let tag_buf = &mut self.buffer[UbusMsgHeader::SIZE..UbusMsgHeader::SIZE + BlobTag::SIZE];
        let tag_buf: &mut [u8; BlobTag::SIZE] = tag_buf.try_into().unwrap();
        *tag_buf = tag.to_bytes();
        &self.buffer[..self.offset]
    }
}
impl<'a> Into<&'a [u8]> for UbusMsgBuilder<'a> {
    fn into(self) -> &'a [u8] {
        self.finish()
    }
}

#[derive(Debug)]
pub enum UbusMsgAttr<'a> {
    Status(i32),
    ObjPath(&'a str),
    ObjId(u32),
    Method(&'a str),
    ObjType(u32),
    Signature(HashMap<&'a str, BlobMsgPayload<'a>>),
    Data(&'a [u8]),
    Target(u32),
    Active(bool),
    NoReply(bool),
    Subscribers(BlobIter<'a, Blob<'a>>),
    User(&'a str),
    Group(&'a str),
    Unknown(BlobAttrId, &'a [u8]),
}

impl<'a> From<Blob<'a>> for UbusMsgAttr<'a> {
    fn from(blob: Blob<'a>) -> Self {
        let payload = Payload::from(blob.data);
        match blob.tag.id().into() {
            BlobAttrId::STATUS => UbusMsgAttr::Status(payload.try_into().unwrap()),
            BlobAttrId::OBJPATH => UbusMsgAttr::ObjPath(payload.try_into().unwrap()),
            BlobAttrId::OBJID => UbusMsgAttr::ObjId(payload.try_into().unwrap()),
            BlobAttrId::METHOD => UbusMsgAttr::Method(payload.try_into().unwrap()),
            BlobAttrId::OBJTYPE => UbusMsgAttr::ObjType(payload.try_into().unwrap()),
            BlobAttrId::SIGNATURE => UbusMsgAttr::Signature(payload.try_into().unwrap()),
            BlobAttrId::DATA => UbusMsgAttr::Data(payload.try_into().unwrap()),
            BlobAttrId::TARGET => UbusMsgAttr::Target(payload.try_into().unwrap()),
            BlobAttrId::ACTIVE => UbusMsgAttr::Active(payload.try_into().unwrap()),
            BlobAttrId::NO_REPLY => UbusMsgAttr::NoReply(payload.try_into().unwrap()),
            BlobAttrId::SUBSCRIBERS => UbusMsgAttr::Subscribers(payload.try_into().unwrap()),
            BlobAttrId::USER => UbusMsgAttr::User(payload.try_into().unwrap()),
            BlobAttrId::GROUP => UbusMsgAttr::Group(payload.try_into().unwrap()),
            id => UbusMsgAttr::Unknown(id, blob.data.into()),
        }
    }
}
