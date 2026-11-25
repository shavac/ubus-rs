use alloc::string::String;
use core::str::Utf8Error;
use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UbusError {
    #[error("io error: {0}")]
    IO(#[from] io::Error),
    #[error("Invalid decoding string: {0}")]
    Utf8(#[from] Utf8Error),
    #[error("Invalid Data: {0}")]
    InvalidData(&'static str),
    #[error("Ubus return ErrorCode({0})")]
    Status(i32),
    #[cfg(feature = "json")]
    #[error("Error parse arguments string:{0}")]
    ParseArguments(#[from] serde_json::Error),
    #[error("Invalid method:{0}")]
    InvalidMethod(String),
    #[error("Serde deserialization error: {0}")]
    Deserialize(String),
}

impl serde::de::Error for UbusError {
    fn custom<T: core::fmt::Display>(msg: T) -> Self {
        UbusError::Deserialize(alloc::string::ToString::to_string(&msg))
    }
}
