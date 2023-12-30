extern crate alloc;
use core::str::Utf8Error;
use std::io;

use alloc::string::String;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UbusError {
    #[error("io error")]
    IO(#[from] io::Error),
    #[error("Invalid decoding string")]
    Utf8(#[from] Utf8Error),
    #[error("Invalid Data")]
    InvalidData(&'static str),
    #[error("Ubus return ErrorCode({0})")]
    Status(i32),
    #[error("Error parse arguments string:{0}")]
    ParseArguments(#[from] serde_json::Error),
    #[error("Invalid method:{0}")]
    InvalidMethod(String),
}
