use crate::{BlobMsg, BlobMsgPayload, BlobMsgType};

use super::Error;
use core::convert::{TryFrom, TryInto};
use core::marker::PhantomData;
use core::mem::{align_of, size_of, transmute};
use core::str;
use std::collections::HashMap;
use std::println;
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

    pub fn new(id: u32, len: usize) -> Result<Self, Error> {
        if id > Self::ID_MASK || len < Self::SIZE || len > Self::LEN_MASK as usize {
            Err(Error::InvalidData("Invalid TAG construction"))
        } else {
            let id = id & Self::ID_MASK;
            let len = len as u32 & Self::LEN_MASK;
            let val = len | (id << Self::ID_SHIFT);
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
    pub fn is_valid(&self) -> Result<(), Error> {
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

    pub fn push_u32(&mut self, id: u32, data: u32) -> Result<(), Error> {
        self.push_bytes(id, &data.to_be_bytes())
    }

    pub fn push_bool(&mut self, id: u32, data: bool) -> Result<(), Error> {
        self.push_bytes(id, if data { &[1] } else { &[0] })
    }

    pub fn push_str(&mut self, id: u32, data: &str) -> Result<(), Error> {
        self.push_bytes(id, data.as_bytes().iter().chain([0u8].iter()))
    }

    pub fn push_bytes<'b>(
        &mut self,
        id: u32,
        data: impl IntoIterator<Item = &'b u8>,
    ) -> Result<(), Error> {
        let iter = data.into_iter();
        let buffer = &mut self.buffer[self.offset..];

        let mut len = BlobTag::SIZE;
        for b in iter {
            if len >= buffer.len() {
                return Err(Error::InvalidData("BlobBuilder overflow!"));
            }
            buffer[len] = *b;
            len += 1;
        }

        let tag = BlobTag::new(id, len)?;
        let pad = tag.padding();
        buffer[..4].copy_from_slice(&tag.to_bytes());

        self.offset += len + pad;

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
    //pub name: Option<&'a str>,
}

impl<'a> Blob<'a> {
    pub fn from_bytes(data: &'a [u8]) -> Result<Self, Error> {
        valid_data!(data.len() >= BlobTag::SIZE, "Blob too short");
        // Read the blob's tag
        let (tag, data) = data.split_at(BlobTag::SIZE);
        let tag = BlobTag::from_bytes(tag.try_into().unwrap());
        Self::from_tag_and_data(tag, data)
    }
    pub fn from_tag_and_data(tag: BlobTag, data: &'a [u8]) -> Result<Self, Error> {
        tag.is_valid()?;
        valid_data!(data.len() >= tag.inner_len(), "Blob too short");

        // Restrict data to payload size
        let data = &data[..tag.inner_len()];

        Ok(Blob { tag, data })
    }
}

impl<'a> TryInto<BlobMsg<'a>> for Blob<'a> {
    type Error = Error;
    fn try_into(self) -> Result<BlobMsg<'a>, Self::Error> {
        if !self.tag.is_extended() {
            return Err(Error::InvalidData("Not a extended blob"));
        }
        let (len_bytes, data) = self.data.split_at(size_of::<u16>());
        let name_len = u16::from_be_bytes(len_bytes.try_into().unwrap()) as usize;
        // Get the string
        let (name_bytes, data) = data.split_at(name_len);
        let name = str::from_utf8(name_bytes).unwrap();
        // Get the nul terminator (implicit)
        let name_len = name_len + 1;
        let (terminator, data) = data.split_at(1);
        valid_data!(terminator[0] == b'\0', "No extended name nul terminator");
        // Ensure the rest of the payload is aligned
        let name_total_len = size_of::<u16>() + name_len;
        let padding = BlobTag::ALIGNMENT.wrapping_sub(name_total_len) & (BlobTag::ALIGNMENT - 1);
        let payload = Payload(&data[padding..]);
        let data = match self.tag.id().into() {
            BlobMsgType::ARRAY => BlobMsgPayload::Array(payload.try_into()?),
            BlobMsgType::TABLE => BlobMsgPayload::Table(payload.try_into()?),
            BlobMsgType::STRING => BlobMsgPayload::String(payload.try_into()?),
            BlobMsgType::INT64 => BlobMsgPayload::Int64(payload.try_into()?),
            BlobMsgType::INT32 => BlobMsgPayload::Int32(payload.try_into()?),
            BlobMsgType::INT16 => BlobMsgPayload::Int16(payload.try_into()?),
            BlobMsgType::INT8 => BlobMsgPayload::Int8(payload.try_into()?),
            id => BlobMsgPayload::Unknown(id.value(), payload.0),
        };
        let name = Some(name);
        Ok(BlobMsg { name, data })
    }
}

pub struct Payload<'a>(pub &'a [u8]);
macro_rules! payload_try_into_number {
    ( $( $ty:ty , )* ) => { $( payload_try_into_number!($ty); )* };
    ( $ty:ty ) => {
        impl TryInto<$ty> for Payload<'_> {
            type Error = Error;
            fn try_into(self) -> Result<$ty, Self::Error> {
                let size = size_of::<$ty>();
                if let Ok(bytes) = self.0[..size].try_into() {
                    Ok(<$ty>::from_be_bytes(bytes))
                } else {
                    Err(Error::InvalidData(stringify!("Blob wrong size for " $ty)))
                }
            }
        }
    };
}
payload_try_into_number!(u8, i8, u16, i16, u32, i32, u64, i64, f64,);
impl<'a> TryInto<bool> for Payload<'a> {
    type Error = Error;
    fn try_into(self) -> Result<bool, Self::Error> {
        let value: u8 = self.0[0];
        Ok(value != 0)
    }
}

impl<'a> TryInto<&'a str> for Payload<'a> {
    type Error = Error;
    fn try_into(self) -> Result<&'a str, Self::Error> {
        let data = if self.0.last() == Some(&b'\0') {
            &self.0[..self.0.len() - 1]
        } else {
            self.0
        };
        str::from_utf8(data).map_err(|_| Error::InvalidData("Blob not valid UTF-8"))
    }
}
impl<'a> TryInto<Vec<BlobMsg<'a>>> for Payload<'a> {
    type Error = Error;
    fn try_into(self) -> Result<Vec<BlobMsg<'a>>, Error> {
        let iter = BlobIter::<Blob>::new(self.0);
        let mut list = Vec::new();
        for item in iter {
            list.push(item.try_into()?);
        }
        Ok(list)
    }
}

impl<'a> TryInto<HashMap<&'a str, BlobMsgPayload<'a>>> for Payload<'a> {
    type Error = Error;
    fn try_into(self) -> Result<HashMap<&'a str, BlobMsgPayload<'a>>, Error> {
        let mut map = HashMap::<&str, BlobMsgPayload>::new();
        let iter = BlobIter::<Blob>::new(self.0);
        for item in iter {
            let item: BlobMsg = item.try_into()?;
            map.insert(item.name.unwrap_or_default(), item.data);
        }
        Ok(map)
    }
}

impl<'a> Into<&'a [u8]> for Payload<'a> {
    fn into(self) -> &'a [u8] {
        self.0
    }
}
impl<'a, T> Into<BlobIter<'a, T>> for Blob<'a> {
    fn into(self) -> BlobIter<'a, T> {
        BlobIter::new(self.data)
    }
}

impl<'a, T> Into<BlobIter<'a, T>> for Payload<'a> {
    fn into(self) -> BlobIter<'a, T> {
        BlobIter::new(self.0)
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
