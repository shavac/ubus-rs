use alloc::string::{String, ToString};
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
    #[error("Ubus status error ({0}: {})", crate::UbusStatus::from(*.0))]
    Status(i32),
    #[error("Request timed out")]
    Timeout,
    #[cfg(feature = "json")]
    #[error("Error parse arguments string:{0}")]
    ParseArguments(#[from] serde_json::Error),
    #[error("Invalid method:{0}")]
    InvalidMethod(String),
    #[error("Serde deserialization error: {0}")]
    Deserialize(String),
    #[error("Serde serialization error: {0}")]
    Serialize(String),
}

impl UbusError {
    /// Convert a ubus status or timeout error to its [`UbusStatus`], if applicable.
    pub fn status(&self) -> Option<crate::UbusStatus> {
        match self {
            UbusError::Status(code) => Some(crate::UbusStatus::from(*code)),
            UbusError::Timeout => Some(crate::UbusStatus::TIMEOUT),
            _ => None,
        }
    }
}

pub(crate) trait OptionInvalidExt<T> {
    fn or_invalid(self, msg: &'static str) -> Result<T, UbusError>;
}

impl<T> OptionInvalidExt<T> for Option<T> {
    #[inline]
    fn or_invalid(self, msg: &'static str) -> Result<T, UbusError> {
        self.map_or_else(|| Err(UbusError::InvalidData(msg)), Ok)
    }
}

impl serde::de::Error for UbusError {
    fn custom<T: core::fmt::Display>(msg: T) -> Self {
        UbusError::Deserialize(msg.to_string())
    }
}

impl serde::ser::Error for UbusError {
    fn custom<T: core::fmt::Display>(msg: T) -> Self {
        UbusError::Serialize(msg.to_string())
    }
}
