use crate::{Blob, BlobIter, BlobMsgType, BlobTag, Payload, UbusError};
use core::convert::TryInto;
use serde::de::value::{BorrowedStrDeserializer, MapAccessDeserializer};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Error as SerError, Serialize, SerializeMap, SerializeSeq, Serializer};

#[inline]
fn next_blob<'a>(data: &mut &'a [u8]) -> Result<Option<Blob<'a>>, UbusError> {
    if data.is_empty() {
        return Ok(None);
    }
    let blob = Blob::from_bytes(data)?;
    let next = blob.tag.next_tag().min(data.len());
    *data = &data[next..];
    Ok(Some(blob))
}

/// Zero-copy wrapper over a sequence of extended blobs representing a JSON/map object.
#[derive(Copy, Clone, Debug)]
pub struct BlobTable<'a>(pub &'a [u8]);

/// Zero-copy wrapper over a single extended blob's tag and payload.
#[derive(Copy, Clone, Debug)]
pub struct BlobValue<'a> {
    tag: BlobTag,
    payload: Payload<'a>,
}

impl<'a> BlobValue<'a> {
    /// Extract a `BlobValue` from an extended `Blob`.
    pub fn from_blob(blob: Blob<'a>) -> Result<Self, UbusError> {
        let (_, payload) = blob.into_named_payload()?;
        Ok(Self {
            tag: blob.tag,
            payload,
        })
    }
}

impl Serialize for BlobTable<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let mut data = self.0;
        while let Some(blob) = next_blob(&mut data).map_err(S::Error::custom)? {
            let (name, payload) = blob.into_named_payload().map_err(S::Error::custom)?;
            map.serialize_entry(
                name,
                &BlobValue {
                    tag: blob.tag,
                    payload,
                },
            )?;
        }
        map.end()
    }
}

impl Serialize for BlobValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::ARRAY => {
                let mut seq = serializer.serialize_seq(None)?;
                let mut data: &[u8] = self.payload.into();
                while let Some(item) = next_blob(&mut data).map_err(S::Error::custom)? {
                    let (_, item_payload) = item.into_named_payload().map_err(S::Error::custom)?;
                    seq.serialize_element(&BlobValue {
                        tag: item.tag,
                        payload: item_payload,
                    })?;
                }
                seq.end()
            }
            BlobMsgType::TABLE => BlobTable(self.payload.into()).serialize(serializer),
            BlobMsgType::STRING => {
                let s: &str = self.payload.try_into().map_err(S::Error::custom)?;
                serializer.serialize_str(s)
            }
            BlobMsgType::INT64 => {
                let n: i64 = self.payload.try_into().map_err(S::Error::custom)?;
                serializer.serialize_i64(n)
            }
            BlobMsgType::INT32 => {
                let n: i32 = self.payload.try_into().map_err(S::Error::custom)?;
                serializer.serialize_i32(n)
            }
            BlobMsgType::INT16 => {
                let n: i16 = self.payload.try_into().map_err(S::Error::custom)?;
                serializer.serialize_i16(n)
            }
            BlobMsgType::BOOL => {
                let b: bool = self.payload.try_into().map_err(S::Error::custom)?;
                serializer.serialize_bool(b)
            }
            BlobMsgType::DOUBLE => {
                let n: f64 = self.payload.try_into().map_err(S::Error::custom)?;
                serializer.serialize_f64(n)
            }
            _ => serializer.serialize_bytes(self.payload.into()),
        }
    }
}

/// Deserialize a sequence of extended blobs directly into a typed Rust struct without JSON.
pub fn from_blob<'de, T: Deserialize<'de>>(data: &'de [u8]) -> Result<T, UbusError> {
    T::deserialize(BlobTableDeserializer { data })
}

/// Serde `Deserializer` over a top-level ubus `BlobTable` byte slice.
pub struct BlobTableDeserializer<'de> {
    data: &'de [u8],
}

impl<'de> Deserializer<'de> for BlobTableDeserializer<'de> {
    type Error = UbusError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_map(BlobMapAccess {
            data: self.data,
            current: None,
        })
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
        bytes byte_buf seq tuple tuple_struct enum identifier
    }
}

struct BlobMapAccess<'de> {
    data: &'de [u8],
    current: Option<(BlobTag, Payload<'de>)>,
}

impl<'de> MapAccess<'de> for BlobMapAccess<'de> {
    type Error = UbusError;

    fn next_key_seed<K: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let Some(blob) = next_blob(&mut self.data)? else {
            return Ok(None);
        };
        let (name, payload) = blob.into_named_payload()?;
        self.current = Some((blob.tag, payload));
        seed.deserialize(BorrowedStrDeserializer::<UbusError>::new(name))
            .map(Some)
    }

    fn next_value_seed<V: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        let (tag, payload) = self
            .current
            .take()
            .ok_or(UbusError::InvalidData("Map value requested without key"))?;
        seed.deserialize(BlobValueDeserializer { tag, payload })
    }
}

struct BlobSeqAccess<'de> {
    data: &'de [u8],
}

impl<'de> SeqAccess<'de> for BlobSeqAccess<'de> {
    type Error = UbusError;

    fn next_element_seed<T: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        let Some(blob) = next_blob(&mut self.data)? else {
            return Ok(None);
        };
        let val = BlobValue::from_blob(blob)?;
        seed.deserialize(BlobValueDeserializer {
            tag: val.tag,
            payload: val.payload,
        })
        .map(Some)
    }
}

struct BlobValueDeserializer<'de> {
    tag: BlobTag,
    payload: Payload<'de>,
}

impl<'de> BlobValueDeserializer<'de> {
    fn read_i64(&self) -> Result<i64, UbusError> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::BOOL => {
                let n: i8 = self.payload.try_into()?;
                Ok(i64::from(n))
            }
            BlobMsgType::INT16 => {
                let n: i16 = self.payload.try_into()?;
                Ok(i64::from(n))
            }
            BlobMsgType::INT32 => {
                let n: i32 = self.payload.try_into()?;
                Ok(i64::from(n))
            }
            BlobMsgType::INT64 => self.payload.try_into(),
            _ => Err(UbusError::InvalidData("Expected integer blob")),
        }
    }

    fn read_u64(&self) -> Result<u64, UbusError> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::BOOL => {
                let n: u8 = self.payload.try_into()?;
                Ok(u64::from(n))
            }
            BlobMsgType::INT16 => {
                let n: u16 = self.payload.try_into()?;
                Ok(u64::from(n))
            }
            BlobMsgType::INT32 => {
                let n: u32 = self.payload.try_into()?;
                Ok(u64::from(n))
            }
            BlobMsgType::INT64 => self.payload.try_into(),
            _ => Err(UbusError::InvalidData("Expected unsigned integer blob")),
        }
    }
}

macro_rules! impl_signed_deserialize {
    ($fn_name:ident, $visit_fn:ident, $ty:ty, $err_msg:literal) => {
        fn $fn_name<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            let n = <$ty>::try_from(self.read_i64()?)
                .map_err(|_| UbusError::InvalidData($err_msg))?;
            visitor.$visit_fn(n)
        }
    };
}

macro_rules! impl_unsigned_deserialize {
    ($fn_name:ident, $visit_fn:ident, $ty:ty, $err_msg:literal) => {
        fn $fn_name<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            let n = <$ty>::try_from(self.read_u64()?)
                .map_err(|_| UbusError::InvalidData($err_msg))?;
            visitor.$visit_fn(n)
        }
    };
}

impl<'de> Deserializer<'de> for BlobValueDeserializer<'de> {
    type Error = UbusError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::ARRAY => visitor.visit_seq(BlobSeqAccess {
                data: self.payload.into(),
            }),
            BlobMsgType::TABLE => visitor.visit_map(BlobMapAccess {
                data: self.payload.into(),
                current: None,
            }),
            BlobMsgType::STRING => {
                let s: &'de str = self.payload.try_into()?;
                visitor.visit_borrowed_str(s)
            }
            BlobMsgType::INT64 => visitor.visit_i64(self.payload.try_into()?),
            BlobMsgType::INT32 => visitor.visit_i32(self.payload.try_into()?),
            BlobMsgType::INT16 => visitor.visit_i16(self.payload.try_into()?),
            BlobMsgType::BOOL => visitor.visit_bool(self.payload.try_into()?),
            BlobMsgType::DOUBLE => visitor.visit_f64(self.payload.try_into()?),
            _ => visitor.visit_borrowed_bytes(self.payload.into()),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::BOOL => visitor.visit_bool(self.payload.try_into()?),
            BlobMsgType::INT16 | BlobMsgType::INT32 | BlobMsgType::INT64 => {
                visitor.visit_bool(self.read_i64()? != 0)
            }
            _ => Err(UbusError::InvalidData("Expected bool blob")),
        }
    }

    impl_signed_deserialize!(deserialize_i8, visit_i8, i8, "Integer overflow for i8");
    impl_signed_deserialize!(deserialize_i16, visit_i16, i16, "Integer overflow for i16");
    impl_signed_deserialize!(deserialize_i32, visit_i32, i32, "Integer overflow for i32");

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i64(self.read_i64()?)
    }

    impl_unsigned_deserialize!(deserialize_u8, visit_u8, u8, "Integer overflow for u8");
    impl_unsigned_deserialize!(deserialize_u16, visit_u16, u16, "Integer overflow for u16");
    impl_unsigned_deserialize!(deserialize_u32, visit_u32, u32, "Integer overflow for u32");

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u64(self.read_u64()?)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::DOUBLE => visitor.visit_f64(self.payload.try_into()?),
            BlobMsgType::BOOL
            | BlobMsgType::INT16
            | BlobMsgType::INT32
            | BlobMsgType::INT64 => visitor.visit_f64(self.read_i64()? as f64),
            _ => Err(UbusError::InvalidData("Expected float or integer blob")),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        if BlobMsgType::from(self.tag.id()) == BlobMsgType::STRING {
            let s: &'de str = self.payload.try_into()?;
            visitor.visit_borrowed_str(s)
        } else {
            Err(UbusError::InvalidData("Expected string blob"))
        }
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_borrowed_bytes(self.payload.into())
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_borrowed_bytes(self.payload.into())
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        if BlobMsgType::from(self.tag.id()) == BlobMsgType::ARRAY {
            visitor.visit_seq(BlobSeqAccess {
                data: self.payload.into(),
            })
        } else {
            Err(UbusError::InvalidData("Expected array blob"))
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        if BlobMsgType::from(self.tag.id()) == BlobMsgType::TABLE {
            visitor.visit_map(BlobMapAccess {
                data: self.payload.into(),
                current: None,
            })
        } else {
            Err(UbusError::InvalidData("Expected table blob"))
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::STRING => {
                let s: &'de str = self.payload.try_into()?;
                visitor.visit_enum(BorrowedStrDeserializer::<UbusError>::new(s))
            }
            BlobMsgType::TABLE => visitor.visit_enum(MapAccessDeserializer::new(BlobMapAccess {
                data: self.payload.into(),
                current: None,
            })),
            _ => Err(UbusError::InvalidData(
                "Expected string or table blob for enum",
            )),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }
}

impl<'a> BlobIter<'a, Blob<'a>> {
    /// Wrap the remaining blob bytes as a zero-copy `BlobTable` for serde serialization.
    pub const fn as_table(&self) -> BlobTable<'a> {
        BlobTable(self.as_bytes())
    }

    /// Deserialize the remaining blob bytes directly into a typed Rust struct via serde.
    pub fn deserialize<T: Deserialize<'a>>(&self) -> Result<T, UbusError> {
        from_blob(self.as_bytes())
    }
}
