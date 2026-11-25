use crate::{BlobMsg, BlobMsgPayload, BlobMsgType, UbusError};

use alloc::vec::Vec;
use core::convert::{TryFrom, TryInto};
use core::marker::PhantomData;
use core::mem::{align_of, size_of, transmute};
use core::str;
use std::collections::HashMap;
use storage_endian::BEu32;

#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct BlobTag(BEu32);
impl BlobTag {
    pub const SIZE: usize = size_of::<Self>();
    const ID_MASK: u32 = 0x7f;
    const ID_SHIFT: u32 = 24;
    const LEN_MASK: u32 = 0xff_ff_ff;
    const EXTENDED_BIT: u32 = 1 << 31;
    pub(crate) const ALIGNMENT: usize = align_of::<Self>();

    #[inline]
    pub const fn align_padding(len: usize) -> usize {
        Self::ALIGNMENT.wrapping_sub(len) & (Self::ALIGNMENT - 1)
    }

    pub fn new(id: u32, len: usize, extended: bool) -> Result<Self, UbusError> {
        if id > Self::ID_MASK || len < Self::SIZE || len > Self::LEN_MASK as usize {
            Err(UbusError::InvalidData("Invalid TAG construction"))
        } else {
            let mut val = (len as u32) | (id << Self::ID_SHIFT);
            if extended {
                val |= Self::EXTENDED_BIT;
            }
            Ok(Self(val.into()))
        }
    }

    /// Create BlobTag from a byte array
    #[inline]
    pub fn from_bytes(bytes: [u8; Self::SIZE]) -> Self {
        unsafe { transmute(bytes) }
    }
    /// Dump out bytes of BlobTag
    #[inline]
    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        unsafe { transmute(self) }
    }
    #[inline]
    fn raw(self) -> u32 {
        u32::from(self.0)
    }
    /// ID code of this blob
    #[inline]
    pub fn id(&self) -> u32 {
        (self.raw() >> Self::ID_SHIFT) & Self::ID_MASK
    }
    /// Total number of bytes this blob contains (header + data)
    #[inline]
    pub fn size(&self) -> usize {
        (self.raw() & Self::LEN_MASK) as usize
    }

    pub fn set_size(&mut self, size: usize) {
        *self = Self::new(self.id(), size, self.is_extended()).unwrap();
    }
    /// Number of padding bytes between this blob and the next blob
    #[inline]
    pub(crate) fn padding(&self) -> usize {
        Self::align_padding(self.size())
    }
    /// Number of bytes to the next tag
    #[inline]
    pub(crate) fn next_tag(&self) -> usize {
        self.size() + self.padding()
    }
    /// Total number of bytes following the tag (extended header + data)
    #[inline]
    pub fn inner_len(&self) -> usize {
        self.size().saturating_sub(Self::SIZE)
    }
    /// Is this an "extended" blob
    #[inline]
    pub fn is_extended(&self) -> bool {
        (self.raw() & Self::EXTENDED_BIT) != 0
    }
    /// Does this blob look valid
    #[inline]
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
        self.push_slice(id, &data.to_be_bytes())
    }

    pub fn push_bool(&mut self, id: u32, data: bool) -> Result<(), UbusError> {
        self.push_slice(id, if data { &[1] } else { &[0] })
    }

    pub fn push_str(&mut self, id: u32, data: &str) -> Result<(), UbusError> {
        let str_bytes = data.as_bytes();
        let unpadded = BlobTag::SIZE + str_bytes.len() + 1;
        let tag = BlobTag::new(id, unpadded, false)?;
        let total = tag.next_tag();
        let buffer = &mut self.buffer[self.offset..];
        if total > buffer.len() {
            return Err(UbusError::InvalidData("BlobBuilder overflow!"));
        }
        buffer[..BlobTag::SIZE].copy_from_slice(&tag.to_bytes());
        buffer[BlobTag::SIZE..BlobTag::SIZE + str_bytes.len()].copy_from_slice(str_bytes);
        buffer[BlobTag::SIZE + str_bytes.len()..total].fill(0);
        self.offset += total;
        Ok(())
    }

    /// Push a raw byte slice into the blob without iterator overhead.
    pub fn push_slice(&mut self, id: u32, data: &[u8]) -> Result<(), UbusError> {
        let unpadded = BlobTag::SIZE + data.len();
        let tag = BlobTag::new(id, unpadded, false)?;
        let total = tag.next_tag();
        let buffer = &mut self.buffer[self.offset..];
        if total > buffer.len() {
            return Err(UbusError::InvalidData("BlobBuilder overflow!"));
        }
        buffer[..BlobTag::SIZE].copy_from_slice(&tag.to_bytes());
        buffer[BlobTag::SIZE..unpadded].copy_from_slice(data);
        buffer[unpadded..total].fill(0);
        self.offset += total;
        Ok(())
    }

    pub fn push_bytes<'b>(
        &mut self,
        id: u32,
        data: impl IntoIterator<Item = &'b u8>,
    ) -> Result<(), UbusError> {
        let buffer = &mut self.buffer[self.offset..];
        let mut offset = BlobTag::SIZE;
        for b in data {
            if offset >= buffer.len() {
                return Err(UbusError::InvalidData("BlobBuilder overflow!"));
            }
            buffer[offset] = *b;
            offset += 1;
        }

        let tag = BlobTag::new(id, offset, false)?;
        let total = tag.next_tag();
        if total > buffer.len() {
            return Err(UbusError::InvalidData("BlobBuilder overflow!"));
        }
        buffer[..BlobTag::SIZE].copy_from_slice(&tag.to_bytes());
        buffer[offset..total].fill(0);

        self.offset += total;
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
        let (tag, data) = data.split_at(BlobTag::SIZE);
        let tag = BlobTag::from_bytes(tag.try_into().unwrap());
        Self::from_tag_and_data(tag, data)
    }

    pub fn from_tag_and_data(tag: BlobTag, data: &'a [u8]) -> Result<Self, UbusError> {
        tag.is_valid()?;
        valid_data!(data.len() >= tag.inner_len(), "Blob too short");
        let data = &data[..tag.inner_len()];
        Ok(Blob { tag, data })
    }

    /// Extract the extended blob's name and inner payload directly from the buffer (zero heap allocations).
    pub fn into_named_payload(self) -> Result<(&'a str, Payload<'a>), UbusError> {
        if !self.tag.is_extended() {
            return Err(UbusError::InvalidData("Not an extended blob"));
        }
        valid_data!(self.data.len() >= size_of::<u16>(), "Extended blob too short");
        let (len_bytes, data) = self.data.split_at(size_of::<u16>());
        let name_len = u16::from_be_bytes(len_bytes.try_into().unwrap()) as usize;
        if name_len >= data.len() {
            return Err(UbusError::InvalidData("name length >= data length"));
        }
        let (name_bytes, data) = data.split_at(name_len);
        let name = str::from_utf8(name_bytes)?;
        let (terminator, data) = data.split_at(1);
        valid_data!(terminator[0] == b'\0', "No extended name nul terminator");
        let name_total_len = size_of::<u16>() + name_len + 1;
        let name_padding = BlobTag::align_padding(name_total_len);
        valid_data!(
            data.len() >= name_padding,
            "Extended name padding out of bounds"
        );
        Ok((name, Payload::from(&data[name_padding..])))
    }
}

impl<'a> TryFrom<Blob<'a>> for BlobMsg<'a> {
    type Error = UbusError;
    fn try_from(blob: Blob<'a>) -> Result<BlobMsg<'a>, Self::Error> {
        let (name, payload) = blob.into_named_payload()?;
        let data = match blob.tag.id().into() {
            BlobMsgType::ARRAY => BlobMsgPayload::Array(payload.try_into()?),
            BlobMsgType::TABLE => BlobMsgPayload::Table(payload.try_into()?),
            BlobMsgType::STRING => BlobMsgPayload::String(payload.try_into()?),
            BlobMsgType::INT64 => BlobMsgPayload::Int64(payload.try_into()?),
            BlobMsgType::INT32 => BlobMsgPayload::Int32(payload.try_into()?),
            BlobMsgType::INT16 => BlobMsgPayload::Int16(payload.try_into()?),
            BlobMsgType::INT8 => BlobMsgPayload::Int8(payload.try_into()?),
            BlobMsgType::DOUBLE => BlobMsgPayload::Double(payload.try_into()?),
            id => BlobMsgPayload::Unknown(id.value(), payload.into()),
        };
        Ok(BlobMsg { name, data })
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Payload<'a>(&'a [u8]);
impl<'a> From<&'a [u8]> for Payload<'a> {
    fn from(value: &'a [u8]) -> Self {
        Payload(value)
    }
}

macro_rules! payload_try_into_number {
    ( $( $ty:ty , )* ) => { $( payload_try_into_number!($ty); )* };
    ( $ty:ty ) => {
        impl<'a> TryInto<$ty> for Payload<'a> {
            type Error = UbusError;
            fn try_into(self) -> Result<$ty, Self::Error> {
                let size = size_of::<$ty>();
                if let Some(Ok(bytes)) = self.0.get(..size).map(TryInto::try_into) {
                    Ok(<$ty>::from_be_bytes(bytes))
                } else {
                    Err(UbusError::InvalidData(concat!("Blob wrong size for ", stringify!($ty))))
                }
            }
        }
    };
}
payload_try_into_number!(u8, i8, u16, i16, u32, i32, u64, i64, f64,);

impl<'a> TryInto<bool> for Payload<'a> {
    type Error = UbusError;
    fn try_into(self) -> Result<bool, Self::Error> {
        let &value = self
            .0
            .first()
            .ok_or(UbusError::InvalidData("Blob wrong size for bool"))?;
        Ok(value != 0)
    }
}

impl<'a> TryInto<&'a str> for Payload<'a> {
    type Error = UbusError;
    fn try_into(self) -> Result<&'a str, UbusError> {
        let data = match self.0.strip_suffix(b"\0") {
            Some(stripped) => stripped,
            None => self.0,
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
            list.push(item.try_into()?);
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

impl<'a> From<Payload<'a>> for &'a [u8] {
    fn from(val: Payload<'a>) -> Self {
        val.0
    }
}

impl<'a, T> From<Blob<'a>> for BlobIter<'a, T> {
    fn from(val: Blob<'a>) -> Self {
        BlobIter::new(val.data)
    }
}

impl<'a, T> From<Payload<'a>> for BlobIter<'a, T> {
    fn from(val: Payload<'a>) -> Self {
        BlobIter::new(val.into())
    }
}

#[derive(Copy, Clone)]
pub struct BlobIter<'a, T> {
    data: &'a [u8],
    _phantom: PhantomData<T>,
}
impl<'a, T> BlobIter<'a, T> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            _phantom: PhantomData,
        }
    }

    pub const fn as_bytes(&self) -> &'a [u8] {
        self.data
    }
}

impl<'a, T: TryFrom<Blob<'a>>> Iterator for BlobIter<'a, T> {
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }
        if let Ok(blob) = Blob::from_bytes(self.data) {
            // Advance the internal pointer to the next tag safely
            let next = blob.tag.next_tag().min(self.data.len());
            self.data = &self.data[next..];
            if let Ok(item) = blob.try_into() {
                return Some(item);
            }
        } else {
            self.data = &[];
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
        let mut buffer = Vec::with_capacity(32);
        Self::write_blobmsg_into(&mut buffer, &blobmsg)?;
        Ok(Self {
            buffer,
            _phantom: PhantomData,
        })
    }
}

impl<'a> BlobMsgBuilder<'a> {
    pub fn from_bytes(bytes: &'a [u8]) -> Self {
        Self {
            buffer: Vec::from(bytes),
            _phantom: PhantomData,
        }
    }

    /// Write an extended blob header at the end of `buffer` and return the byte offset of its `BlobTag`.
    pub(crate) fn begin_extended_in(buffer: &mut Vec<u8>, id: u32, name: &str) -> Result<usize, UbusError> {
        let start = buffer.len();
        let name_bytes = name.as_bytes();
        if name_bytes.len() > u16::MAX as usize {
            return Err(UbusError::InvalidData("Extended name too long"));
        }
        let name_total_len = size_of::<u16>() + name_bytes.len() + 1;
        let name_padding = BlobTag::align_padding(name_total_len);
        let header_len = BlobTag::SIZE + name_total_len + name_padding;

        buffer.reserve(header_len + 8);
        let tag = BlobTag::new(id, header_len, true)?;
        buffer.extend_from_slice(&tag.to_bytes());
        buffer.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
        buffer.extend_from_slice(name_bytes);
        buffer.push(b'\0');
        buffer.resize(start + header_len, 0u8);
        Ok(start)
    }

    /// Finish an extended blob started at `start` in `buffer`: update its tag size and pad to 4-byte alignment.
    pub(crate) fn end_extended_in(buffer: &mut Vec<u8>, start: usize, id: u32) -> Result<(), UbusError> {
        let unpadded_len = buffer.len() - start;
        let tag = BlobTag::new(id, unpadded_len, true)?;
        buffer[start..start + BlobTag::SIZE].copy_from_slice(&tag.to_bytes());
        let pad = tag.padding();
        if pad > 0 {
            buffer.resize(buffer.len() + pad, 0u8);
        }
        Ok(())
    }

    /// Serialize a `BlobMsg` tree directly into a single `Vec<u8>` without intermediate `Vec` allocations.
    pub fn write_blobmsg_into(buffer: &mut Vec<u8>, blobmsg: &BlobMsg<'_>) -> Result<(), UbusError> {
        let name = blobmsg.name;
        match &blobmsg.data {
            BlobMsgPayload::String(s) => {
                let id = BlobMsgType::STRING.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                buffer.extend_from_slice(s.as_bytes());
                buffer.push(b'\0');
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Int64(num) => {
                let id = BlobMsgType::INT64.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                buffer.extend_from_slice(&num.to_be_bytes());
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Int32(num) => {
                let id = BlobMsgType::INT32.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                buffer.extend_from_slice(&num.to_be_bytes());
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Int16(num) => {
                let id = BlobMsgType::INT16.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                buffer.extend_from_slice(&num.to_be_bytes());
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Int8(num) => {
                let id = BlobMsgType::INT8.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                buffer.extend_from_slice(&num.to_be_bytes());
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Double(num) => {
                let id = BlobMsgType::DOUBLE.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                buffer.extend_from_slice(&num.to_be_bytes());
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Bool(b) => {
                let id = BlobMsgType::BOOL.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                buffer.extend_from_slice(&b.to_be_bytes());
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Unknown(typeid, bytes) => {
                let start = Self::begin_extended_in(buffer, *typeid, name)?;
                buffer.extend_from_slice(bytes);
                Self::end_extended_in(buffer, start, *typeid)?;
            }
            BlobMsgPayload::Array(list) => {
                let id = BlobMsgType::ARRAY.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                for item in list {
                    Self::write_blobmsg_into(buffer, item)?;
                }
                Self::end_extended_in(buffer, start, id)?;
            }
            BlobMsgPayload::Table(table) => {
                let id = BlobMsgType::TABLE.value();
                let start = Self::begin_extended_in(buffer, id, name)?;
                for (&k, v) in table {
                    let inner = BlobMsg {
                        name: k,
                        data: v.clone(),
                    };
                    Self::write_blobmsg_into(buffer, &inner)?;
                }
                Self::end_extended_in(buffer, start, id)?;
            }
        }
        Ok(())
    }

    pub fn new_extended(id: u32, name: &str) -> Self {
        let mut buffer = Vec::new();
        Self::begin_extended_in(&mut buffer, id, name).unwrap();
        Self {
            buffer,
            _phantom: PhantomData,
        }
    }

    pub fn tag(&self) -> BlobTag {
        let tag_bytes: [u8; BlobTag::SIZE] = self.buffer[..4].try_into().unwrap();
        BlobTag::from_bytes(tag_bytes)
    }

    pub fn push_slice(&mut self, data: &[u8]) -> Result<(), UbusError> {
        self.buffer.extend_from_slice(data);
        let id = self.tag().id();
        Self::end_extended_in(&mut self.buffer, 0, id)
    }

    pub fn push_bytes<'b>(
        &mut self,
        data: impl IntoIterator<Item = &'b u8>,
    ) -> Result<(), UbusError> {
        self.buffer.extend(data.into_iter().copied());
        let id = self.tag().id();
        Self::end_extended_in(&mut self.buffer, 0, id)
    }

    pub fn push_int64(&mut self, data: i64) -> Result<(), UbusError> {
        self.push_slice(&data.to_be_bytes())
    }

    pub fn push_int32(&mut self, data: i32) -> Result<(), UbusError> {
        self.push_slice(&data.to_be_bytes())
    }

    pub fn push_int16(&mut self, data: i16) -> Result<(), UbusError> {
        self.push_slice(&data.to_be_bytes())
    }

    pub fn push_int8(&mut self, data: i8) -> Result<(), UbusError> {
        self.push_slice(&data.to_be_bytes())
    }

    pub fn push_double(&mut self, data: f64) -> Result<(), UbusError> {
        self.push_slice(&data.to_be_bytes())
    }

    pub fn push_bool(&mut self, data: bool) -> Result<(), UbusError> {
        let tf: i8 = if data { 1 } else { 0 };
        self.push_slice(&tf.to_be_bytes())
    }

    pub fn push_str(&mut self, data: &str) -> Result<(), UbusError> {
        self.buffer.reserve(data.len() + 4);
        self.buffer.extend_from_slice(data.as_bytes());
        self.buffer.push(b'\0');
        let id = self.tag().id();
        Self::end_extended_in(&mut self.buffer, 0, id)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn data(&self) -> &[u8] {
        &self.buffer
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buffer
    }

    pub fn build(&self) -> Blob<'_> {
        let data = self.data();
        let tag = BlobTag::from_bytes(self.buffer[..4].try_into().unwrap());
        let data = &data[4..];
        Blob { tag, data }
    }
}
