use crate::{
    Blob, BlobIter, BlobMsgBuilder, BlobMsgType, BlobTag, OptionInvalidExt, Payload, UbusError,
};
use alloc::{string::String, vec::Vec};
use core::convert::TryInto;
use core::fmt::Write as _;
use serde::de::value::{BorrowedStrDeserializer, MapAccessDeserializer};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{
    Error as SerError, Serialize, SerializeMap, SerializeSeq, SerializeStruct,
    SerializeStructVariant, SerializeTuple, SerializeTupleStruct, SerializeTupleVariant,
    Serializer,
};

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
                    let elem = BlobValue::from_blob(item).map_err(S::Error::custom)?;
                    seq.serialize_element(&elem)?;
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
            .or_invalid("Map value requested without key")?;
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

impl BlobValueDeserializer<'_> {
    fn read_i64(&self) -> Result<i64, UbusError> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::BOOL => TryInto::<i8>::try_into(self.payload).map(i64::from),
            BlobMsgType::INT16 => TryInto::<i16>::try_into(self.payload).map(i64::from),
            BlobMsgType::INT32 => TryInto::<i32>::try_into(self.payload).map(i64::from),
            BlobMsgType::INT64 => self.payload.try_into(),
            _ => Err(UbusError::InvalidData("Expected integer blob")),
        }
    }

    fn read_u64(&self) -> Result<u64, UbusError> {
        match BlobMsgType::from(self.tag.id()) {
            BlobMsgType::BOOL => TryInto::<u8>::try_into(self.payload).map(u64::from),
            BlobMsgType::INT16 => TryInto::<u16>::try_into(self.payload).map(u64::from),
            BlobMsgType::INT32 => TryInto::<u32>::try_into(self.payload).map(u64::from),
            BlobMsgType::INT64 => self.payload.try_into(),
            _ => Err(UbusError::InvalidData("Expected unsigned integer blob")),
        }
    }
}

macro_rules! impl_int_deserialize {
    ($fn_name:ident, $visit_fn:ident, $ty:ty, $reader:ident, $err_msg:literal) => {
        fn $fn_name<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            let n = <$ty>::try_from(self.$reader()?)
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

    impl_int_deserialize!(deserialize_i8, visit_i8, i8, read_i64, "Integer overflow for i8");
    impl_int_deserialize!(deserialize_i16, visit_i16, i16, read_i64, "Integer overflow for i16");
    impl_int_deserialize!(deserialize_i32, visit_i32, i32, read_i64, "Integer overflow for i32");

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i64(self.read_i64()?)
    }

    impl_int_deserialize!(deserialize_u8, visit_u8, u8, read_u64, "Integer overflow for u8");
    impl_int_deserialize!(deserialize_u16, visit_u16, u16, read_u64, "Integer overflow for u16");
    impl_int_deserialize!(deserialize_u32, visit_u32, u32, read_u64, "Integer overflow for u32");

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

/// Serialize a Rust struct or map directly into ubus extended blob bytes without JSON.
pub fn to_blob<T: Serialize>(value: &T) -> Result<Vec<u8>, UbusError> {
    let mut buf = Vec::new();
    to_blob_into(&mut buf, value)?;
    Ok(buf)
}

/// Serialize a Rust struct or map directly into `buf` as ubus extended blob bytes.
pub fn to_blob_into<T: Serialize>(buf: &mut Vec<u8>, value: &T) -> Result<(), UbusError> {
    value.serialize(BlobTableSerializer { buf })
}

macro_rules! impl_ser_err {
    (forward_ints) => {
        fn serialize_i8(self, v: i8) -> Result<(), Self::Error> {
            self.serialize_i64(i64::from(v))
        }
        fn serialize_i16(self, v: i16) -> Result<(), Self::Error> {
            self.serialize_i64(i64::from(v))
        }
        fn serialize_i32(self, v: i32) -> Result<(), Self::Error> {
            self.serialize_i64(i64::from(v))
        }
        fn serialize_u8(self, v: u8) -> Result<(), Self::Error> {
            self.serialize_u64(u64::from(v))
        }
        fn serialize_u16(self, v: u16) -> Result<(), Self::Error> {
            self.serialize_u64(u64::from(v))
        }
        fn serialize_u32(self, v: u32) -> Result<(), Self::Error> {
            self.serialize_u64(u64::from(v))
        }
    };
    ($err:expr; scalar $($fn_name:ident($ty:ty)),* $(,)?) => {
        $(
            fn $fn_name(self, _: $ty) -> Result<(), Self::Error> {
                Err(UbusError::InvalidData($err))
            }
        )*
    };
    ($err:expr; unit $($fn_name:ident()),* $(,)?) => {
        $(
            fn $fn_name(self) -> Result<(), Self::Error> {
                Err(UbusError::InvalidData($err))
            }
        )*
    };
    ($err:expr; seq_variants) => {
        fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
            Err(UbusError::InvalidData($err))
        }
        fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Self::Error> {
            Err(UbusError::InvalidData($err))
        }
        fn serialize_tuple_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleStruct, Self::Error> {
            Err(UbusError::InvalidData($err))
        }
        fn serialize_tuple_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleVariant, Self::Error> {
            Err(UbusError::InvalidData($err))
        }
        fn serialize_struct_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeStructVariant, Self::Error> {
            Err(UbusError::InvalidData($err))
        }
    };
}

const TOP_LEVEL_ERR: &str = "Top-level ubus blob payload must be a struct or map";
const KEY_ERR: &str = "Map key must be a string";

struct BlobTableSerializer<'a> {
    buf: &'a mut Vec<u8>,
}

impl<'a> Serializer for BlobTableSerializer<'a> {
    type Ok = ();
    type Error = UbusError;
    type SerializeSeq = serde::ser::Impossible<(), UbusError>;
    type SerializeTuple = serde::ser::Impossible<(), UbusError>;
    type SerializeTupleStruct = serde::ser::Impossible<(), UbusError>;
    type SerializeTupleVariant = serde::ser::Impossible<(), UbusError>;
    type SerializeMap = BlobMapSerializer<'a>;
    type SerializeStruct = BlobMapSerializer<'a>;
    type SerializeStructVariant = serde::ser::Impossible<(), UbusError>;

    impl_ser_err!(forward_ints);
    impl_ser_err!(TOP_LEVEL_ERR; scalar
        serialize_bool(bool),
        serialize_i64(i64),
        serialize_u64(u64),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
    );
    impl_ser_err!(TOP_LEVEL_ERR; seq_variants);

    fn serialize_f32(self, v: f32) -> Result<(), Self::Error> {
        self.serialize_f64(f64::from(v))
    }

    fn serialize_none(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
    ) -> Result<(), Self::Error> {
        Err(UbusError::InvalidData(TOP_LEVEL_ERR))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(BlobValueSerializer {
            buf: self.buf,
            name: variant,
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(BlobMapSerializer::new(self.buf, None, None))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(BlobMapSerializer::new(self.buf, None, None))
    }
}

struct StrKeyCapture<'a>(&'a mut String);

impl Serializer for StrKeyCapture<'_> {
    type Ok = ();
    type Error = UbusError;
    type SerializeSeq = serde::ser::Impossible<(), UbusError>;
    type SerializeTuple = serde::ser::Impossible<(), UbusError>;
    type SerializeTupleStruct = serde::ser::Impossible<(), UbusError>;
    type SerializeTupleVariant = serde::ser::Impossible<(), UbusError>;
    type SerializeMap = serde::ser::Impossible<(), UbusError>;
    type SerializeStruct = serde::ser::Impossible<(), UbusError>;
    type SerializeStructVariant = serde::ser::Impossible<(), UbusError>;

    impl_ser_err!(forward_ints);
    impl_ser_err!(KEY_ERR; scalar
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_bytes(&[u8]),
        serialize_unit_struct(&'static str),
    );
    impl_ser_err!(KEY_ERR; unit serialize_none(), serialize_unit());
    impl_ser_err!(KEY_ERR; seq_variants);

    fn serialize_bool(self, v: bool) -> Result<(), Self::Error> {
        self.0.push_str(if v { "true" } else { "false" });
        Ok(())
    }

    fn serialize_i64(self, v: i64) -> Result<(), Self::Error> {
        let _ = write!(self.0, "{v}");
        Ok(())
    }

    fn serialize_u64(self, v: u64) -> Result<(), Self::Error> {
        let _ = write!(self.0, "{v}");
        Ok(())
    }

    fn serialize_char(self, v: char) -> Result<(), Self::Error> {
        self.0.push(v);
        Ok(())
    }

    fn serialize_str(self, v: &str) -> Result<(), Self::Error> {
        self.0.push_str(v);
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<(), Self::Error> {
        self.0.push_str(variant);
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<(), Self::Error> {
        Err(UbusError::InvalidData(KEY_ERR))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(UbusError::InvalidData(KEY_ERR))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(UbusError::InvalidData(KEY_ERR))
    }
}

struct BlobMapSerializer<'a> {
    buf: &'a mut Vec<u8>,
    table_start: Option<usize>,
    outer_table_start: Option<usize>,
    key_scratch: String,
    has_key: bool,
}

impl<'a> BlobMapSerializer<'a> {
    fn new(
        buf: &'a mut Vec<u8>,
        table_start: Option<usize>,
        outer_table_start: Option<usize>,
    ) -> Self {
        Self {
            buf,
            table_start,
            outer_table_start,
            key_scratch: String::new(),
            has_key: false,
        }
    }

    fn finish(self) -> Result<(), UbusError> {
        for start in [self.table_start, self.outer_table_start]
            .into_iter()
            .flatten()
        {
            BlobMsgBuilder::end_extended_in(self.buf, start, BlobMsgType::TABLE.value())?;
        }
        Ok(())
    }
}

impl SerializeMap for BlobMapSerializer<'_> {
    type Ok = ();
    type Error = UbusError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.key_scratch.clear();
        self.has_key = true;
        key.serialize(StrKeyCapture(&mut self.key_scratch))
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        if !self.has_key {
            return Err(UbusError::InvalidData("Map value serialized without key"));
        }
        self.has_key = false;
        value.serialize(BlobValueSerializer {
            buf: self.buf,
            name: &self.key_scratch,
        })
    }

    fn serialize_entry<K: ?Sized + Serialize, V: ?Sized + Serialize>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), Self::Error> {
        self.serialize_key(key)?;
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish()
    }
}

macro_rules! impl_blob_struct_ser {
    ($($trait_name:ident),*) => {
        $(
            impl $trait_name for BlobMapSerializer<'_> {
                type Ok = ();
                type Error = UbusError;

                fn serialize_field<T: ?Sized + Serialize>(
                    &mut self,
                    key: &'static str,
                    value: &T,
                ) -> Result<(), Self::Error> {
                    value.serialize(BlobValueSerializer {
                        buf: self.buf,
                        name: key,
                    })
                }

                fn end(self) -> Result<Self::Ok, Self::Error> {
                    self.finish()
                }
            }
        )*
    };
}
impl_blob_struct_ser!(SerializeStruct, SerializeStructVariant);

struct BlobSeqSerializer<'a> {
    buf: &'a mut Vec<u8>,
    array_start: usize,
    outer_table_start: Option<usize>,
}

impl BlobSeqSerializer<'_> {
    fn finish(self) -> Result<(), UbusError> {
        BlobMsgBuilder::end_extended_in(self.buf, self.array_start, BlobMsgType::ARRAY.value())?;
        self.outer_table_start.map_or(Ok(()), |outer| {
            BlobMsgBuilder::end_extended_in(self.buf, outer, BlobMsgType::TABLE.value())
        })
    }
}

macro_rules! impl_blob_seq_ser {
    ($($trait_name:ident :: $method:ident),*) => {
        $(
            impl $trait_name for BlobSeqSerializer<'_> {
                type Ok = ();
                type Error = UbusError;

                fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
                    value.serialize(BlobValueSerializer {
                        buf: self.buf,
                        name: "",
                    })
                }

                fn end(self) -> Result<Self::Ok, Self::Error> {
                    self.finish()
                }
            }
        )*
    };
}
impl_blob_seq_ser!(
    SerializeSeq::serialize_element,
    SerializeTuple::serialize_element,
    SerializeTupleStruct::serialize_field,
    SerializeTupleVariant::serialize_field
);

struct BlobValueSerializer<'a, 'k> {
    buf: &'a mut Vec<u8>,
    name: &'k str,
}

impl BlobValueSerializer<'_, '_> {
    fn write_primitive(self, id: BlobMsgType, bytes: &[u8]) -> Result<(), UbusError> {
        BlobMsgBuilder::write_extended_in(self.buf, id.value(), self.name, bytes)
    }
}

macro_rules! impl_val_int_ser {
    ($($fn_name:ident($ty:ty) => $msg_ty:ident),* $(,)?) => {
        $(
            fn $fn_name(self, v: $ty) -> Result<(), Self::Error> {
                self.write_primitive(BlobMsgType::$msg_ty, &v.to_be_bytes())
            }
        )*
    };
}

impl<'a> Serializer for BlobValueSerializer<'a, '_> {
    type Ok = ();
    type Error = UbusError;
    type SerializeSeq = BlobSeqSerializer<'a>;
    type SerializeTuple = BlobSeqSerializer<'a>;
    type SerializeTupleStruct = BlobSeqSerializer<'a>;
    type SerializeTupleVariant = BlobSeqSerializer<'a>;
    type SerializeMap = BlobMapSerializer<'a>;
    type SerializeStruct = BlobMapSerializer<'a>;
    type SerializeStructVariant = BlobMapSerializer<'a>;

    fn serialize_bool(self, v: bool) -> Result<(), Self::Error> {
        self.write_primitive(BlobMsgType::BOOL, &[u8::from(v)])
    }

    impl_val_int_ser!(
        serialize_i8(i8) => INT8,
        serialize_i16(i16) => INT16,
        serialize_i32(i32) => INT32,
        serialize_i64(i64) => INT64,
        serialize_u8(u8) => INT8,
        serialize_u16(u16) => INT16,
        serialize_u32(u32) => INT32,
        serialize_u64(u64) => INT64,
    );

    fn serialize_i128(self, _v: i128) -> Result<(), Self::Error> {
        Err(UbusError::InvalidData(
            "128-bit integers not supported in ubus",
        ))
    }

    fn serialize_u128(self, _v: u128) -> Result<(), Self::Error> {
        Err(UbusError::InvalidData(
            "128-bit integers not supported in ubus",
        ))
    }

    fn serialize_f32(self, v: f32) -> Result<(), Self::Error> {
        self.serialize_f64(f64::from(v))
    }

    fn serialize_f64(self, v: f64) -> Result<(), Self::Error> {
        self.write_primitive(BlobMsgType::DOUBLE, &v.to_be_bytes())
    }

    fn serialize_char(self, v: char) -> Result<(), Self::Error> {
        let mut buf = [0u8; 4];
        self.serialize_str(v.encode_utf8(&mut buf))
    }

    fn serialize_str(self, v: &str) -> Result<(), Self::Error> {
        BlobMsgBuilder::write_extended_str_in(self.buf, self.name, v)
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<(), Self::Error> {
        self.write_primitive(BlobMsgType::UNSPEC, v)
    }

    fn serialize_none(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<(), Self::Error> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        let table_id = BlobMsgType::TABLE.value();
        let start = BlobMsgBuilder::begin_extended_in(self.buf, table_id, self.name)?;
        value.serialize(BlobValueSerializer {
            buf: self.buf,
            name: variant,
        })?;
        BlobMsgBuilder::end_extended_in(self.buf, start, table_id)
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        let array_start =
            BlobMsgBuilder::begin_extended_in(self.buf, BlobMsgType::ARRAY.value(), self.name)?;
        Ok(BlobSeqSerializer {
            buf: self.buf,
            array_start,
            outer_table_start: None,
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        let outer =
            BlobMsgBuilder::begin_extended_in(self.buf, BlobMsgType::TABLE.value(), self.name)?;
        let inner =
            BlobMsgBuilder::begin_extended_in(self.buf, BlobMsgType::ARRAY.value(), variant)?;
        Ok(BlobSeqSerializer {
            buf: self.buf,
            array_start: inner,
            outer_table_start: Some(outer),
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        let start =
            BlobMsgBuilder::begin_extended_in(self.buf, BlobMsgType::TABLE.value(), self.name)?;
        Ok(BlobMapSerializer::new(self.buf, Some(start), None))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        let outer =
            BlobMsgBuilder::begin_extended_in(self.buf, BlobMsgType::TABLE.value(), self.name)?;
        let inner =
            BlobMsgBuilder::begin_extended_in(self.buf, BlobMsgType::TABLE.value(), variant)?;
        Ok(BlobMapSerializer::new(self.buf, Some(inner), Some(outer)))
    }
}
