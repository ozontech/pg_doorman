// Standard library imports
use std::io::{BufRead, Cursor};

// External crate imports
use bytes::BytesMut;

// Internal crate imports
use crate::errors::Error;

/// Postgres data type mappings
/// used in RowDescription ('T') message.
pub enum DataType {
    Text,
    Int4,
    Numeric,
    Bool,
    Oid,
    AnyArray,
    Any,
}

impl From<&DataType> for i32 {
    fn from(data_type: &DataType) -> i32 {
        match data_type {
            DataType::Text => 25,
            DataType::Int4 => 23,
            DataType::Numeric => 1700,
            DataType::Bool => 16,
            DataType::Oid => 26,
            DataType::AnyArray => 2277,
            DataType::Any => 2276,
        }
    }
}

/// Trait for reading strings from BytesMut
pub trait BytesMutReader {
    fn read_string(&mut self) -> Result<String, Error>;
}

impl BytesMutReader for Cursor<&BytesMut> {
    /// Should only be used when reading strings from the message protocol.
    /// Can be used to read multiple strings from the same message which are separated by the null byte
    fn read_string(&mut self) -> Result<String, Error> {
        let mut buf = vec![];
        match self.read_until(b'\0', &mut buf) {
            Ok(_) => Ok(String::from_utf8_lossy(&buf[..buf.len() - 1]).to_string()),
            Err(err) => Err(Error::ParseBytesError(err.to_string())),
        }
    }
}

impl BytesMutReader for BytesMut {
    /// Should only be used when reading strings from the message protocol.
    /// Can be used to read multiple strings from the same message which are separated by the null byte
    fn read_string(&mut self) -> Result<String, Error> {
        let null_index = self.iter().position(|&byte| byte == b'\0');

        match null_index {
            Some(index) => {
                let string_bytes = self.split_to(index + 1);
                Ok(String::from_utf8_lossy(&string_bytes[..string_bytes.len() - 1]).to_string())
            }
            None => Err(Error::ParseBytesError("Could not read string".to_string())),
        }
    }
}

/// Convert a vector of bytes to a string.
pub fn vec_to_string(vec: Vec<u8>) -> Result<String, Error> {
    let vec_with_nul = match std::str::from_utf8(&vec) {
        Ok(token) => token,
        Err(err) => return Err(Error::ConvertError(err.to_string())),
    };
    match std::ffi::CStr::from_bytes_until_nul(vec_with_nul.as_ref()) {
        Ok(token) => Ok(token.to_str().unwrap().to_string()),
        Err(err) => Err(Error::ConvertError(err.to_string())),
    }
}
