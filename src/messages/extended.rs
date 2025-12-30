// Standard library imports
use std::collections::hash_map::DefaultHasher;
use std::ffi::CString;
use std::hash::Hasher;
use std::mem;
use std::sync::atomic::Ordering;
use std::sync::Arc;

// External crate imports
use bytes::{Buf, BufMut, BytesMut};
use xxhash_rust::xxh3::Xxh3;
use zerocopy::IntoBytes;
// Internal crate imports
use crate::client::PREPARED_STATEMENT_COUNTER;
use crate::errors::Error;
use crate::messages::types::BytesMutReader;

/// Extended protocol data enum for different message types.
pub enum ExtendedProtocolData {
    Parse {
        data: BytesMut,
        metadata: Option<(Arc<Parse>, u64)>,
    },
    Bind {
        data: BytesMut,
        hash: Option<u64>,
    },
    Describe {
        data: BytesMut,
        hash: Option<u64>,
    },
    Execute {
        data: BytesMut,
    },
    Close {
        data: BytesMut,
        close: Close,
    },
}

impl ExtendedProtocolData {
    pub fn create_new_parse(data: BytesMut, metadata: Option<(Arc<Parse>, u64)>) -> Self {
        Self::Parse { data, metadata }
    }

    pub fn create_new_bind(data: BytesMut, hash: Option<u64>) -> Self {
        Self::Bind { data, hash }
    }

    pub fn create_new_describe(data: BytesMut, hash: Option<u64>) -> Self {
        Self::Describe { data, hash }
    }

    pub fn create_new_execute(data: BytesMut) -> Self {
        Self::Execute { data }
    }

    pub fn create_new_close(data: BytesMut, close: Close) -> Self {
        Self::Close { data, close }
    }
}

/// Parse (F) message.
/// See: <https://www.postgresql.org/docs/current/protocol-message-formats.html>
#[derive(Clone, Debug)]
pub struct Parse {
    code: char,
    #[allow(dead_code)]
    len: i32,
    pub name: String,
    query: String,
    num_params: i16,
    param_types: Vec<i32>,
}

impl TryFrom<&BytesMut> for Parse {
    type Error = Error;

    fn try_from(buf: &BytesMut) -> Result<Parse, Error> {
        let mut cursor = std::io::Cursor::new(buf);
        let code = cursor.get_u8() as char;
        let len = cursor.get_i32();
        let name = cursor.read_string()?;
        let query = cursor.read_string()?;
        let num_params = cursor.get_i16();
        let mut param_types = Vec::new();

        for _ in 0..num_params {
            param_types.push(cursor.get_i32());
        }

        Ok(Parse {
            code,
            len,
            name,
            query,
            num_params,
            param_types,
        })
    }
}

impl TryFrom<Parse> for BytesMut {
    type Error = Error;

    fn try_from(parse: Parse) -> Result<BytesMut, Error> {
        let mut bytes = BytesMut::new();

        let name_binding = CString::new(parse.name)?;
        let name = name_binding.as_bytes_with_nul();

        let query_binding = CString::new(parse.query)?;
        let query = query_binding.as_bytes_with_nul();

        // Recompute length of the message.
        let len = 4 // self
            + name.len()
            + query.len()
            + 2
            + 4 * parse.num_params as usize;

        bytes.put_u8(parse.code as u8);
        bytes.put_i32(len as i32);
        bytes.put_slice(name);
        bytes.put_slice(query);
        bytes.put_i16(parse.num_params);
        for param in parse.param_types {
            bytes.put_i32(param);
        }

        Ok(bytes)
    }
}

impl TryFrom<&Parse> for BytesMut {
    type Error = Error;

    fn try_from(parse: &Parse) -> Result<BytesMut, Error> {
        parse.clone().try_into()
    }
}

impl Parse {
    /// Renames the prepared statement to a new name based on the global counter
    pub fn rewrite(mut self) -> Self {
        self.name = format!(
            "DOORMAN_{}",
            PREPARED_STATEMENT_COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        self
    }

    /// Gets the name of the prepared statement from the buffer
    pub fn get_name(buf: &BytesMut) -> Result<String, Error> {
        let mut cursor = std::io::Cursor::new(buf);
        // Skip the code and length
        cursor.advance(mem::size_of::<u8>() + mem::size_of::<i32>());
        cursor.read_string()
    }

    /// Hashes the parse statement to be used as a key in the global cache
    pub fn get_hash(&self) -> u64 {
        if self.query.len() >= 64 {
            let mut hasher = Xxh3::default();

            hasher.write(self.query.as_bytes());
            hasher.write_i16(self.num_params);
            hasher.write(self.param_types.as_slice().as_bytes());

            hasher.finish()
        } else {
            // in benchmarks default hasher was better on short strings.
            let mut hasher = DefaultHasher::new();

            hasher.write(self.query.as_bytes());
            hasher.write_i16(self.num_params);
            hasher.write(self.param_types.as_slice().as_bytes());

            hasher.finish()
        }
    }

    pub fn anonymous(&self) -> bool {
        self.name.is_empty()
    }
}

/// Bind (B) message.
/// See: <https://www.postgresql.org/docs/current/protocol-message-formats.html>
#[derive(Clone, Debug)]
pub struct Bind {
    code: char,
    #[allow(dead_code)]
    len: i64,
    portal: String,
    pub prepared_statement: String,
    num_param_format_codes: i16,
    param_format_codes: Vec<i16>,
    num_param_values: i16,
    param_values: Vec<(i32, BytesMut)>,
    num_result_column_format_codes: i16,
    result_columns_format_codes: Vec<i16>,
}

impl TryFrom<&BytesMut> for Bind {
    type Error = Error;

    fn try_from(buf: &BytesMut) -> Result<Bind, Error> {
        let mut cursor = std::io::Cursor::new(buf);
        let code = cursor.get_u8() as char;
        let len = cursor.get_i32();
        let portal = cursor.read_string()?;
        let prepared_statement = cursor.read_string()?;
        let num_param_format_codes = cursor.get_i16();
        let mut param_format_codes = Vec::new();

        for _ in 0..num_param_format_codes {
            param_format_codes.push(cursor.get_i16());
        }

        let num_param_values = cursor.get_i16();
        let mut param_values = Vec::new();

        for _ in 0..num_param_values {
            let param_len = cursor.get_i32();
            if param_len == -1 {
                param_values.push((-1, BytesMut::new()));
            } else {
                let mut param = BytesMut::with_capacity(param_len as usize);
                for _ in 0..param_len {
                    param.put_u8(cursor.get_u8());
                }
                param_values.push((param_len, param));
            }
        }

        let num_result_column_format_codes = cursor.get_i16();
        let mut result_columns_format_codes = Vec::new();

        for _ in 0..num_result_column_format_codes {
            result_columns_format_codes.push(cursor.get_i16());
        }

        Ok(Bind {
            code,
            len: len as i64,
            portal,
            prepared_statement,
            num_param_format_codes,
            param_format_codes,
            num_param_values,
            param_values,
            num_result_column_format_codes,
            result_columns_format_codes,
        })
    }
}

impl TryFrom<Bind> for BytesMut {
    type Error = Error;

    fn try_from(bind: Bind) -> Result<BytesMut, Error> {
        let mut bytes = BytesMut::new();

        let portal_binding = CString::new(bind.portal)?;
        let portal = portal_binding.as_bytes_with_nul();

        let prepared_statement_binding = CString::new(bind.prepared_statement)?;
        let prepared_statement = prepared_statement_binding.as_bytes_with_nul();

        let mut len = 4 // self
            + portal.len()
            + prepared_statement.len()
            + 2 // num_param_format_codes
            + 2 * bind.num_param_format_codes as usize // num_param_format_codes
            + 2; // num_param_values

        for (param_len, _) in &bind.param_values {
            len += 4 + *param_len as usize;
        }
        len += 2; // num_result_column_format_codes
        len += 2 * bind.num_result_column_format_codes as usize;

        bytes.put_u8(bind.code as u8);
        bytes.put_i32(len as i32);
        bytes.put_slice(portal);
        bytes.put_slice(prepared_statement);
        bytes.put_i16(bind.num_param_format_codes);
        for param_format_code in bind.param_format_codes {
            bytes.put_i16(param_format_code);
        }
        bytes.put_i16(bind.num_param_values);
        for (param_len, param) in bind.param_values {
            bytes.put_i32(param_len);
            bytes.put_slice(&param);
        }
        bytes.put_i16(bind.num_result_column_format_codes);
        for result_column_format_code in bind.result_columns_format_codes {
            bytes.put_i16(result_column_format_code);
        }

        Ok(bytes)
    }
}

impl Bind {
    /// Gets the name of the prepared statement from the buffer
    pub fn get_name(buf: &BytesMut) -> Result<String, Error> {
        let mut cursor = std::io::Cursor::new(buf);
        // Skip the code and length
        cursor.advance(mem::size_of::<u8>() + mem::size_of::<i32>());
        cursor.read_string()?;
        cursor.read_string()
    }

    /// Renames the prepared statement to a new name
    pub fn rename(buf: BytesMut, new_name: &str) -> Result<BytesMut, Error> {
        let mut cursor = std::io::Cursor::new(&buf);
        // Read basic data from the cursor
        let code = cursor.get_u8();
        let current_len = cursor.get_i32();
        let portal = cursor.read_string()?;
        let prepared_statement = cursor.read_string()?;

        // Calculate new length
        let new_len = current_len + new_name.len() as i32 - prepared_statement.len() as i32;

        // Begin building the response buffer
        let mut response_buf = BytesMut::with_capacity(new_len as usize + 1);
        response_buf.put_u8(code);
        response_buf.put_i32(new_len);

        // Put the portal and new name into the buffer
        // Note: panic if the provided string contains null byte
        response_buf.put_slice(CString::new(portal)?.as_bytes_with_nul());
        response_buf.put_slice(CString::new(new_name)?.as_bytes_with_nul());

        // Add the remainder of the original buffer into the response
        response_buf.put_slice(&buf[cursor.position() as usize..]);

        // Return the buffer
        Ok(response_buf)
    }

    pub fn anonymous(&self) -> bool {
        self.prepared_statement.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Describe {
    code: char,

    #[allow(dead_code)]
    len: i32,
    pub target: char,
    pub statement_name: String,
}

impl TryFrom<&BytesMut> for Describe {
    type Error = Error;

    fn try_from(bytes: &BytesMut) -> Result<Describe, Error> {
        let mut cursor = std::io::Cursor::new(bytes);
        let code = cursor.get_u8() as char;
        let len = cursor.get_i32();
        let target = cursor.get_u8() as char;
        let statement_name = cursor.read_string()?;

        Ok(Describe {
            code,
            len,
            target,
            statement_name,
        })
    }
}

impl TryFrom<Describe> for BytesMut {
    type Error = Error;

    fn try_from(describe: Describe) -> Result<BytesMut, Error> {
        let mut bytes = BytesMut::new();
        let statement_name_binding = CString::new(describe.statement_name)?;
        let statement_name = statement_name_binding.as_bytes_with_nul();
        let len = 4 + 1 + statement_name.len();

        bytes.put_u8(describe.code as u8);
        bytes.put_i32(len as i32);
        bytes.put_u8(describe.target as u8);
        bytes.put_slice(statement_name);

        Ok(bytes)
    }
}

impl Describe {
    pub fn empty_new() -> Describe {
        Describe {
            code: 'D',
            len: 4 + 1 + 1,
            target: 'S',
            statement_name: "".to_string(),
        }
    }

    pub fn rename(mut self, name: &str) -> Self {
        self.statement_name = name.to_string();
        self
    }

    pub fn anonymous(&self) -> bool {
        self.statement_name.is_empty()
    }
}

/// Close (F) message.
/// See: <https://www.postgresql.org/docs/current/protocol-message-formats.html>
#[derive(Clone, Debug)]
pub struct Close {
    code: char,
    #[allow(dead_code)]
    len: i32,
    close_type: char,
    pub name: String,
}

impl TryFrom<&BytesMut> for Close {
    type Error = Error;

    fn try_from(bytes: &BytesMut) -> Result<Close, Error> {
        let mut cursor = std::io::Cursor::new(bytes);
        let code = cursor.get_u8() as char;
        let len = cursor.get_i32();
        let close_type = cursor.get_u8() as char;
        let name = cursor.read_string()?;

        Ok(Close {
            code,
            len,
            close_type,
            name,
        })
    }
}

impl TryFrom<Close> for BytesMut {
    type Error = Error;

    fn try_from(close: Close) -> Result<BytesMut, Error> {
        let mut bytes = BytesMut::new();
        let name_binding = CString::new(close.name)?;
        let name = name_binding.as_bytes_with_nul();
        let len = 4 + 1 + name.len();

        bytes.put_u8(close.code as u8);
        bytes.put_i32(len as i32);
        bytes.put_u8(close.close_type as u8);
        bytes.put_slice(name);

        Ok(bytes)
    }
}

impl Close {
    pub fn new(name: &str) -> Close {
        let name = name.to_string();

        Close {
            code: 'C',
            len: 4 + 1 + name.len() as i32 + 1, // will be recalculated
            close_type: 'S',
            name,
        }
    }

    pub fn is_prepared_statement(&self) -> bool {
        self.close_type == 'S'
    }

    pub fn anonymous(&self) -> bool {
        self.name.is_empty()
    }
}

/// Create a CloseComplete message.
pub fn close_complete() -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'3');
    bytes.put_i32(4);
    bytes
}
