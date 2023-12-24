use super::{Blob, Error};
use core::convert::{TryFrom, TryInto};
use core::str;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::format;
use std::vec::Vec;

values!(pub BlobMsgType(u32) {
    UNSPEC = 0,
    ARRAY  = 1,
    TABLE  = 2,
    STRING = 3,
    INT64  = 4,
    INT32  = 5,
    INT16  = 6,
    INT8   = 7,
    DOUBLE = 8,
});

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum BlobMsgData<'a> {
    Array(Vec<BlobMsg<'a>>),
    Table(HashMap<&'a str, BlobMsgData<'a>>),
    String(&'a str),
    Int64(i64),
    Int32(i32),
    Int16(i16),
    Int8(i8),
    Double(f64),
    Unknown(u32, &'a [u8]),
}

impl fmt::Display for BlobMsgData<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobMsgData::Array(list) => write!(f, "{}", List(list)),
            BlobMsgData::Table(dict) => write!(f, "{}", Dict(dict)),
            BlobMsgData::String(s) => write!(f, "\"{}\"", s),
            BlobMsgData::Int64(num) => write!(f, "{}", num),
            BlobMsgData::Int32(num) => write!(f, "{}", num),
            BlobMsgData::Int16(num) => write!(f, "{}", num),
            BlobMsgData::Double(num) => write!(f, "{}", num),
            BlobMsgData::Int8(num) => write!(f, "{}", *num == 1),
            BlobMsgData::Unknown(typeid, bytes) => {
                write!(f, "Unknown: type={} data={:?}", typeid, bytes)
            }
        }
    }
}

/* impl fmt::Display for Vec<BlobMsg<'_>> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let formatted = self.iter().map(|x| format!("{}", x)).iterspace(",").collect();
        write!(f, "[{}]", formatted)
    }
}
 */
struct List<'a>(&'a Vec<BlobMsg<'a>>);
impl<'a> fmt::Display for List<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        let mut first = true;
        for msg in self.0 {
            if first {
                first = false;
            } else {
                write!(f, ", ")?;
            }
            write!(f, "{}", msg)?; // Use the Display implementation of BlobMsg
        }
        write!(f, "]")?;
        Ok(())
    }
}

struct Dict<'a>(&'a HashMap<&'a str, BlobMsgData<'a>>);
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
            write!(f, "\"{}\": {}", k, v)?; // Use the Display implementation of BlobMsg
        }
        write!(f, "}}")?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
//#[serde(serialize_with = "serialize_key_and_value")]
pub struct BlobMsg<'a> {
    pub name: Option<&'a str>,
    pub data: BlobMsgData<'a>,
}

/* impl<'a> Serialize for BlobMsg<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let key = self.name.unwrap_or_default();
        let value = format!("{:?}", self.data);
        let mut s = serializer.serialize_struct("", 1)?;
        s.serialize_field(key, value.as_str().clone())?;
        s.end()
    }
}
 */
impl<'a> TryFrom<Blob<'a>> for BlobMsg<'a> {
    type Error = Error;
    fn try_from(blob: Blob<'a>) -> Result<Self, Self::Error> {
        let data = match blob.tag.id().into() {
            BlobMsgType::ARRAY => BlobMsgData::Array(blob.try_into()?),
            BlobMsgType::TABLE => BlobMsgData::Table(blob.try_into()?),
            BlobMsgType::STRING => BlobMsgData::String(blob.try_into()?),
            BlobMsgType::INT64 => BlobMsgData::Int64(blob.try_into()?),
            BlobMsgType::INT32 => BlobMsgData::Int32(blob.try_into()?),
            BlobMsgType::INT16 => BlobMsgData::Int16(blob.try_into()?),
            BlobMsgType::INT8 => BlobMsgData::Int8(blob.try_into()?),
            id => BlobMsgData::Unknown(id.value(), blob.data),
        };
        Ok(BlobMsg {
            name: blob.name,
            data,
        })
    }
}

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

impl Serialize for BlobMsgData<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("BlobMsgData", 1)?;
        s.serialize_field("name", &self)?;
        s.end()
    }
}

impl<'de> Serialize for BlobMsg<'de> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("BlobMsg", 2)?;
        let name = self.name.unwrap_or_default();
        /*                  let data = self.data;
        let value = format!("{:?}", data).as_str().clone(); */
        s.serialize_field("abc", &self.data)?;
        s.end()
    }
}
