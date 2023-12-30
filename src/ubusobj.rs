extern crate alloc;
use crate::*;
use alloc::{string::ToString, vec::Vec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

impl<'a> UbusObject<'a> {
    pub fn args_from_json(&self, method: &'a str, json: &'a str) -> Result<Vec<u8>, UbusError> {
        let mut args = Vec::new();
        if json.len() == 0 {
            return Ok(args);
        }
        match serde_json::from_str::<Value>(json) {
            Ok(value) => {
                if let Some(object) = value.as_object() {
                    let method = self
                        .methods
                        .get(method)
                        .ok_or(UbusError::InvalidMethod(method.to_string()))?;
                    for (k, v) in object.into_iter() {
                        if let Some(arg_typ) = method.policy.get(k.as_str()) {
                            let mut builder =
                                BlobMsgBuilder::new_extended(arg_typ.value(), k.as_str());
                            match *arg_typ {
                                BlobMsgType::STRING => {
                                    if let Some(s) = v.as_str() {
                                        builder.push_str(s)?;
                                    }
                                }
                                BlobMsgType::INT64 => {
                                    if let Some(num) = v.as_i64() {
                                        builder.push_int64(num)?;
                                    }
                                }
                                BlobMsgType::INT32 => {
                                    if let Some(num) = v.as_i64() {
                                        builder.push_int32(num as i32)?;
                                    }
                                }
                                BlobMsgType::INT16 => {
                                    if let Some(num) = v.as_i64() {
                                        builder.push_int16(num as i16)?;
                                    }
                                }
                                BlobMsgType::INT8 => {
                                    if let Some(num) = v.as_i64() {
                                        builder.push_int8(num as i8)?;
                                    }
                                }
                                // BlobMsgType::BOOL => {
                                //     if let Some(b) = v.as_bool() {
                                //         builder.push_bool(b)?;
                                //     }
                                // }
                                BlobMsgType::DOUBLE => {
                                    if let Some(b) = v.as_f64() {
                                        builder.push_double(b)?;
                                    }
                                }
                                _ => continue,
                            }
                            args.extend_from_slice(builder.data())
                        } else {
                            continue;
                        }
                    }
                }
                Ok(args)
            }
            Err(e) => Err(UbusError::ParseArguments(e)),
        }
    }
}
