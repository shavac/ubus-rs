use super::*;
use alloc::vec::Vec;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;

fn map_io_err(e: std::io::Error) -> UbusError {
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => UbusError::Timeout,
        _ => UbusError::IO(e),
    }
}

impl IO for UnixStream {
    type Error = std::io::Error;
    fn put(&mut self, data: &[u8]) -> Result<(), UbusError> {
        self.write_all(data).map_err(map_io_err)
    }
    fn get(&mut self, data: &mut [u8]) -> Result<(), UbusError> {
        self.read_exact(data).map_err(map_io_err)
    }
    fn set_read_timeout(
        &mut self,
        timeout: Option<core::time::Duration>,
    ) -> Result<(), UbusError> {
        UnixStream::set_read_timeout(self, timeout).map_err(UbusError::IO)
    }
    fn set_write_timeout(
        &mut self,
        timeout: Option<core::time::Duration>,
    ) -> Result<(), UbusError> {
        UnixStream::set_write_timeout(self, timeout).map_err(UbusError::IO)
    }
}

impl Connection<UnixStream> {
    pub fn connect(path: &Path) -> Result<Self, UbusError> {
        Self::new(UnixStream::connect(path)?)
    }

    /// Toggle non-blocking mode on the underlying Unix socket for integration with `epoll`/`mio`/`tokio`.
    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<(), UbusError> {
        self.io().set_nonblocking(nonblocking).map_err(UbusError::IO)
    }

    /// Reconnect to the Unix socket at `path` and perform a fresh `HELLO` handshake.
    pub fn reconnect(&mut self, path: &Path) -> Result<(), UbusError> {
        self.reconnect_with(UnixStream::connect(path)?)
    }

    /// Reconnect to `path` and re-register all objects listed in `objects`.
    pub fn reconnect_and_restore_path(
        &mut self,
        path: &Path,
        objects: &[ObjectSpec<'_>],
    ) -> Result<Vec<RegisteredObject>, UbusError> {
        self.reconnect_and_restore(UnixStream::connect(path)?, objects)
    }
}

impl AsRawFd for Connection<UnixStream> {
    fn as_raw_fd(&self) -> RawFd {
        self.io().as_raw_fd()
    }
}

impl AsFd for Connection<UnixStream> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.io().as_fd()
    }
}

impl IOError for std::io::Error {}
impl std::error::Error for Error {}
