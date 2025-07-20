// Standard library imports
use std::collections::HashMap;
use std::mem;
// External crate imports
use bytes::{Buf, BufMut, BytesMut};
use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::constants::SCRAM_SHA_256;
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
pub fn notify(message: &str, details: String) -> BytesMut {
    let mut res = BytesMut::new();
    let mut notify = BytesMut::new();

    // Notification name
    notify.put_slice(message.as_bytes());
    notify.put_u8(0);

    // Process ID
    notify.put_i32(0);

    // Additional information
    notify.put_slice(details.as_bytes());
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
