// Standard library imports
use std::fmt::{Display, Formatter};
use std::str::FromStr;

// External crate imports
use bytes::{BufMut, BytesMut};

// Internal crate imports
use crate::errors::Error;
use crate::messages::extended::close_complete;
use crate::messages::parse_complete;

/// PostgreSQL error message structure.
/// See: https://www.postgresql.org/docs/12/protocol-error-fields.html
#[derive(Debug, Default, PartialEq)]
pub struct PgErrorMsg {
    pub severity_localized: String,      // S
    pub severity: String,                // V
    pub code: String,                    // C
    pub message: String,                 // M
    pub detail: Option<String>,          // D
    pub hint: Option<String>,            // H
    pub position: Option<u32>,           // P
    pub internal_position: Option<u32>,  // p
    pub internal_query: Option<String>,  // q
    pub where_context: Option<String>,   // W
    pub schema_name: Option<String>,     // s
    pub table_name: Option<String>,      // t
    pub column_name: Option<String>,     // c
    pub data_type_name: Option<String>,  // d
    pub constraint_name: Option<String>, // n
    pub file_name: Option<String>,       // F
    pub line: Option<u32>,               // L
    pub routine: Option<String>,         // R
}

impl Display for PgErrorMsg {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} [{}]",
            self.severity_localized, self.message, self.code
        )?;

        if let Some(val) = &self.detail {
            write!(f, "[detail: {val}]")?;
        }
        if let Some(val) = &self.hint {
            write!(f, "[hint: {val}]")?;
        }
        if let Some(val) = &self.position {
            write!(f, "[position: {val}]")?;
        }
        if let Some(val) = &self.internal_position {
            write!(f, "[internal_position: {val}]")?;
        }
        if let Some(val) = &self.internal_query {
            write!(f, "[internal_query: {val}]")?;
        }
        if let Some(val) = &self.where_context {
            write!(f, "[where: {val}]")?;
        }
        if let Some(val) = &self.schema_name {
            write!(f, "[schema_name: {val}]")?;
        }
        if let Some(val) = &self.table_name {
            write!(f, "[table_name: {val}]")?;
        }
        if let Some(val) = &self.column_name {
            write!(f, "[column_name: {val}]")?;
        }
        if let Some(val) = &self.data_type_name {
            write!(f, "[data_type_name: {val}]")?;
        }
        if let Some(val) = &self.constraint_name {
            write!(f, "[constraint_name: {val}]")?;
        }
        if let Some(val) = &self.file_name {
            write!(f, "[file_name: {val}]")?;
        }
        if let Some(val) = &self.line {
            write!(f, "[line: {val}]")?;
        }
        if let Some(val) = &self.routine {
            write!(f, "[routine: {val}]")?;
        }

        write!(f, " ")?;

        Ok(())
    }
}

impl PgErrorMsg {
    /// Parse a PostgreSQL error message from a byte array.
    pub fn parse(error_msg: &[u8]) -> Result<PgErrorMsg, Error> {
        let mut out = PgErrorMsg {
            severity_localized: "".to_string(),
            severity: "".to_string(),
            code: "".to_string(),
            message: "".to_string(),
            detail: None,
            hint: None,
            position: None,
            internal_position: None,
            internal_query: None,
            where_context: None,
            schema_name: None,
            table_name: None,
            column_name: None,
            data_type_name: None,
            constraint_name: None,
            file_name: None,
            line: None,
            routine: None,
        };

        let mut i = 0;
        while i < error_msg.len() {
            let field_type = error_msg[i];
            if field_type == 0 {
                break;
            }
            i += 1;

            let mut msg_content = String::new();
            while i < error_msg.len() && error_msg[i] != 0 {
                msg_content.push(error_msg[i] as char);
                i += 1;
            }
            i += 1;

            match field_type {
                b'S' => {
                    out.severity_localized = msg_content;
                }
                b'V' => {
                    out.severity = msg_content;
                }
                b'C' => {
                    out.code = msg_content;
                }
                b'M' => {
                    out.message = msg_content;
                }
                b'D' => {
                    out.detail = Some(msg_content);
                }
                b'H' => {
                    out.hint = Some(msg_content);
                }
                b'P' => out.position = Some(u32::from_str(msg_content.as_str()).unwrap_or(0)),
                b'p' => {
                    out.internal_position = Some(u32::from_str(msg_content.as_str()).unwrap_or(0))
                }
                b'q' => {
                    out.internal_query = Some(msg_content);
                }
                b'W' => {
                    out.where_context = Some(msg_content);
                }
                b's' => {
                    out.schema_name = Some(msg_content);
                }
                b't' => {
                    out.table_name = Some(msg_content);
                }
                b'c' => {
                    out.column_name = Some(msg_content);
                }
                b'd' => {
                    out.data_type_name = Some(msg_content);
                }
                b'n' => {
                    out.constraint_name = Some(msg_content);
                }
                b'F' => {
                    out.file_name = Some(msg_content);
                }
                b'L' => out.line = Some(u32::from_str(msg_content.as_str()).unwrap_or(0)),
                b'R' => {
                    out.routine = Some(msg_content);
                }
                _ => {}
            }
        }

        Ok(out)
    }
}

/// Reorder messages to ensure they are in the correct order.
pub fn set_messages_right_place(in_msg: Vec<u8>) -> Result<BytesMut, Error> {
    let in_msg_len = in_msg.len();
    let mut cursor = 0;
    let mut count_parse_complete = 0;
    let mut count_stmt_close = 0;
    let mut result = BytesMut::with_capacity(in_msg_len);

    // count parse message.
    loop {
        if cursor > in_msg_len {
            return Err(Error::ServerMessageParserError(
                "Cursor is more than total message size".to_string(),
            ));
        }
        if cursor == in_msg_len {
            break;
        }

        match in_msg[cursor] as char {
            '1' => count_parse_complete += 1,
            '3' => count_stmt_close += 1,
            _ => (),
        }

        cursor += 1;
        if cursor + 4 > in_msg_len {
            return Err(Error::ServerMessageParserError(
                "Can't read i32 from server message".to_string(),
            ));
        }
        let len_ref = match <[u8; 4]>::try_from(&in_msg[cursor..cursor + 4]) {
            Ok(len_ref) => len_ref,
            _ => {
                return Err(Error::ServerMessageParserError(
                    "Can't convert i32 from server message".to_string(),
                ))
            }
        };
        let mut len = i32::from_be_bytes(len_ref) as usize;
        if len < 4 {
            return Err(Error::ServerMessageParserError(
                "Message len less than 4".to_string(),
            ));
        }
        len -= 4;
        cursor += 4;
        if cursor + len > in_msg_len {
            return Err(Error::ServerMessageParserError(
                "Message len more than server message size".to_string(),
            ));
        }
        cursor += len;
    }
    if count_stmt_close == 0 && count_parse_complete == 0 {
        result.put(&in_msg[..]);
        return Ok(result);
    }

    cursor = 0;
    let mut prev_msg: char = ' ';
    loop {
        if cursor == in_msg_len {
            return Ok(result);
        }
        match in_msg[cursor] as char {
            '1' => {
                if count_parse_complete == 0 || prev_msg == '1' {
                    // ParseComplete: ignore.
                    cursor += 5;
                    continue;
                }
                count_parse_complete -= 1;
            }
            '2' | 't' => {
                if (prev_msg != '1') && (prev_msg != '2') && count_parse_complete > 0 {
                    // BindComplete, just add before ParseComplete.
                    result.put(parse_complete());
                    count_parse_complete -= 1;
                }
            }
            '3' => {
                if count_stmt_close == 1 {
                    cursor += 5;
                    continue;
                }
            }
            'Z' => {
                if count_stmt_close == 1 {
                    result.put(close_complete())
                }
            }
            _ => {}
        };
        prev_msg = in_msg[cursor] as char;
        cursor += 1; // code
        let len_ref = match <[u8; 4]>::try_from(&in_msg[cursor..cursor + 4]) {
            Ok(len_ref) => len_ref,
            _ => {
                return Err(Error::ServerMessageParserError(
                    "Can't convert i32 from server message".to_string(),
                ))
            }
        };
        let mut len = i32::from_be_bytes(len_ref) as usize;
        len -= 4;
        cursor += 4;
        result.put(&in_msg[cursor - 5..cursor + len]);
        cursor += len;
    }
}
