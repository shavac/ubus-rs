#![no_std]

extern crate alloc;
#[cfg(not(feature = "no_std"))]
extern crate std;

/// Macro for defining helpful enum-like opaque structs
macro_rules! values {
    (
        $vis:vis $name:ident ( $repr:ty ) {
            $( $variant:ident = $value:literal ),* $(,)?
        }
    ) => {
        #[repr(transparent)]
        #[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        $vis struct $name($repr);
        impl $name {
            $( pub const $variant: Self = Self($value); )*
            pub const fn known(self) -> bool {
                matches!(self, $( Self::$variant )|*)
            }
            pub const fn value(self) -> $repr {
                self.0
            }
        }
        impl From<$repr> for $name {
            fn from(other: $repr) -> Self {
                Self(other)
            }
        }
        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                let name = match *self {
                    $( Self::$variant => stringify!($variant), )*
                    unknown => return write!(f, "UNKNOWN({})", unknown.0),
                };
                f.write_str(name)
            }
        }
    };
}

macro_rules! invalid_data_panic {
    ($($arg:tt)*) => (if cfg!(debug_assertions) { panic!($($arg)*); })
}

macro_rules! valid_data {
    ($thing:expr, $msg:literal) => {{
        if !($thing) {
            invalid_data_panic!("Invalid Data: {}", $msg);
            return Err(crate::UbusError::InvalidData($msg));
        }
    }};
}

#[derive(Debug)]
pub enum NoIO {}
impl core::fmt::Display for NoIO {
    fn fmt(&self, _f: &mut core::fmt::Formatter) -> core::fmt::Result {
        unreachable!()
    }
}

#[derive(Debug)]
pub enum Error<T = NoIO> {
    IO(T),
    InvalidData(&'static str),
    Status(i32),
}

impl<T: core::fmt::Display> core::fmt::Display for Error<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Self::IO(e) => write!(f, "IO Error: {e}"),
            Self::InvalidData(e) => write!(f, "Invalid Data: {e}"),
            Self::Status(e) => write!(f, "Ubus Status: {e}"),
        }
    }
}

impl<T: IOError> From<Error<NoIO>> for Error<T> {
    fn from(e: Error<NoIO>) -> Self {
        match e {
            Error::IO(never) => match never {},
            Error::InvalidData(v) => Self::InvalidData(v),
            Error::Status(v) => Self::Status(v),
        }
    }
}

impl<T> From<core::convert::Infallible> for Error<T> {
    fn from(never: core::convert::Infallible) -> Self {
        match never {}
    }
}

pub trait IOError {}

pub trait IO {
    type Error: IOError;
    fn put(&mut self, data: &[u8]) -> Result<(), UbusError>;
    fn get(&mut self, data: &mut [u8]) -> Result<(), UbusError>;
    fn set_read_timeout(
        &mut self,
        _timeout: Option<core::time::Duration>,
    ) -> Result<(), UbusError> {
        Ok(())
    }
    fn set_write_timeout(
        &mut self,
        _timeout: Option<core::time::Duration>,
    ) -> Result<(), UbusError> {
        Ok(())
    }
}

mod blob;
mod blobmsg;
mod connection;
mod serde_blob;
mod ubuserror;
mod ubusmsg;
mod ubusobj;
mod usock;

pub use blob::*;
pub use blobmsg::*;
pub use connection::*;
pub use serde_blob::*;
pub use ubuserror::*;
pub(crate) use ubuserror::OptionInvalidExt;
pub use ubusmsg::*;
pub use ubusobj::*;
