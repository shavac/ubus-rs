use alloc::vec::Vec;
use core::fmt;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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

impl BlobMsgType {
    /// Alias for `INT8` (`BLOBMSG_TYPE_BOOL` and `BLOBMSG_TYPE_INT8` share type ID 7 in libubox).
    pub const BOOL: Self = Self::INT8;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            BlobMsgPayload::Array(list) => {
                f.write_str("[")?;
                list.iter().enumerate().try_for_each(|(i, msg)| {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    fmt::Display::fmt(&msg.data, f)
                })?;
                f.write_str("]")
            }
            BlobMsgPayload::Table(dict) => {
                f.write_str("{")?;
                dict.iter().enumerate().try_for_each(|(i, (k, v))| {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "\"{k}\": {v}")
                })?;
                f.write_str("}")
            }
            BlobMsgPayload::String(s) => write!(f, "\"{s}\""),
            BlobMsgPayload::Int64(n) => fmt::Display::fmt(n, f),
            BlobMsgPayload::Int32(n) => fmt::Display::fmt(n, f),
            BlobMsgPayload::Int16(n) => fmt::Display::fmt(n, f),
            BlobMsgPayload::Int8(n) => fmt::Display::fmt(n, f),
            BlobMsgPayload::Bool(n) => f.write_str(if *n == 1 { "true" } else { "false" }),
            BlobMsgPayload::Double(n) => fmt::Display::fmt(n, f),
            BlobMsgPayload::Unknown(typeid, bytes) => {
                write!(f, "\"type={typeid} data={bytes:?}\"")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobMsg<'a> {
    pub name: &'a str,
    pub data: BlobMsgPayload<'a>,
}

impl fmt::Display for BlobMsg<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.name.is_empty() {
            return fmt::Display::fmt(&self.data, f);
        }
        write!(f, "\"{}\": {}", self.name, self.data)
    }
}
