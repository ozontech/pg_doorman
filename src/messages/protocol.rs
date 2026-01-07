// Standard library imports
use std::collections::HashMap;
use std::mem;
// External crate imports
use crate::messages::constants::SCRAM_SHA_256;
use bytes::{Buf, BufMut, BytesMut};
use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
// Internal crate imports
use crate::errors::Error;
use crate::messages::socket::{write_all, write_all_flush};
use crate::messages::types::DataType;

/// Generate md5 password challenge.
pub async fn md5_challenge<S>(stream: &mut S) -> Result<[u8; 4], Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    // let mut rng = rand::thread_rng();
    let salt: [u8; 4] = [
        rand::random(),
        rand::random(),
        rand::random(),
        rand::random(),
    ];

    let mut res = BytesMut::new();
    res.put_u8(b'R');
    res.put_i32(12);
    res.put_i32(5); // MD5
    res.put_slice(&salt[..]);

    match stream.write_all(&res).await {
        Ok(_) => Ok(salt),
        Err(err) => Err(Error::SocketError(format!(
            "Failed to write MD5 challenge to socket: {err}"
        ))),
    }
}

/// Generate plain password challenge.
pub async fn plain_password_challenge<S>(stream: &mut S) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put_u8(b'R');
    res.put_i32(8);
    res.put_i32(3); // Plain password

    match stream.write_all(&res).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Failed to write plain password challenge to socket: {err}"
        ))),
    }
}

/// Generate SCRAM-SHA-256 challenge.
pub async fn scram_start_challenge<S>(stream: &mut S) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put_u8(b'R');
    res.put_i32(23);
    res.put_i32(10); // SCRAM-SHA-256
    res.put_slice(SCRAM_SHA_256.as_bytes());
    res.put_u8(0);
    res.put_u8(0);

    match stream.write_all(&res).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Failed to write SCRAM-SHA-256 challenge to socket: {err}"
        ))),
    }
}

/// Send SCRAM-SHA-256 server response.
pub async fn scram_server_response<S>(stream: &mut S, code: i32, data: &str) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put_u8(b'R');
    res.put_i32(4 + 4 + data.len() as i32);
    res.put_i32(code);
    res.put_slice(data.as_bytes());

    match stream.write_all(&res).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Failed to write SCRAM-SHA-256 server response to socket: {err}"
        ))),
    }
}

/// Read password from client.
pub async fn read_password<S>(stream: &mut S) -> Result<Vec<u8>, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let mut code = [0u8; 1];
    match stream.read_exact(&mut code).await {
        Ok(_) => {}
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to read password message type identifier: {err}"
            )))
        }
    }

    if code[0] != b'p' {
        return Err(Error::ProtocolSyncError(format!(
            "Protocol synchronization error: Expected password message (p), received '{}' instead",
            code[0] as char
        )));
    }

    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to read password message length: {err}"
            )))
        }
    }

    let len = i32::from_be_bytes(len_buf);
    let mut password = vec![0u8; (len - 4) as usize];
    match stream.read_exact(&mut password).await {
        Ok(_) => {}
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to read password message content: {err}"
            )))
        }
    }

    Ok(password)
}

/// Create a simple query message.
pub fn simple_query(query: &str) -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'Q');
    bytes.put_i32(4 + query.len() as i32 + 1);
    bytes.put_slice(query.as_bytes());
    bytes.put_u8(0);
    bytes
}

/// Send startup message to the server.
pub async fn startup<S>(
    stream: &mut S,
    user: String,
    database: &str,
    application_name: String,
) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut bytes = BytesMut::new();

    // Protocol version
    bytes.put_i32(196608); // Version 3.0

    // User
    bytes.put(&b"user\0"[..]);
    bytes.put_slice(user.as_bytes());
    bytes.put_u8(0);

    // Application name
    bytes.put(&b"application_name\0"[..]);
    bytes.put_slice(application_name.as_bytes());
    bytes.put_u8(0);

    // Database
    bytes.put(&b"database\0"[..]);
    bytes.put_slice(database.as_bytes());
    bytes.put_u8(0);
    bytes.put_u8(0); // Null terminator

    let len = bytes.len() as i32 + 4i32;

    let mut startup = BytesMut::with_capacity(len as usize);

    startup.put_i32(len);
    startup.put(bytes);

    match stream.write_all(&startup).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Failed to write startup message to server socket: {err}"
        ))),
    }
}

/// Send SSL request to the server.
pub async fn ssl_request(stream: &mut tokio::net::TcpStream) -> Result<(), Error> {
    let mut bytes = BytesMut::with_capacity(12);

    bytes.put_i32(8);
    bytes.put_i32(80877103);

    match stream.write_all(&bytes).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Failed to write SSL request to server socket: {err}"
        ))),
    }
}

/// Parse the params the server sends as a key/value format.
pub fn parse_params(mut bytes: BytesMut) -> Result<HashMap<String, String>, Error> {
    let mut result = HashMap::new();
    let mut buf = Vec::new();
    let mut tmp = String::new();

    while bytes.has_remaining() {
        let mut c = bytes.get_u8();

        // Null-terminated C-strings.
        while c != 0 {
            tmp.push(c as char);
            c = bytes.get_u8();
        }

        if !tmp.is_empty() {
            buf.push(tmp.clone());
            tmp.clear();
        }
    }

    // Expect pairs of name and value
    // and at least one pair to be present.
    if buf.len() % 2 != 0 || buf.len() < 2 {
        return Err(Error::ProtocolSyncError(format!(
            "Invalid client startup message: Expected key-value pairs, but received {} parameters",
            buf.len()
        )));
    }

    let mut i = 0;
    while i < buf.len() {
        let name = buf[i].clone();
        let value = buf[i + 1].clone();
        let _ = result.insert(name, value);
        i += 2;
    }

    Ok(result)
}

/// Parse StartupMessage parameters.
/// e.g. user, database, application_name, etc.
pub fn parse_startup(bytes: BytesMut) -> Result<HashMap<String, String>, Error> {
    let result = parse_params(bytes)?;

    // Minimum required parameters
    // I want to have the user at the very minimum, according to the protocol spec.
    if !result.contains_key("user") {
        return Err(Error::ClientBadStartup);
    }

    Ok(result)
}

/// Create md5 password hash given a salt.
pub fn md5_hash_password(user: &str, password: &str, salt: &[u8]) -> Vec<u8> {
    let mut md5 = Md5::new();

    // First pass
    md5.update(password.as_bytes());
    md5.update(user.as_bytes());

    let output = md5.finalize_reset();

    // Second pass
    md5_hash_second_pass(&(format!("{output:x}")), salt)
}

pub fn md5_hash_second_pass(hash: &str, salt: &[u8]) -> Vec<u8> {
    let mut md5 = Md5::new();
    // Second pass
    md5.update(hash);
    md5.update(salt);

    let mut password = format!("md5{:x}", md5.finalize())
        .chars()
        .map(|x| x as u8)
        .collect::<Vec<u8>>();
    password.push(0);

    password
}

/// Send password challenge response to the server.
/// This is the MD5 challenge.
pub async fn md5_password<S>(
    stream: &mut S,
    user: &str,
    password: &str,
    salt: &[u8],
) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let password = md5_hash_password(user, password, salt);

    let mut message = BytesMut::with_capacity(password.len() as usize + 5);

    message.put_u8(b'p');
    message.put_i32(password.len() as i32 + 4);
    message.put_slice(&password[..]);

    write_all(stream, message).await
}

pub async fn md5_password_with_hash<S>(stream: &mut S, hash: &str, salt: &[u8]) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let password = md5_hash_second_pass(hash, salt);
    let mut message = BytesMut::with_capacity(password.len() as usize + 5);

    message.put_u8(b'p');
    message.put_i32(password.len() as i32 + 4);
    message.put_slice(&password[..]);

    write_all(stream, message).await
}

pub async fn error_response<S>(stream: &mut S, message: &str, code: &str) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut buf = error_message(message, code);
    buf.put(ready_for_query(false));
    write_all_flush(stream, &buf).await
}

pub fn error_message(message: &str, code: &str) -> BytesMut {
    let mut error = BytesMut::new();
    // Error level
    error.put_u8(b'S');
    error.put_slice(&b"FATAL\0"[..]);
    // Error level (non-translatable)
    error.put_u8(b'V');
    error.put_slice(&b"FATAL\0"[..]);

    // Error code: not sure how much this matters.
    error.put_u8(b'C');
    error.put_slice(format!("{code}\0").as_bytes());

    // The short error message.
    error.put_u8(b'M');
    error.put_slice(format!("{message}\0").as_bytes());

    // No more fields follow.
    error.put_u8(0);

    // Compose the two message reply.
    let mut res = BytesMut::with_capacity(error.len() + 5);

    res.put_u8(b'E');
    res.put_i32(error.len() as i32 + 4);
    res.put(error);
    res
}

pub async fn error_response_terminal<S>(
    stream: &mut S,
    message: &str,
    code: &str,
) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let res = error_message(message, code);
    write_all_flush(stream, &res).await
}

pub async fn wrong_password<S>(stream: &mut S, user: &str) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut error = BytesMut::new();

    // Error level
    error.put_u8(b'S');
    error.put_slice(&b"FATAL\0"[..]);

    // Error level (non-translatable)
    error.put_u8(b'V');
    error.put_slice(&b"FATAL\0"[..]);

    // Error code: not sure how much this matters.
    error.put_u8(b'C');
    error.put_slice(&b"28P01\0"[..]); // system_error, see Appendix A.

    // The short error message.
    error.put_u8(b'M');
    error.put_slice(format!("password authentication failed for user \"{user}\"\0").as_bytes());

    // No more fields follow.
    error.put_u8(0);

    // Compose the two message reply.
    let mut res = BytesMut::new();

    res.put_u8(b'E');
    res.put_i32(error.len() as i32 + 4);

    res.put(error);

    write_all(stream, res).await
}

/// Create a row description message.
pub fn row_description(columns: &Vec<(&str, DataType)>) -> BytesMut {
    let mut res = BytesMut::new();
    let mut row_desc = BytesMut::new();

    // how many columns we are storing
    row_desc.put_i16(columns.len() as i16);

    for (name, data_type) in columns {
        // Column name
        row_desc.put_slice(format!("{name}\0").as_bytes());

        // Doesn't belong to any table
        row_desc.put_i32(0);

        // Doesn't belong to any table
        row_desc.put_i16(0);

        // Text
        row_desc.put_i32(data_type.into());

        // Text size = variable (-1)
        let type_size = match data_type {
            DataType::Text => -1,
            DataType::Int4 => 4,
            DataType::Numeric => -1,
            DataType::Bool => 1,
            DataType::Oid => 4,
            DataType::AnyArray => -1,
            DataType::Any => -1,
        };

        row_desc.put_i16(type_size);

        // Type modifier
        row_desc.put_i32(-1);

        // Text format code = 0
        row_desc.put_i16(0);
    }

    res.put_u8(b'T');
    res.put_i32(row_desc.len() as i32 + 4);
    res.put(row_desc);

    res
}

/// Create a data row message.
pub fn data_row(row: &Vec<String>) -> BytesMut {
    let mut res = BytesMut::new();
    let mut data_row = BytesMut::new();

    // how many columns we are storing
    data_row.put_i16(row.len() as i16);

    for value in row {
        // Column value
        data_row.put_i32(value.len() as i32);
        data_row.put_slice(value.as_bytes());
    }

    res.put_u8(b'D');
    res.put_i32(data_row.len() as i32 + 4);
    res.put(data_row);

    res
}

/// Create a data row message with nullable values.
pub fn data_row_nullable(row: &Vec<Option<String>>) -> BytesMut {
    let mut res = BytesMut::new();
    let mut data_row = BytesMut::new();

    // how many columns we are storing
    data_row.put_i16(row.len() as i16);

    for value in row {
        // Column value
        match value {
            Some(value) => {
                data_row.put_i32(value.len() as i32);
                data_row.put_slice(value.as_bytes());
            }
            None => {
                data_row.put_i32(-1);
            }
        }
    }

    res.put_u8(b'D');
    res.put_i32(data_row.len() as i32 + 4);
    res.put(data_row);

    res
}

/// Create a command complete message.
pub fn command_complete(command: &str) -> BytesMut {
    let mut res = BytesMut::new();
    res.put_u8(b'C');
    res.put_i32(command.len() as i32 + 4 + 1);
    res.put_slice(command.as_bytes());
    res.put_u8(0);
    res
}

/// Create a notification message.
/// NotificationResponse format (PostgreSQL protocol):
///   'A' (1 byte) + length (4 bytes) + process_id (4 bytes) + channel (null-terminated) + payload (null-terminated)
pub fn notify(channel: &str, payload: String) -> BytesMut {
    let mut res = BytesMut::new();
    let mut notify = BytesMut::new();

    // Process ID (4 bytes) - must be first
    notify.put_i32(0);

    // Channel name (null-terminated string)
    notify.put_slice(channel.as_bytes());
    notify.put_u8(0);

    // Payload (null-terminated string)
    notify.put_slice(payload.as_bytes());
    notify.put_u8(0);

    res.put_u8(b'A');
    res.put_i32(notify.len() as i32 + 4);
    res.put(notify);

    res
}

/// Create a flush message.
pub fn flush() -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'H');
    bytes.put_i32(4);
    bytes
}

/// Create a sync message.
pub fn sync() -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'S');
    bytes.put_i32(4);
    bytes
}

/// Create a parse complete message.
pub fn parse_complete() -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'1');
    bytes.put_i32(4);
    bytes
}

/// Create a check query response message.
pub fn check_query_response() -> BytesMut {
    let mut bytes = BytesMut::with_capacity(11);

    bytes.put_u8(b'I');
    bytes.put_i32(mem::size_of::<i32>() as i32);
    bytes.put_u8(b'Z');
    bytes.put_i32(mem::size_of::<i32>() as i32 + 1);
    bytes.put_u8(b'I');
    bytes
}

/// Create a deallocate response message.
pub fn deallocate_response() -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put(parse_complete());
    bytes.put(command_complete("DEALLOCATE"));
    bytes.put(ready_for_query(false));
    bytes
}

/// Create a ready for query message.
pub fn ready_for_query(in_transaction: bool) -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'Z');
    bytes.put_i32(5);
    if in_transaction {
        bytes.put_u8(b'T');
    } else {
        bytes.put_u8(b'I');
    }

    bytes
}

/// Create a server parameter message.
pub fn server_parameter_message(key: &str, value: &str) -> BytesMut {
    let mut server_info = BytesMut::new();
    server_info.put_u8(b'S');
    server_info.put_i32(4 + key.len() as i32 + 1 + value.len() as i32 + 1);
    server_info.put_slice(key.as_bytes());
    server_info.put_bytes(0, 1);
    server_info.put_slice(value.as_bytes());
    server_info.put_bytes(0, 1);

    server_info
}

/// Insert ParseComplete messages before BindComplete messages that don't already have one.
/// This ensures proper message ordering in the PostgreSQL extended protocol.
/// Returns (modified buffer, number of ParseComplete messages inserted).
pub fn insert_parse_complete_before_bind_complete(buffer: BytesMut, count: u32) -> (BytesMut, u32) {
    if count == 0 {
        return (buffer, 0);
    }

    const PARSE_COMPLETE_SIZE: usize = 5; // '1' (1) + length (4)
    const PARSE_COMPLETE_MSG: [u8; 5] = [b'1', 0, 0, 0, 4];

    let bytes = buffer.as_ref();
    let len = bytes.len();

    // Fast path: count=1, find first position without Vec allocation
    if count == 1 {
        let mut pos = 0;
        let mut prev_msg_type: u8 = 0;

        while pos < len {
            if pos + 5 > len {
                break;
            }
            let msg_type = bytes[pos];
            let msg_len = i32::from_be_bytes([
                bytes[pos + 1],
                bytes[pos + 2],
                bytes[pos + 3],
                bytes[pos + 4],
            ]) as usize;

            if msg_type == b'2' && prev_msg_type != b'1' {
                // Found position for insertion
                let mut result = BytesMut::with_capacity(len + PARSE_COMPLETE_SIZE);
                result.extend_from_slice(&bytes[..pos]);
                result.extend_from_slice(&PARSE_COMPLETE_MSG);
                result.extend_from_slice(&bytes[pos..]);
                return (result, 1);
            }

            prev_msg_type = msg_type;
            pos += 1 + msg_len;
        }
        return (buffer, 0);
    }

    // Slow path: multiple insertions
    // Use stack-allocated array for small counts to avoid heap allocation
    let mut bind_positions_stack: [usize; 8] = [0; 8];
    let mut bind_positions_heap: Vec<usize> = Vec::new();
    let mut num_positions = 0;
    let mut pos = 0;
    let mut prev_msg_type: u8 = 0;

    // Lazy parsing: stop after finding 'count' positions
    while pos < len && num_positions < count as usize {
        if pos + 5 > len {
            break;
        }
        let msg_type = bytes[pos];
        let msg_len = i32::from_be_bytes([
            bytes[pos + 1],
            bytes[pos + 2],
            bytes[pos + 3],
            bytes[pos + 4],
        ]) as usize;

        if msg_type == b'2' && prev_msg_type != b'1' {
            // BindComplete without preceding ParseComplete
            if num_positions < 8 {
                bind_positions_stack[num_positions] = pos;
            } else {
                if bind_positions_heap.is_empty() {
                    bind_positions_heap.reserve(count as usize - 8);
                }
                bind_positions_heap.push(pos);
            }
            num_positions += 1;
        }

        prev_msg_type = msg_type;
        pos += 1 + msg_len;
    }

    if num_positions == 0 {
        return (buffer, 0);
    }

    // Single allocation with exact size
    let mut result = BytesMut::with_capacity(len + PARSE_COMPLETE_SIZE * num_positions);
    let mut last_pos = 0;

    for i in 0..num_positions {
        let bind_pos = if i < 8 {
            bind_positions_stack[i]
        } else {
            bind_positions_heap[i - 8]
        };

        result.extend_from_slice(&bytes[last_pos..bind_pos]);
        result.extend_from_slice(&PARSE_COMPLETE_MSG);
        last_pos = bind_pos;
    }
    result.extend_from_slice(&bytes[last_pos..]);

    (result, num_positions as u32)
}

/// Insert CloseComplete messages after the last CloseComplete from server.
/// If no CloseComplete found, insert before ReadyForQuery.
/// Returns (modified_buffer, inserted_count).
pub fn insert_close_complete_after_last_close_complete(
    buffer: BytesMut,
    count: u32,
) -> (BytesMut, u32) {
    if count == 0 {
        return (buffer, 0);
    }

    const CLOSE_COMPLETE_SIZE: usize = 5; // '3' (1) + length (4)
    const CLOSE_COMPLETE_MSG: [u8; 5] = [b'3', 0, 0, 0, 4];
    const READY_FOR_QUERY_SIZE: usize = 6; // 'Z' (1) + length (4) + status (1)

    let bytes = buffer.as_ref();
    let mut pos = 0;
    let mut last_close_complete_pos: Option<usize> = None;

    // Find the last CloseComplete ('3') message
    while pos < bytes.len() {
        if pos + 5 > bytes.len() {
            break;
        }

        let msg_type = bytes[pos];
        let msg_len = i32::from_be_bytes([
            bytes[pos + 1],
            bytes[pos + 2],
            bytes[pos + 3],
            bytes[pos + 4],
        ]) as usize;

        if msg_type == b'3' {
            last_close_complete_pos = Some(pos + 1 + 4 + (msg_len - 4));
        }

        pos += 1 + 4 + (msg_len - 4);
    }

    let insert_size = CLOSE_COMPLETE_SIZE * count as usize;
    let mut result = BytesMut::with_capacity(buffer.len() + insert_size);

    if let Some(insert_pos) = last_close_complete_pos {
        // Insert after last CloseComplete
        result.extend_from_slice(&bytes[..insert_pos]);
        for _ in 0..count {
            result.extend_from_slice(&CLOSE_COMPLETE_MSG);
        }
        result.extend_from_slice(&bytes[insert_pos..]);
        (result, count)
    } else {
        // No CloseComplete found, insert before ReadyForQuery if present
        let len = bytes.len();
        if len >= READY_FOR_QUERY_SIZE && bytes[len - READY_FOR_QUERY_SIZE] == b'Z' {
            result.extend_from_slice(&bytes[..len - READY_FOR_QUERY_SIZE]);
            for _ in 0..count {
                result.extend_from_slice(&CLOSE_COMPLETE_MSG);
            }
            result.extend_from_slice(&bytes[len - READY_FOR_QUERY_SIZE..]);
            (result, count)
        } else {
            // No ReadyForQuery either, return unchanged
            (buffer, 0)
        }
    }
}

/// Insert CloseComplete messages before ReadyForQuery in the response buffer.
/// This ensures proper message ordering in the PostgreSQL extended protocol.
pub fn insert_close_complete_before_ready_for_query(mut buffer: BytesMut, count: u32) -> BytesMut {
    if count == 0 {
        return buffer;
    }

    const READY_FOR_QUERY_SIZE: usize = 6; // 'Z' (1) + length (4) + status (1)
    const CLOSE_COMPLETE_SIZE: usize = 5; // '3' (1) + length (4)
    const CLOSE_COMPLETE_MSG: [u8; 5] = [b'3', 0, 0, 0, 4];

    let insert_size = CLOSE_COMPLETE_SIZE * count as usize;
    let len = buffer.len();

    // Check if ReadyForQuery is at the end
    if len >= READY_FOR_QUERY_SIZE && buffer[len - READY_FOR_QUERY_SIZE] == b'Z' {
        // Try in-place modification if we have enough capacity
        if buffer.capacity() - len >= insert_size {
            // SAFETY: We have verified that:
            // 1. buffer has enough capacity for the insertion
            // 2. We're moving non-overlapping memory regions (ReadyForQuery moves right)
            // 3. We update the length after all writes are complete
            unsafe {
                let ptr = buffer.as_mut_ptr();
                let rfq_start = len - READY_FOR_QUERY_SIZE;

                // Move ReadyForQuery to the right to make space for CloseComplete messages
                std::ptr::copy(
                    ptr.add(rfq_start),
                    ptr.add(rfq_start + insert_size),
                    READY_FOR_QUERY_SIZE,
                );

                // Write CloseComplete messages in the gap
                let mut offset = rfq_start;
                for _ in 0..count {
                    std::ptr::copy_nonoverlapping(
                        CLOSE_COMPLETE_MSG.as_ptr(),
                        ptr.add(offset),
                        CLOSE_COMPLETE_SIZE,
                    );
                    offset += CLOSE_COMPLETE_SIZE;
                }

                // Update buffer length
                buffer.set_len(len + insert_size);
            }
            return buffer;
        }

        // Fallback: copy with optimization
        let mut result = BytesMut::with_capacity(len + insert_size);
        result.extend_from_slice(&buffer[..len - READY_FOR_QUERY_SIZE]);

        // Insert CloseComplete messages
        for _ in 0..count {
            result.extend_from_slice(&CLOSE_COMPLETE_MSG);
        }

        result.extend_from_slice(&buffer[len - READY_FOR_QUERY_SIZE..]);
        result
    } else {
        // No ReadyForQuery found, append CloseComplete at the end
        buffer.reserve(insert_size);
        for _ in 0..count {
            buffer.extend_from_slice(&CLOSE_COMPLETE_MSG);
        }
        buffer
    }
}

