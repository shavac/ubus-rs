use crate::{BlobTag};

use super::{Blob, Error};
use core::convert::{TryFrom, TryInto};
use core::str;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

values!(pub BlobMsgType(u32) {
    UNSPEC = 0,
    ARRAY  = 1,
    TABLE  = 2,
    STRING = 3,
    INT64  = 4,
    INT32  = 5,
    INT16  = 6,
    BOOL   = 7,
    INT8   = 7,
    DOUBLE = 8,
});

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum BlobMsgPayload<'a> {
    Array(Vec<BlobMsg<'a>>),
    Table(HashMap<&'a str, BlobMsgPayload<'a>>),
    String(&'a str),
    Int64(i64),
    Int32(i32),
    Int16(i16),
    Int8(i8),
    Bool(i8),
    Double(f64),
    Unknown(u32, &'a [u8]),
}

impl fmt::Display for BlobMsgPayload<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobMsgPayload::Array(list) => write!(f, "{}", List(list)),
            BlobMsgPayload::Table(dict) => write!(f, "{}", Dict(dict)),
            BlobMsgPayload::String(s) => write!(f, "\"{}\"", s),
            BlobMsgPayload::Int64(num) => write!(f, "{}", num),
            BlobMsgPayload::Int32(num) => write!(f, "{}", num),
            BlobMsgPayload::Int16(num) => write!(f, "{}", num),
            BlobMsgPayload::Int8(num) => write!(f, "{}", num),
            BlobMsgPayload::Bool(num) => write!(f, "{}", *num == 1),
            BlobMsgPayload::Double(num) => write!(f, "{}", num),
            BlobMsgPayload::Unknown(typeid, bytes) => {
                write!(f, "\"type={} data={:?}\"", typeid, bytes)
            }
        }
    }
}

struct List<'a>(&'a Vec<BlobMsg<'a>>);
impl<'a> fmt::Display for List<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        let mut first = true;
        for msg in self.0 {
            if !first {
                write!(f, ", ")?;
            } else {
                first = false;
            }
            write!(f, "{}", msg)?; // Use the Display implementation of BlobMsg
        }
        write!(f, "]")?;
        Ok(())
    }
}

struct Dict<'a>(&'a HashMap<&'a str, BlobMsgPayload<'a>>);
impl<'a> fmt::Display for Dict<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        let mut first = true;
        for (k, v) in self.0 {
            if first {
                first = false;
            } else {
                write!(f, ", ")?;
            }
            write!(f, "\"{}\": {}", k, v)?;
        }
        write!(f, "}}")?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct BlobMsg<'a> {
    pub name: Option<&'a str>,
    pub data: BlobMsgPayload<'a>,
}

/* impl<'a> TryFrom<Blob<'a>> for BlobMsg<'a> {
    type Error = Error;
    fn try_from(blob: Blob<'a>) -> Result<Self, Self::Error> {
        let data = match blob.tag.id().into() {
            BlobMsgType::ARRAY => BlobMsgPayload::Array(blob.try_into()?),
            BlobMsgType::TABLE => BlobMsgPayload::Table(blob.try_into()?),
            BlobMsgType::STRING => BlobMsgPayload::String(blob.try_into()?),
            BlobMsgType::INT64 => BlobMsgPayload::Int64(blob.try_into()?),
            BlobMsgType::INT32 => BlobMsgPayload::Int32(blob.try_into()?),
            BlobMsgType::INT16 => BlobMsgPayload::Int16(blob.try_into()?),
            BlobMsgType::INT8 => BlobMsgPayload::Int8(blob.try_into()?),
            id => BlobMsgPayload::Unknown(id.value(), blob.payload),
        };
        Ok(BlobMsg {
            name: blob.name,
            data,
        })
    }
} */

impl fmt::Display for BlobMsg<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.name.unwrap_or_default();
        if name.len() > 0 {
            write!(f, "\"{}\": {}", name, self.data)
        } else {
            write!(f, "{}", self.data)
        }
    }
}
