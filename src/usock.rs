use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

impl IO for UnixStream {
    type Error = std::io::Error;
    fn put(&mut self, data: &[u8]) -> Result<(), UbusError> {
        self.write_all(data).map_err(UbusError::IO)
    }
    fn get(&mut self, data: &mut [u8]) -> Result<(), UbusError> {
        self.read_exact(data).map_err(UbusError::IO)
    }
}

impl Connection<UnixStream> {
    pub fn connect(path: &Path) -> Result<Self, UbusError> {
        Self::new(UnixStream::connect(path).map_err(UbusError::IO)?)
    }
}

impl IOError for std::io::Error {}
impl std::error::Error for Error {}
