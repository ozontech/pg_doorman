use crate::pg_connection::PgConnection;
use std::collections::HashMap;

/// Get a mutable reference to a named session, panicking with a clear message if not found.
pub(crate) fn get_session<'a>(
    named_sessions: &'a mut HashMap<String, PgConnection>,
    session_name: &str,
) -> &'a mut PgConnection {
    named_sessions
        .get_mut(session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name))
}

/// Parse the first field of a DataRow message as an integer.
///
/// DataRow format:
///   Int16 - number of fields
///   For each field:
///     Int32 - field length (-1 for NULL)
///     Byte*n - field value (text format)
///
/// Returns `Some(value)` if the DataRow has at least one non-NULL field
/// that can be parsed as i32, or `None` otherwise.
pub(crate) fn parse_first_datarow_int(data: &[u8]) -> Option<i32> {
    if data.len() < 6 {
        return None;
    }
    let field_count = i16::from_be_bytes([data[0], data[1]]);
    if field_count < 1 {
        return None;
    }
    let field_len = i32::from_be_bytes([data[2], data[3], data[4], data[5]]);
    if field_len <= 0 {
        return None;
    }
    let end = 6 + field_len as usize;
    if data.len() < end {
        return None;
    }
    let value_str = String::from_utf8_lossy(&data[6..end]);
    value_str.parse::<i32>().ok()
}

/// Parse all fields from a DataRow message as strings.
///
/// DataRow format:
///   Int16 - number of fields
///   For each field:
///     Int32 - field length (-1 for NULL)
///     Byte*n - field value (text format)
///
/// Returns a Vec of field values. NULL fields are represented as the string "NULL".
pub(crate) fn parse_datarow_fields(data: &[u8]) -> Vec<String> {
    let mut fields = Vec::new();
    if data.len() < 2 {
        return fields;
    }
    let field_count = i16::from_be_bytes([data[0], data[1]]) as usize;
    let mut pos = 2;
    for _ in 0..field_count {
        if pos + 4 > data.len() {
            break;
        }
        let field_len =
            i32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;
        if field_len < 0 {
            fields.push("NULL".to_string());
        } else {
            let end = pos + field_len as usize;
            if end > data.len() {
                break;
            }
            let value = String::from_utf8_lossy(&data[pos..end]).into_owned();
            fields.push(value);
            pos = end;
        }
    }
    fields
}

/// Parse comma-separated params string into bind parameter format.
pub(crate) fn parse_bind_params(params_str: &str) -> Vec<Option<Vec<u8>>> {
    if params_str.is_empty() {
        vec![]
    } else {
        params_str
            .split(',')
            .map(|s| Some(s.trim().as_bytes().to_vec()))
            .collect()
    }
}

/// Extract the stored admin response string from session_messages.
pub(crate) fn get_admin_response(
    session_messages: &std::collections::HashMap<String, Vec<(char, Vec<u8>)>>,
    session_name: &str,
) -> String {
    let messages = session_messages
        .get(session_name)
        .unwrap_or_else(|| panic!("No response stored for session '{}'", session_name));

    if let Some((_, data)) = messages.first() {
        String::from_utf8_lossy(data).to_string()
    } else {
        panic!("No response content for session '{}'", session_name);
    }
}

/// Parse a table response into headers and find a column index by name.
/// Supports both pipe-separated ("col1|col2") and whitespace-separated formats.
pub(crate) fn find_column_index(header_line: &str, column_name: &str) -> (usize, bool) {
    let use_pipe = header_line.contains('|');
    let headers: Vec<&str> = if use_pipe {
        header_line.split('|').map(|s| s.trim()).collect()
    } else {
        header_line.split_whitespace().collect()
    };

    let idx = headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case(column_name))
        .unwrap_or_else(|| {
            panic!(
                "Column '{}' not found in headers: {:?}",
                column_name, headers
            )
        });

    (idx, use_pipe)
}

/// Split a table row using the detected separator.
pub(crate) fn split_row(line: &str, use_pipe: bool) -> Vec<&str> {
    if use_pipe {
        line.split('|').map(|s| s.trim()).collect()
    } else {
        line.split_whitespace().collect()
    }
}

/// Format message details for debugging output.
///
/// Produces a human-readable summary of a PostgreSQL protocol message,
/// including type-specific field decoding.
pub(crate) fn format_message_details(msg_type: char, data: &[u8]) -> String {
    let mut details = format!("type='{}' len={}", msg_type, data.len());

    match msg_type {
        'R' => {
            // Authentication request
            if data.len() >= 4 {
                let auth_type = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                details.push_str(&format!(" [AuthenticationRequest type={}]", auth_type));
            }
        }
        'S' => {
            // ParameterStatus: name\0value\0
            if let Some(null_pos) = data.iter().position(|&b| b == 0) {
                let name = String::from_utf8_lossy(&data[..null_pos]);
                let value = String::from_utf8_lossy(
                    data[null_pos + 1..]
                        .split(|&b| b == 0)
                        .next()
                        .unwrap_or(&[]),
                );
                details.push_str(&format!(" [ParameterStatus {}={}]", name, value));
            }
        }
        'K' => {
            // BackendKeyData: process_id(4) + secret_key(4)
            if data.len() >= 8 {
                let pid = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let key = i32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                details.push_str(&format!(" [BackendKeyData pid={} key={}]", pid, key));
            }
        }
        'Z' => {
            // ReadyForQuery: status(1)
            if !data.is_empty() {
                let status = match data[0] {
                    b'I' => "Idle",
                    b'T' => "InTransaction",
                    b'E' => "FailedTransaction",
                    _ => "Unknown",
                };
                details.push_str(&format!(" [ReadyForQuery status={}]", status));
            }
        }
        'T' => {
            // RowDescription
            if data.len() >= 2 {
                let field_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [RowDescription fields={}]", field_count));
            }
        }
        'D' => {
            // DataRow
            if data.len() >= 2 {
                let field_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [DataRow fields={}]", field_count));
            }
        }
        'C' => {
            // CommandComplete: tag\0
            if let Some(null_pos) = data.iter().position(|&b| b == 0) {
                let tag = String::from_utf8_lossy(&data[..null_pos]);
                details.push_str(&format!(" [CommandComplete tag='{}']", tag));
            }
        }
        'E' => {
            // ErrorResponse: parse fields
            details.push_str(" [ErrorResponse");
            let mut pos = 0;
            while pos < data.len() {
                let field_type = data[pos] as char;
                if field_type == '\0' {
                    break;
                }
                pos += 1;
                if let Some(null_pos) = data[pos..].iter().position(|&b| b == 0) {
                    let value = String::from_utf8_lossy(&data[pos..pos + null_pos]);
                    match field_type {
                        'S' => details.push_str(&format!(" severity={}", value)),
                        'C' => details.push_str(&format!(" code={}", value)),
                        'M' => details.push_str(&format!(" message={}", value)),
                        _ => {}
                    }
                    pos += null_pos + 1;
                } else {
                    break;
                }
            }
            details.push(']');
        }
        'N' => {
            // NoticeResponse: similar to ErrorResponse
            details.push_str(" [NoticeResponse");
            let mut pos = 0;
            while pos < data.len() {
                let field_type = data[pos] as char;
                if field_type == '\0' {
                    break;
                }
                pos += 1;
                if let Some(null_pos) = data[pos..].iter().position(|&b| b == 0) {
                    let value = String::from_utf8_lossy(&data[pos..pos + null_pos]);
                    match field_type {
                        'S' => details.push_str(&format!(" severity={}", value)),
                        'C' => details.push_str(&format!(" code={}", value)),
                        'M' => details.push_str(&format!(" message={}", value)),
                        _ => {}
                    }
                    pos += null_pos + 1;
                } else {
                    break;
                }
            }
            details.push(']');
        }
        '1' => {
            // ParseComplete
            details.push_str(" [ParseComplete]");
        }
        '2' => {
            // BindComplete
            details.push_str(" [BindComplete]");
        }
        't' => {
            // ParameterDescription
            if data.len() >= 2 {
                let param_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [ParameterDescription params={}]", param_count));
            }
        }
        'n' => {
            // NoData
            details.push_str(" [NoData]");
        }
        's' => {
            // PortalSuspended
            details.push_str(" [PortalSuspended]");
        }
        _ => {
            // Unknown message type, show first 32 bytes as hex
            let preview_len = data.len().min(32);
            let hex_preview: String = data[..preview_len]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");
            details.push_str(&format!(
                " [data: {}{}]",
                hex_preview,
                if data.len() > 32 { "..." } else { "" }
            ));
        }
    }

    details
}

/// Normalize RowDescription message by zeroing out table OIDs.
///
/// RowDescription format:
///   Int16 - number of fields
///   For each field:
///     String - field name (null-terminated)
///     Int32 - table OID (if from a table, else 0) <- we zero this
///     Int16 - column attribute number
///     Int32 - data type OID
///     Int16 - data type size
///     Int32 - type modifier
///     Int16 - format code
pub(crate) fn normalize_row_description(data: &[u8]) -> Vec<u8> {
    let mut result = data.to_vec();
    if data.len() < 2 {
        return result;
    }

    let field_count = i16::from_be_bytes([data[0], data[1]]) as usize;
    let mut pos = 2;

    for _ in 0..field_count {
        // Skip field name (null-terminated string)
        while pos < result.len() && result[pos] != 0 {
            pos += 1;
        }
        pos += 1; // skip null terminator

        // Zero out table OID (4 bytes)
        if pos + 4 <= result.len() {
            result[pos] = 0;
            result[pos + 1] = 0;
            result[pos + 2] = 0;
            result[pos + 3] = 0;
        }
        pos += 4; // table OID

        pos += 2; // column attribute number
        pos += 4; // data type OID
        pos += 2; // data type size
        pos += 4; // type modifier
        pos += 2; // format code
    }

    result
}
