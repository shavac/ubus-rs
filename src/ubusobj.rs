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
    use alloc::{
        borrow::Cow,
        string::{String, ToString},
        vec::Vec,
    };
    use core::fmt;
    use serde::de::{
        self, Deserialize, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess,
        Visitor,
    };

    /// Lightweight scalar representation that borrows strings directly from the input JSON slice
    /// instead of allocating a `serde_json::Value` DOM tree on the heap.
    enum ScalarArg<'de> {
        Str(Cow<'de, str>),
        I64(i64),
        U64(u64),
        F64(f64),
        Bool(bool),
        Other,
    }

    impl<'de> ScalarArg<'de> {
        fn as_str(&self) -> Option<&str> {
            match self {
                ScalarArg::Str(s) => Some(s.as_ref()),
                _ => None,
            }
        }

        fn as_i64(&self) -> Option<i64> {
            match *self {
                ScalarArg::I64(n) => Some(n),
                ScalarArg::U64(n) => i64::try_from(n).ok(),
                _ => None,
            }
        }

        fn as_f64(&self) -> Option<f64> {
            match *self {
                ScalarArg::F64(n) => Some(n),
                ScalarArg::I64(n) => Some(n as f64),
                ScalarArg::U64(n) => Some(n as f64),
                _ => None,
            }
        }

        fn as_bool(&self) -> Option<bool> {
            match *self {
                ScalarArg::Bool(b) => Some(b),
                _ => None,
            }
        }
    }

    impl<'de> Deserialize<'de> for ScalarArg<'de> {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct ScalarVisitor;

            impl<'de> Visitor<'de> for ScalarVisitor {
                type Value = ScalarArg<'de>;

                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter.write_str("any JSON value")
                }

                fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
                    Ok(ScalarArg::Str(Cow::Borrowed(v)))
                }

                fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                    Ok(ScalarArg::Str(Cow::Owned(String::from(v))))
                }

                fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                    Ok(ScalarArg::Str(Cow::Owned(v)))
                }

                fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                    Ok(ScalarArg::I64(v))
                }

                fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                    Ok(ScalarArg::U64(v))
                }

                fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                    Ok(ScalarArg::F64(v))
                }

                fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                    Ok(ScalarArg::Bool(v))
                }

                fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                    Ok(ScalarArg::Other)
                }

                fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                    Ok(ScalarArg::Other)
                }

                fn visit_seq<A: SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Self::Value, A::Error> {
                    while seq.next_element::<IgnoredAny>()?.is_some() {}
                    Ok(ScalarArg::Other)
                }

                fn visit_map<A: MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Self::Value, A::Error> {
                    while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                    Ok(ScalarArg::Other)
                }
            }

            deserializer.deserialize_any(ScalarVisitor)
        }
    }

    /// Encode a JSON object string directly into a single `Vec<u8>` of extended `BlobMsg` attributes
    /// according to a policy lookup closure, without allocating `serde_json::Value` or intermediate buffers.
    pub(crate) fn encode_json_with_policy<F>(
        json: &str,
        lookup_type: F,
    ) -> Result<Vec<u8>, UbusError>
    where
        F: FnMut(&str) -> Option<BlobMsgType>,
    {
        if json.is_empty() {
            return Ok(Vec::new());
        }

        struct PolicyEncoder<F> {
            args: Vec<u8>,
            lookup_type: F,
            blob_err: Option<UbusError>,
        }

        impl<'de, F> Visitor<'de> for &mut PolicyEncoder<F>
        where
            F: FnMut(&str) -> Option<BlobMsgType>,
        {
            type Value = ();

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JSON argument value")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    let key_str = key.as_ref();
                    if let Some(arg_typ) = (self.lookup_type)(key_str) {
                        let val = map.next_value::<ScalarArg<'de>>()?;
                        let res = match arg_typ {
                            BlobMsgType::STRING => {
                                let start = BlobMsgBuilder::begin_extended_in(
                                    &mut self.args,
                                    arg_typ.value(),
                                    key_str,
                                );
                                start.and_then(|start| {
                                    if let Some(s) = val.as_str() {
                                        self.args.extend_from_slice(s.as_bytes());
                                        self.args.push(b'\0');
                                    }
                                    BlobMsgBuilder::end_extended_in(
                                        &mut self.args,
                                        start,
                                        arg_typ.value(),
                                    )
                                })
                            }
                            BlobMsgType::INT64 => {
                                let start = BlobMsgBuilder::begin_extended_in(
                                    &mut self.args,
                                    arg_typ.value(),
                                    key_str,
                                );
                                start.and_then(|start| {
                                    if let Some(num) = val.as_i64() {
                                        self.args.extend_from_slice(&num.to_be_bytes());
                                    }
                                    BlobMsgBuilder::end_extended_in(
                                        &mut self.args,
                                        start,
                                        arg_typ.value(),
                                    )
                                })
                            }
                            BlobMsgType::INT32 => {
                                let start = BlobMsgBuilder::begin_extended_in(
                                    &mut self.args,
                                    arg_typ.value(),
                                    key_str,
                                );
                                start.and_then(|start| {
                                    if let Some(num) = val.as_i64() {
                                        self.args.extend_from_slice(&(num as i32).to_be_bytes());
                                    }
                                    BlobMsgBuilder::end_extended_in(
                                        &mut self.args,
                                        start,
                                        arg_typ.value(),
                                    )
                                })
                            }
                            BlobMsgType::INT16 => {
                                let start = BlobMsgBuilder::begin_extended_in(
                                    &mut self.args,
                                    arg_typ.value(),
                                    key_str,
                                );
                                start.and_then(|start| {
                                    if let Some(num) = val.as_i64() {
                                        self.args.extend_from_slice(&(num as i16).to_be_bytes());
                                    }
                                    BlobMsgBuilder::end_extended_in(
                                        &mut self.args,
                                        start,
                                        arg_typ.value(),
                                    )
                                })
                            }
                            BlobMsgType::INT8 => {
                                let start = BlobMsgBuilder::begin_extended_in(
                                    &mut self.args,
                                    arg_typ.value(),
                                    key_str,
                                );
                                start.and_then(|start| {
                                    if let Some(num) = val.as_i64() {
                                        self.args.extend_from_slice(&(num as i8).to_be_bytes());
                                    } else if let Some(b) = val.as_bool() {
                                        let tf: i8 = if b { 1 } else { 0 };
                                        self.args.extend_from_slice(&tf.to_be_bytes());
                                    }
                                    BlobMsgBuilder::end_extended_in(
                                        &mut self.args,
                                        start,
                                        arg_typ.value(),
                                    )
                                })
                            }
                            BlobMsgType::DOUBLE => {
                                let start = BlobMsgBuilder::begin_extended_in(
                                    &mut self.args,
                                    arg_typ.value(),
                                    key_str,
                                );
                                start.and_then(|start| {
                                    if let Some(num) = val.as_f64() {
                                        self.args.extend_from_slice(&num.to_be_bytes());
                                    }
                                    BlobMsgBuilder::end_extended_in(
                                        &mut self.args,
                                        start,
                                        arg_typ.value(),
                                    )
                                })
                            }
                            _ => Ok(()),
                        };
                        if let Err(e) = res {
                            self.blob_err = Some(e);
                            return Err(de::Error::custom("blob encoding error"));
                        }
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
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

        impl<'de, F> DeserializeSeed<'de> for &mut PolicyEncoder<F>
        where
            F: FnMut(&str) -> Option<BlobMsgType>,
        {
            type Value = ();

            fn deserialize<D: Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                deserializer.deserialize_any(self)
            }
        }

        let mut encoder = PolicyEncoder {
            args: Vec::new(),
            lookup_type,
            blob_err: None,
        };
        let mut de = serde_json::Deserializer::from_str(json);
        match (&mut encoder).deserialize(&mut de) {
            Ok(()) => {
                de.end().map_err(UbusError::ParseArguments)?;
                Ok(encoder.args)
            }
            Err(e) => {
                if let Some(blob_err) = encoder.blob_err {
                    Err(blob_err)
                } else {
                    Err(UbusError::ParseArguments(e))
                }
            }
        }
    }

    impl<'a> UbusObject<'a> {
        pub fn args_from_json(&self, method: &str, json: &str) -> Result<Vec<u8>, UbusError> {
            if json.is_empty() {
                return Ok(Vec::new());
            }
            // Validate that the method exists once if the input is a JSON object
            let method_entry = self.methods.get(method);
            let mut need_method_check = false;
            let args = encode_json_with_policy(json, |k| {
                need_method_check = true;
                method_entry.and_then(|m| m.policy.get(k).copied())
            })?;
            if need_method_check && method_entry.is_none() {
                return Err(UbusError::InvalidMethod(method.to_string()));
            }
            // Also check when json is `{}` (an empty JSON object)
            if method_entry.is_none() && json.trim_start().starts_with('{') {
                return Err(UbusError::InvalidMethod(method.to_string()));
            }
            Ok(args)
        }
    }
}

#[cfg(feature = "json")]
pub(crate) use json_args::encode_json_with_policy;
