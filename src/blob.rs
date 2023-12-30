use crate::{BlobMsg, BlobMsgPayload, BlobMsgType, UbusError};

use super::Error;
use core::convert::{TryFrom, TryInto};
use core::marker::PhantomData;
use core::mem::{align_of, size_of, transmute};
use core::str;
use std::collections::HashMap;
use std::vec::Vec;
use storage_endian::BEu32;

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct BlobTag(BEu32);
impl BlobTag {
    pub const SIZE: usize = size_of::<Self>();
    const ID_MASK: u32 = 0x7f;
    const ID_SHIFT: u32 = 24;
    const LEN_MASK: u32 = 0xff_ff_ff;
    const EXTENDED_BIT: u32 = 1 << 31;
    const ALIGNMENT: usize = align_of::<Self>();

    pub fn new(id: u32, len: usize, extended: bool) -> Result<Self, UbusError> {
        if id > Self::ID_MASK || len < Self::SIZE || len > Self::LEN_MASK as usize {
            Err(UbusError::InvalidData("Invalid TAG construction"))
        } else {
            let id = id & Self::ID_MASK;
            let len = len as u32 & Self::LEN_MASK;
            let mut val = len | (id << Self::ID_SHIFT);
            if extended {
                val = val | Self::EXTENDED_BIT;
            }
            Ok(Self(val.into()))
        }
    }

    /// Create BlobTag from a byte array
    pub fn from_bytes(bytes: [u8; Self::SIZE]) -> Self {
        unsafe { transmute(bytes) }
    }
    // Dump out bytes of MessageHeader
    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        unsafe { core::mem::transmute(self) }
    }
    /// ID code of this blob
    pub fn id(&self) -> u32 {
        u32::from((self.0 >> Self::ID_SHIFT) & Self::ID_MASK)
    }
    /// Total number of bytes this blob contains (header + data)
    pub fn size(&self) -> usize {
        u32::from(self.0 & Self::LEN_MASK) as usize
    }

    pub fn set_size(&mut self, size: usize) {
        let tag = Self::new(self.id(), size, self.is_extended()).unwrap();
        self.0 = tag.0
    }
    /// Number of padding bytes between this blob and the next blob
    fn padding(&self) -> usize {
        Self::ALIGNMENT.wrapping_sub(self.size()) & (Self::ALIGNMENT - 1)
    }
    /// Number of bytes to the next tag
    fn next_tag(&self) -> usize {
        self.size() + self.padding()
    }
    /// Total number of bytes following the tag (extended header + data)
    pub fn inner_len(&self) -> usize {
        self.size().saturating_sub(Self::SIZE)
    }
    /// Is this an "extended" blob
    pub fn is_extended(&self) -> bool {
        (self.0 & Self::EXTENDED_BIT) != 0
    }
    /// Does this blob look valid
    pub fn is_valid(&self) -> Result<(), UbusError> {
        valid_data!(self.size() >= Self::SIZE, "Tag size smaller than tag");
        Ok(())
    }
}
impl core::fmt::Debug for BlobTag {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let (id, len) = (self.id(), self.size());
        let extended = if self.is_extended() { ", extended" } else { "" };
        write!(f, "BlobTag(id={:?}, len={}{})", id, len, extended)
    }
}

pub struct BlobBuilder<'a> {
    buffer: &'a mut [u8],
    offset: usize,
}

impl<'a> BlobBuilder<'a> {
    pub fn from_bytes(buffer: &'a mut [u8]) -> Self {
        Self { buffer, offset: 0 }
    }

    pub fn push_u32(&mut self, id: u32, data: u32) -> Result<(), UbusError> {
        self.push_bytes(id, &data.to_be_bytes())
    }

    pub fn push_bool(&mut self, id: u32, data: bool) -> Result<(), UbusError> {
        self.push_bytes(id, if data { &[1] } else { &[0] })
    }

    pub fn push_str(&mut self, id: u32, data: &str) -> Result<(), UbusError> {
        self.push_bytes(id, data.as_bytes().iter().chain([0u8].iter()))
    }

    pub fn push_bytes<'b>(
        &mut self,
        id: u32,
        data: impl IntoIterator<Item = &'b u8>,
    ) -> Result<(), UbusError> {
        let iter = data.into_iter();
        let buffer = &mut self.buffer[self.offset..];

        let mut offset = BlobTag::SIZE;
        for b in iter {
            if offset >= buffer.len() {
                return Err(UbusError::InvalidData("BlobBuilder overflow!"));
            }
            buffer[offset] = *b;
            offset += 1;
        }

        let tag = BlobTag::new(id, offset, false)?;
        let pad = tag.padding();
        buffer[..4].copy_from_slice(&tag.to_bytes());

        self.offset += offset + pad;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.offset
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Blob<'a> {
    pub tag: BlobTag,
    pub data: &'a [u8],
}

impl<'a> Blob<'a> {
    pub fn from_bytes(data: &'a [u8]) -> Result<Self, UbusError> {
        valid_data!(data.len() >= BlobTag::SIZE, "Blob too short");
        // Read the blob's tag
        let (tag, data) = data.split_at(BlobTag::SIZE);
        let tag = BlobTag::from_bytes(tag.try_into().unwrap());
        Self::from_tag_and_data(tag, data)
    }
    pub fn from_tag_and_data(tag: BlobTag, data: &'a [u8]) -> Result<Self, UbusError> {
        tag.is_valid()?;
        valid_data!(data.len() >= tag.inner_len(), "Blob too short");

        // Restrict data to payload size
        let data = &data[..tag.inner_len()];

        Ok(Blob { tag, data })
    }
}

impl<'a> TryInto<BlobMsg<'a>> for Blob<'a> {
    type Error = UbusError;
    fn try_into(self) -> Result<BlobMsg<'a>, Self::Error> {
        if !self.tag.is_extended() {
            return Err(UbusError::InvalidData("Not a extended blob"));
        }
        let (len_bytes, data) = self.data.split_at(size_of::<u16>());
        let name_len = u16::from_be_bytes(len_bytes.try_into().unwrap()) as usize;
        // Get the string
        if name_len > data.len() {
            //eprintln!("name_len:{}, data:{:?}", name_len, data);
            return Err(UbusError::InvalidData("name lenth > data lenth"));
        }
        let (name_bytes, data) = data.split_at(name_len);
        let name = str::from_utf8(name_bytes).unwrap();
        // Get the nul terminator (implicit)
        let name_len = name_len + 1;
        let (terminator, data) = data.split_at(1);
        valid_data!(terminator[0] == b'\0', "No extended name nul terminator");
        // Ensure the rest of the payload is aligned
        let name_total_len = size_of::<u16>() + name_len;
        let name_padding =
            BlobTag::ALIGNMENT.wrapping_sub(name_total_len) & (BlobTag::ALIGNMENT - 1);
        let payload = Payload::from(&data[name_padding..]);
        let data = match self.tag.id().into() {
            BlobMsgType::ARRAY => BlobMsgPayload::Array(payload.try_into()?),
            BlobMsgType::TABLE => BlobMsgPayload::Table(payload.try_into()?),
            BlobMsgType::STRING => BlobMsgPayload::String(payload.try_into()?),
            BlobMsgType::INT64 => BlobMsgPayload::Int64(payload.try_into()?),
            BlobMsgType::INT32 => BlobMsgPayload::Int32(payload.try_into()?),
            BlobMsgType::INT16 => BlobMsgPayload::Int16(payload.try_into()?),
            BlobMsgType::INT8 => BlobMsgPayload::Int8(payload.try_into()?),
            id => BlobMsgPayload::Unknown(id.value(), payload.into()),
        };
        Ok(BlobMsg { name, data })
    }
}
#[derive(Clone, Debug)]
pub struct Payload<'a>(&'a [u8]);
impl<'a> From<&'a [u8]> for Payload<'a> {
    fn from(value: &'a [u8]) -> Self {
        Payload(value)
    }
}

macro_rules! payload_try_into_number {
    ( $( $ty:ty , )* ) => { $( payload_try_into_number!($ty); )* };
    ( $ty:ty ) => {
        impl<'a> TryInto<$ty> for Payload<'a>{
            type Error = UbusError;
            fn try_into(self) -> Result<$ty, Self::Error> {
                let size = size_of::<$ty>();
                if let Ok(bytes) = self.0[..size].try_into() {
                    Ok(<$ty>::from_be_bytes(bytes))
                } else {
                    Err(UbusError::InvalidData(stringify!("Blob wrong size for " $ty)))
                }
            }
        }
    };
}
payload_try_into_number!(u8, i8, u16, i16, u32, i32, u64, i64, f64,);
impl<'a> TryInto<bool> for Payload<'a> {
    type Error = UbusError;
    fn try_into(self) -> Result<bool, Self::Error> {
        let value: u8 = self.0[0];
        Ok(value != 0)
    }
}

impl<'a> TryInto<&'a str> for Payload<'a> {
    type Error = UbusError;
    fn try_into(self) -> Result<&'a str, UbusError> {
        let data = if self.0.last() == Some(&b'\0') {
            &self.0[..self.0.len() - 1]
        } else {
            self.into()
        };
        str::from_utf8(data).map_err(UbusError::from)
    }
}

impl<'a> TryInto<Vec<BlobMsg<'a>>> for Payload<'a> {
    type Error = UbusError;
    fn try_into(self) -> Result<Vec<BlobMsg<'a>>, UbusError> {
        let iter = BlobIter::<Blob>::new(self.into());
        let mut list = Vec::new();
        for item in iter {
            list.push(item.try_into().map_err(UbusError::from)?);
        }
        Ok(list)
    }
}

impl<'a> TryInto<HashMap<&'a str, BlobMsgPayload<'a>>> for Payload<'a> {
    type Error = UbusError;
    fn try_into(self) -> Result<HashMap<&'a str, BlobMsgPayload<'a>>, UbusError> {
        let mut map = HashMap::<&str, BlobMsgPayload>::new();
        let iter = BlobIter::<Blob>::new(self.into());
        for item in iter {
            let item: BlobMsg = item.try_into()?;
            map.insert(item.name, item.data);
        }
        Ok(map)
    }
}

impl<'a> Into<&'a [u8]> for Payload<'a> {
    fn into(self) -> &'a [u8] {
        &self.0
    }
}

impl<'a, T> Into<BlobIter<'a, T>> for Blob<'a> {
    fn into(self) -> BlobIter<'a, T> {
        BlobIter::new(self.data.into())
    }
}

impl<'a, T> Into<BlobIter<'a, T>> for Payload<'a> {
    fn into(self) -> BlobIter<'a, T> {
        BlobIter::new(self.into())
    }
}

pub struct BlobIter<'a, T> {
    data: &'a [u8],
    _phantom: PhantomData<T>,
}
impl<'a, T> BlobIter<'a, T> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            _phantom: PhantomData,
        }
    }
}

impl<'a, T: TryFrom<Blob<'a>>> Iterator for BlobIter<'a, T> {
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }
        if let Ok(blob) = Blob::from_bytes(self.data) {
            // Advance the internal pointer to the next tag
            self.data = &self.data[blob.tag.next_tag()..];
            if let Ok(blob) = blob.try_into() {
                //println!("{:?}", self.data);
                return Some(blob);
            }
        }
        None
    }
}

impl<T> core::fmt::Debug for BlobIter<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "BlobIter")
    }
}

pub struct BlobMsgBuilder<'a> {
    buffer: Vec<u8>,
    _phantom: PhantomData<&'a mut [u8]>,
}

impl<'a> TryFrom<BlobMsg<'a>> for BlobMsgBuilder<'a> {
    type Error = UbusError;

    fn try_from(blobmsg: BlobMsg<'a>) -> Result<Self, Self::Error> {
        let name = blobmsg.name;
        let blob = match blobmsg.data {
            BlobMsgPayload::String(s) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::STRING.value(), name);
                blob.push_str(s)?;
                blob
            }
            BlobMsgPayload::Int64(num) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::INT64.value(), name);
                blob.push_int64(num)?;
                blob
            }
            BlobMsgPayload::Int32(num) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::INT32.value(), name);
                blob.push_int32(num)?;
                blob
            }
            BlobMsgPayload::Int16(num) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::INT16.value(), name);
                blob.push_int16(num)?;
                blob
            }
            BlobMsgPayload::Int8(num) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::INT8.value(), name);
                blob.push_int8(num)?;
                blob
            }
            BlobMsgPayload::Double(num) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::DOUBLE.value(), name);
                blob.push_double(num)?;
                blob
            }
            BlobMsgPayload::Bool(b) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::BOOL.value(), name);
                blob.push_int8(b)?;
                blob
            }
            BlobMsgPayload::Unknown(_typeid, _bytes) => {
                //println!("\"type={} data={:?}\"", typeid, bytes);
                unimplemented!()
            }
            BlobMsgPayload::Array(list) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::ARRAY.value(), name);
                for item in list {
                    let inner_blob = BlobMsgBuilder::try_from(item).unwrap();
                    blob.push_bytes(inner_blob.data())?;
                }
                blob
            }
            BlobMsgPayload::Table(table) => {
                let mut blob = BlobMsgBuilder::new_extended(BlobMsgType::TABLE.value(), name);
                for (name, data) in table {
                    let inner_blobmsg = BlobMsg {
                        name,
                        data,
                    };
                    let inner_blob = BlobMsgBuilder::try_from(inner_blobmsg).unwrap();
                    //let inner_blob = inner_blob.build();
                    blob.push_bytes(inner_blob.data())?;
                }
                blob
            }
        };
        Ok(blob)
    }
}

impl<'a> BlobMsgBuilder<'a> {
    pub fn from_bytes(bytes: &'a [u8]) -> Self {
        Self{ buffer: Vec::from(bytes), _phantom: PhantomData }
    }
    pub fn new_extended(id: u32, name: &str) -> Self {
        let buffer = Vec::new();
        let _phantom = PhantomData::<&mut [u8]>;
        let mut blob = Self {
            buffer,
            _phantom,
        };
        //blob.buffer.extend(&[0u8; BlobTag::SIZE]);
        let tag = BlobTag::new(id, BlobTag::SIZE, true).unwrap();
        blob.buffer.extend(tag.to_bytes());
        let len_bytes = u16::to_be_bytes(name.len() as u16);
        blob.buffer.extend(len_bytes);
        blob.buffer.extend_from_slice(name.as_bytes());
        blob.buffer.push(b'\0');
        let name_total_len = size_of::<u16>() + name.len() + 1;
        let name_padding =
            BlobTag::ALIGNMENT.wrapping_sub(name_total_len) & (BlobTag::ALIGNMENT - 1);
        blob.buffer.resize(blob.buffer.len() + name_padding, 0u8);
        let tag = BlobTag::new(id, blob.buffer.len(), true).unwrap();
        blob.buffer[..4].copy_from_slice(&tag.to_bytes());
        blob
    }

    pub fn tag(&self) -> BlobTag {
        let tag_bytes:[u8;BlobTag::SIZE] = self.buffer[..4].try_into().unwrap();
        BlobTag::from_bytes(tag_bytes)
    }

    pub fn push_bytes<'b>(&mut self, data: impl IntoIterator<Item = &'b u8>) -> Result<(), UbusError> {
        for b in data {
            self.buffer.push(*b);
        }
        let mut tag = self.tag();
        tag.set_size(self.buffer.len());
        self.buffer[..4].copy_from_slice(&tag.to_bytes());
        self.buffer.resize(self.buffer.len() + tag.padding(), 0u8);
        Ok(())
    }

    pub fn push_int64(&mut self, data: i64) -> Result<(), UbusError> {
        //self.id = BlobMsgType::INT64.value();
        self.push_bytes(&data.to_be_bytes())
    }

    pub fn push_int32(&mut self, data: i32) -> Result<(), UbusError> {
        //self.id = BlobMsgType::INT32.value();
        self.push_bytes(&data.to_be_bytes())
    }

    pub fn push_int16(&mut self, data: i16) -> Result<(), UbusError> {
        //self.id = BlobMsgType::INT16.value();
        self.push_bytes(&data.to_be_bytes())
    }

    pub fn push_int8(&mut self, data: i8) -> Result<(), UbusError> {
        //self.id = BlobMsgType::INT8.value();
        self.push_bytes(&data.to_be_bytes())
    }

    pub fn push_double(&mut self, data: f64) -> Result<(), UbusError> {
        //self.id = BlobMsgType::DOUBLE.value();
        self.push_bytes(&data.to_be_bytes())
    }

    pub fn push_bool(&mut self, data: bool) -> Result<(), UbusError> {
        //self.id = BlobMsgType::BOOL.value();
        let tf: i8 = if data { 1 } else { 0 };
        self.push_bytes(&tf.to_be_bytes())
    }

    pub fn push_str(&mut self, data: &str) -> Result<(), UbusError> {
        //self.id = BlobMsgType::STRING.value();
        self.push_bytes(data.as_bytes().iter().chain([0u8].iter()))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn data(&'a self) -> &'a [u8] {
        &self.buffer
    }

    pub fn build(&'a self) -> Blob<'a> {
        let data = self.data();
        let tag = BlobTag::from_bytes(self.buffer[..4].try_into().unwrap());
        let data = &data[4..];
        Blob { tag, data }
    }
}
