use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Method<'a> {
    pub name: &'a str,
    pub policy: HashMap<&'a str, BlobMsgType>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct UbusObject<'a> {
    pub path: &'a str,
    pub id: u32,
    pub ty: u32,
    pub methods: HashMap<&'a str, Method<'a>>,
}

#[cfg(feature = "json")]
mod json_args {
    use crate::*;
    use alloc::{borrow::Cow, string::ToString, vec::Vec};
    use core::fmt;
    use serde::de::{
        self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor,
    };

    #[derive(Copy, Clone)]
    pub enum FieldPolicy {
        Typed(BlobMsgType),
        Dynamic,
        Skip,
    }

    struct TypedValueSeed<'a, 'k> {
        buf: &'a mut Vec<u8>,
        name: &'k str,
        expected: Option<BlobMsgType>,
        blob_err: &'a mut Option<UbusError>,
    }

    impl TypedValueSeed<'_, '_> {
        fn map_blob_err<T, E: de::Error>(
            blob_err: &mut Option<UbusError>,
            res: Result<T, UbusError>,
        ) -> Result<T, E> {
            res.map_err(|e| {
                *blob_err = Some(e);
                E::custom("blob encoding error")
            })
        }

        fn write_bytes<E: de::Error>(&mut self, ty: BlobMsgType, bytes: &[u8]) -> Result<(), E> {
            Self::map_blob_err(
                self.blob_err,
                BlobMsgBuilder::write_extended_in(self.buf, ty.value(), self.name, bytes),
            )
        }

        fn write_str<E: de::Error>(&mut self, s: &str) -> Result<(), E> {
            Self::map_blob_err(
                self.blob_err,
                BlobMsgBuilder::write_extended_str_in(self.buf, self.name, s),
            )
        }

        fn write_int<E: de::Error>(
            &mut self,
            val: i64,
            fits_i32: bool,
            as_f64: f64,
        ) -> Result<(), E> {
            match self.expected {
                None if fits_i32 => self.write_bytes(BlobMsgType::INT32, &(val as i32).to_be_bytes()),
                None | Some(BlobMsgType::INT64) => {
                    self.write_bytes(BlobMsgType::INT64, &val.to_be_bytes())
                }
                Some(BlobMsgType::INT32) => {
                    self.write_bytes(BlobMsgType::INT32, &(val as i32).to_be_bytes())
                }
                Some(BlobMsgType::INT16) => {
                    self.write_bytes(BlobMsgType::INT16, &(val as i16).to_be_bytes())
                }
                Some(BlobMsgType::BOOL) => {
                    self.write_bytes(BlobMsgType::BOOL, &(val as i8).to_be_bytes())
                }
                Some(BlobMsgType::DOUBLE) => {
                    self.write_bytes(BlobMsgType::DOUBLE, &as_f64.to_be_bytes())
                }
                _ => Ok(()),
            }
        }
    }

    impl<'de> Visitor<'de> for TypedValueSeed<'_, '_> {
        type Value = ();

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a JSON argument value")
        }

        fn visit_bool<E: de::Error>(mut self, v: bool) -> Result<Self::Value, E> {
            match self.expected {
                None | Some(BlobMsgType::BOOL) => {
                    self.write_bytes(BlobMsgType::BOOL, &[u8::from(v)])
                }
                _ => Ok(()),
            }
        }

        fn visit_i64<E: de::Error>(mut self, v: i64) -> Result<Self::Value, E> {
            self.write_int(v, i32::try_from(v).is_ok(), v as f64)
        }

        fn visit_u64<E: de::Error>(mut self, v: u64) -> Result<Self::Value, E> {
            self.write_int(v as i64, i32::try_from(v).is_ok(), v as f64)
        }

        fn visit_f64<E: de::Error>(mut self, v: f64) -> Result<Self::Value, E> {
            match self.expected {
                None | Some(BlobMsgType::DOUBLE) => {
                    self.write_bytes(BlobMsgType::DOUBLE, &v.to_be_bytes())
                }
                _ => Ok(()),
            }
        }

        fn visit_str<E: de::Error>(mut self, v: &str) -> Result<Self::Value, E> {
            match self.expected {
                None | Some(BlobMsgType::STRING) => self.write_str(v),
                _ => Ok(()),
            }
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            if matches!(self.expected, None | Some(BlobMsgType::ARRAY)) {
                let id = BlobMsgType::ARRAY.value();
                let start = Self::map_blob_err(
                    self.blob_err,
                    BlobMsgBuilder::begin_extended_in(self.buf, id, self.name),
                )?;
                while seq
                    .next_element_seed(TypedValueSeed {
                        buf: self.buf,
                        name: "",
                        expected: None,
                        blob_err: self.blob_err,
                    })?
                    .is_some()
                {}
                Self::map_blob_err(
                    self.blob_err,
                    BlobMsgBuilder::end_extended_in(self.buf, start, id),
                )?;
            } else {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
            }
            Ok(())
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            if matches!(self.expected, None | Some(BlobMsgType::TABLE)) {
                let id = BlobMsgType::TABLE.value();
                let start = Self::map_blob_err(
                    self.blob_err,
                    BlobMsgBuilder::begin_extended_in(self.buf, id, self.name),
                )?;
                while let Some(k) = map.next_key::<Cow<'de, str>>()? {
                    map.next_value_seed(TypedValueSeed {
                        buf: self.buf,
                        name: k.as_ref(),
                        expected: None,
                        blob_err: self.blob_err,
                    })?;
                }
                Self::map_blob_err(
                    self.blob_err,
                    BlobMsgBuilder::end_extended_in(self.buf, start, id),
                )?;
            } else {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            }
            Ok(())
        }
    }

    impl<'de> DeserializeSeed<'de> for TypedValueSeed<'_, '_> {
        type Value = ();

        fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
            deserializer.deserialize_any(self)
        }
    }

    struct PolicyEncoder<'a> {
        args: Vec<u8>,
        lookup_policy: &'a mut dyn FnMut(&str) -> FieldPolicy,
        blob_err: Option<UbusError>,
    }

    impl<'de> Visitor<'de> for &mut PolicyEncoder<'_> {
        type Value = ();

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                let expected = match (self.lookup_policy)(key.as_ref()) {
                    FieldPolicy::Typed(ty) => Some(ty),
                    FieldPolicy::Dynamic => None,
                    FieldPolicy::Skip => {
                        map.next_value::<IgnoredAny>()?;
                        continue;
                    }
                };
                map.next_value_seed(TypedValueSeed {
                    buf: &mut self.args,
                    name: key.as_ref(),
                    expected,
                    blob_err: &mut self.blob_err,
                })?;
            }
            Ok(())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(())
        }

        fn visit_bool<E: de::Error>(self, _v: bool) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_i64<E: de::Error>(self, _v: i64) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_u64<E: de::Error>(self, _v: u64) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_f64<E: de::Error>(self, _v: f64) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_str<E: de::Error>(self, _v: &str) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(())
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(())
        }
    }

    impl<'de> DeserializeSeed<'de> for &mut PolicyEncoder<'_> {
        type Value = ();

        fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
            deserializer.deserialize_any(self)
        }
    }

    /// Encode a JSON object string directly into a single `Vec<u8>` of extended `BlobMsg` attributes
    /// according to a policy lookup closure, supporting nested arrays/tables without `serde_json::Value`.
    pub fn encode_json_with_policy(
        json: &str,
        lookup_policy: &mut dyn FnMut(&str) -> FieldPolicy,
    ) -> Result<Vec<u8>, UbusError> {
        if json.is_empty() {
            return Ok(Vec::new());
        }
        let mut encoder = PolicyEncoder {
            args: Vec::new(),
            lookup_policy,
            blob_err: None,
        };
        let mut de = serde_json::Deserializer::from_str(json);
        (&mut encoder)
            .deserialize(&mut de)
            .map_err(|e| encoder.blob_err.take().unwrap_or(UbusError::ParseArguments(e)))?;
        de.end()?;
        Ok(encoder.args)
    }

    /// Encode any JSON object string directly into ubus extended blob bytes using dynamic JSON type inference.
    pub fn json_to_blob(json: &str) -> Result<Vec<u8>, UbusError> {
        encode_json_with_policy(json, &mut |_| FieldPolicy::Dynamic)
    }

    impl UbusObject<'_> {
        pub fn args_from_json(&self, method: &str, json: &str) -> Result<Vec<u8>, UbusError> {
            if json.is_empty() {
                return Ok(Vec::new());
            }
            let method_entry = self.methods.get(method);
            let mut saw_key = false;
            let args = encode_json_with_policy(json, &mut |k| {
                saw_key = true;
                match method_entry {
                    Some(m) if m.policy.is_empty() => FieldPolicy::Dynamic,
                    Some(m) => m
                        .policy
                        .get(k)
                        .copied()
                        .map_or(FieldPolicy::Skip, FieldPolicy::Typed),
                    None => FieldPolicy::Skip,
                }
            })?;
            if method_entry.is_none() && (saw_key || json.trim_start().starts_with('{')) {
                return Err(UbusError::InvalidMethod(method.to_string()));
            }
            Ok(args)
        }
    }
}

#[cfg(feature = "json")]
pub(crate) use json_args::{FieldPolicy, encode_json_with_policy};
#[cfg(feature = "json")]
pub use json_args::json_to_blob;
