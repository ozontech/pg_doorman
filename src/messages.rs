/// Helper functions to send one-off protocol messages
/// and handle TcpStream (TCP socket).
use bytes::{Buf, BufMut, BytesMut};
use log::error;
use md5::{Digest, Md5};
use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};

use crate::client::PREPARED_STATEMENT_COUNTER;
use crate::config::get_config;
use crate::errors::Error;

use crate::constants::{AUTHENTICATION_CLEAR_PASSWORD, MESSAGE_TERMINATOR, SCRAM_SHA_256};
use crate::errors::Error::ProxyTimeout;
use once_cell::sync::Lazy;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::CString;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Cursor};
use std::mem;
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

// сообщения больше этого размера не будут обрабатываться.
pub const MAX_MESSAGE_SIZE: i32 = 256 * 1024 * 1024;

// суммарный объем памяти, приблизительно находящийся в обработке.
pub static CURRENT_MEMORY: Lazy<Arc<AtomicI64>> = Lazy::new(|| Arc::new(AtomicI64::new(0)));

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

/// Tell the client that authentication handshake completed successfully.
pub async fn auth_ok<S>(stream: &mut S) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut auth_ok = BytesMut::with_capacity(9);

    auth_ok.put_u8(b'R');
    auth_ok.put_i32(8);
    auth_ok.put_i32(0);

    write_all(stream, auth_ok).await
}

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

    write_all(stream, res).await?;
    Ok(salt)
}

pub async fn plain_password_challenge<S>(stream: &mut S) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put_u8(b'R');
    res.put_i32(mem::size_of::<i32>() as i32 * 2);
    res.put_i32(AUTHENTICATION_CLEAR_PASSWORD);

    write_all(stream, res).await?;
    Ok(())
}

/// Generate scram password challenge.
pub async fn scram_start_challenge<S>(stream: &mut S) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put_u8(b'R');
    res.put_i32(23); // message length = 'int' 4 + 'int' 4 + 'message' 13 + '00' 2.
    res.put_i32(10); // SCRAM.
    res.put_slice(SCRAM_SHA_256.as_ref());
    res.put_u8(0); // close string.
    res.put_u8(0); // pg magic.

    write_all(stream, res).await
}

pub async fn scram_server_response<S>(stream: &mut S, code: i32, data: &str) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put_u8(b'R');
    res.put_i32((4 + 4 + data.len()) as i32); // message length = 'int' 4 + 'int' 4 + 'message'
    res.put_i32(code); // SCRAM CONTINUE == 11 || SCRAM COMPLETE == 12 || SCRAM FINAL == 13
    res.put_slice(data.as_ref());
    write_all(stream, res).await
}

pub async fn read_password<S>(stream: &mut S) -> Result<Vec<u8>, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let code = match stream.read_u8().await {
        Ok(p) => p,
        Err(_) => {
            return Err(Error::AuthError(
                "can't first read symbol of password code".into(),
            ))
        }
    };
    if code as char != 'p' {
        return Err(Error::ProtocolSyncError(format!(
            "Expected p, got {}",
            code as char
        )));
    };
    let len = match stream.read_i32().await {
        Ok(len) => len,
        Err(_) => return Err(Error::AuthError("password message length".into())),
    };
    let mut password_response = vec![0u8; len as usize - mem::size_of::<i32>()];
    match stream.read_exact(&mut password_response).await {
        Ok(_) => (),
        Err(_) => return Err(Error::AuthError("password message".into())),
    };
    Ok(password_response)
}

/// Give the client the process_id and secret we generated
/// used in query cancellation.
pub async fn backend_key_data<S>(
    stream: &mut S,
    backend_id: i32,
    secret_key: i32,
) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut key_data = BytesMut::from(&b"K"[..]);
    key_data.put_i32(12);
    key_data.put_i32(backend_id);
    key_data.put_i32(secret_key);

    write_all(stream, key_data).await
}

/// Construct a `Q`: Query message.
pub fn simple_query(query: &str) -> BytesMut {
    let mut res = BytesMut::from(&b"Q"[..]);
    let query = format!("{}\0", query);

    res.put_i32(query.len() as i32 + 4);
    res.put_slice(query.as_bytes());

    res
}

/// Tell the client we're ready for another query.
pub async fn send_ready_for_query<S>(stream: &mut S) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    write_all(stream, ready_for_query(false)).await
}

/// Send the startup packet the server. We're pretending we're a Pg client.
/// This tells the server which user we are and what database we want.
pub async fn startup<S>(stream: &mut S, user: String, database: &str) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut bytes = BytesMut::with_capacity(25);

    bytes.put_i32(196608); // Protocol number

    // User
    bytes.put(&b"user\0"[..]);
    bytes.put_slice(user.as_bytes());
    bytes.put_u8(0);

    // Application name
    bytes.put(&b"application_name\0"[..]);
    bytes.put_slice(&b"pg_doorman\0"[..]);

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
            "Error writing startup to server socket - Error: {:?}",
            err
        ))),
    }
}

pub async fn ssl_request(stream: &mut TcpStream) -> Result<(), Error> {
    let mut bytes = BytesMut::with_capacity(12);

    bytes.put_i32(8);
    bytes.put_i32(80877103);

    match stream.write_all(&bytes).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Error writing SSLRequest to server socket - Error: {:?}",
            err
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
        return Err(Error::ClientBadStartup);
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
    md5_hash_second_pass(&(format!("{:x}", output)), salt)
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

/// Implements a response to our custom `SET ...`
/// This tells the client we're ready for the next query.
pub async fn custom_protocol_response_ok<S>(stream: &mut S, message: &str) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::with_capacity(25);

    let set_complete = BytesMut::from(&format!("{}\0", message)[..]);
    let len = (set_complete.len() + 4) as i32;

    // CommandComplete
    res.put_u8(b'C');
    res.put_i32(len);
    res.put_slice(&set_complete[..]);

    write_all_half(stream, &res).await?;
    send_ready_for_query(stream).await
}

pub async fn error_response<S>(stream: &mut S, message: &str, code: &str) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    error_response_terminal(stream, message, code).await?;
    send_ready_for_query(stream).await
}

pub async fn error_response_terminal<S>(
    stream: &mut S,
    message: &str,
    code: &str,
) -> Result<(), Error>
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
    error.put_slice(format!("{}\0", code).as_bytes());

    // The short error message.
    error.put_u8(b'M');
    error.put_slice(format!("{}\0", message).as_bytes());

    // No more fields follow.
    error.put_u8(0);

    // Compose the two message reply.
    let mut res = BytesMut::with_capacity(error.len() + 5);

    res.put_u8(b'E');
    res.put_i32(error.len() as i32 + 4);
    res.put(error);

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
    error.put_slice(format!("password authentication failed for user \"{}\"\0", user).as_bytes());

    // No more fields follow.
    error.put_u8(0);

    // Compose the two message reply.
    let mut res = BytesMut::new();

    res.put_u8(b'E');
    res.put_i32(error.len() as i32 + 4);

    res.put(error);

    write_all(stream, res).await
}

pub fn row_description(columns: &Vec<(&str, DataType)>) -> BytesMut {
    let mut res = BytesMut::new();
    let mut row_desc = BytesMut::new();

    // how many columns we are storing
    row_desc.put_i16(columns.len() as i16);

    for (name, data_type) in columns {
        // Column name
        row_desc.put_slice(format!("{}\0", name).as_bytes());

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

        // Type modifier: none that I know
        row_desc.put_i32(-1);

        // Format being used: text (0), binary (1)
        row_desc.put_i16(0);
    }

    res.put_u8(b'T');
    res.put_i32(row_desc.len() as i32 + 4);
    res.put(row_desc);

    res
}

/// Create a DataRow message.
pub fn data_row(row: &Vec<String>) -> BytesMut {
    let mut res = BytesMut::new();
    let mut data_row = BytesMut::new();

    data_row.put_i16(row.len() as i16);

    for column in row {
        let column = column.as_bytes();
        data_row.put_i32(column.len() as i32);
        data_row.put_slice(column);
    }

    res.put_u8(b'D');
    res.put_i32(data_row.len() as i32 + 4);
    res.put(data_row);

    res
}

pub fn data_row_nullable(row: &Vec<Option<String>>) -> BytesMut {
    let mut res = BytesMut::new();
    let mut data_row = BytesMut::new();

    data_row.put_i16(row.len() as i16);

    for column in row {
        if let Some(column) = column {
            let column = column.as_bytes();
            data_row.put_i32(column.len() as i32);
            data_row.put_slice(column);
        } else {
            data_row.put_i32(-1_i32);
        }
    }

    res.put_u8(b'D');
    res.put_i32(data_row.len() as i32 + 4);
    res.put(data_row);

    res
}

/// Create a CommandComplete message.
pub fn command_complete(command: &str) -> BytesMut {
    let cmd = BytesMut::from(format!("{}\0", command).as_bytes());
    let mut res = BytesMut::new();
    res.put_u8(b'C');
    res.put_i32(cmd.len() as i32 + 4);
    res.put(cmd);
    res
}

/// Create a notify message.
pub fn notify(message: &str, details: String) -> BytesMut {
    let mut notify_cmd = BytesMut::new();

    notify_cmd.put_slice("SNOTICE\0".as_bytes());
    notify_cmd.put_slice("C00000\0".as_bytes());
    notify_cmd.put_slice(format!("M{}\0", message).as_bytes());
    notify_cmd.put_slice(format!("D{}\0", details).as_bytes());

    // this extra byte says that is the end of the package
    notify_cmd.put_u8(0);

    let mut res = BytesMut::new();
    res.put_u8(b'N');
    res.put_i32(notify_cmd.len() as i32 + 4);
    res.put(notify_cmd);

    res
}

pub fn flush() -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'H');
    bytes.put_i32(4);
    bytes
}

pub fn sync() -> BytesMut {
    let mut bytes = BytesMut::with_capacity(mem::size_of::<u8>() + mem::size_of::<i32>());
    bytes.put_u8(b'S');
    bytes.put_i32(4);
    bytes
}

pub fn parse_complete() -> BytesMut {
    let mut bytes = BytesMut::with_capacity(mem::size_of::<u8>() + mem::size_of::<i32>());

    bytes.put_u8(b'1');
    bytes.put_i32(4);
    bytes
}

pub fn check_query_response() -> BytesMut {
    let mut bytes = BytesMut::with_capacity(11);

    bytes.put_u8(b'I');
    bytes.put_i32(mem::size_of::<i32>() as i32);
    bytes.put_u8(b'Z');
    bytes.put_i32(mem::size_of::<i32>() as i32 + 1);
    bytes.put_u8(b'I');
    bytes
}

pub fn deallocate_response() -> BytesMut {
    let mut bytes = BytesMut::with_capacity(22);

    bytes.put_u8(b'C');
    bytes.put_i32(15);
    bytes.put_slice(&b"DEALLOCATE\0"[..]);
    bytes.put_u8(b'Z');
    bytes.put_i32(5);
    bytes.put_u8(b'I');
    bytes
}

pub fn ready_for_query(in_transaction: bool) -> BytesMut {
    let mut bytes = BytesMut::with_capacity(
        mem::size_of::<u8>() + mem::size_of::<i32>() + mem::size_of::<u8>(),
    );

    bytes.put_u8(b'Z');
    bytes.put_i32(5);
    if in_transaction {
        bytes.put_u8(b'T');
    } else {
        bytes.put_u8(b'I');
    }

    bytes
}

/// Write all data in the buffer to the TcpStream.
pub async fn write_all<S>(stream: &mut S, buf: BytesMut) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    match stream.write_all(&buf).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Error writing to socket - Error: {:?}",
            err
        ))),
    }
}

/// Write all the data in the buffer to the TcpStream, write owned half (see mpsc).
pub async fn write_all_half<S>(stream: &mut S, buf: &BytesMut) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    match stream.write_all(buf).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Error writing to socket: {:?}",
            err
        ))),
    }
}

pub async fn write_all_flush<S>(stream: &mut S, buf: &[u8]) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    match stream.write_all(buf).await {
        Ok(_) => match stream.flush().await {
            Ok(_) => Ok(()),
            Err(err) => Err(Error::SocketError(format!(
                "Error flushing socket: {:?}",
                err
            ))),
        },
        Err(err) => Err(Error::SocketError(format!(
            "Error writing to socket: {:?}",
            err
        ))),
    }
}

/// Read header.
pub async fn read_message_header<S>(stream: &mut S) -> Result<(u8, i32), Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let code = match stream.read_u8().await {
        Ok(code) => code,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Error reading message code from socket - Error {:?}",
                err
            )))
        }
    };
    let len = match stream.read_i32().await {
        Ok(len) => len,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Error reading message len from socket - Code: {:?}, Error: {:?}",
                code, err
            )))
        }
    };
    Ok((code, len))
}

/// Read a message data from the socket.
pub async fn read_message_data<S>(stream: &mut S, code: u8, len: i32) -> Result<BytesMut, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let mut bytes = BytesMut::with_capacity(len as usize + 1);

    bytes.put_u8(code);
    bytes.put_i32(len);

    bytes.resize(bytes.len() + len as usize - mem::size_of::<i32>(), b'0');

    let slice_start = mem::size_of::<u8>() + mem::size_of::<i32>();
    let slice_end = slice_start + len as usize - mem::size_of::<i32>();

    // Avoids a panic
    if slice_end < slice_start {
        CURRENT_MEMORY.fetch_add(-len as i64, Ordering::Relaxed);
        return Err(Error::SocketError(format!(
            "Error reading message from socket - Code: {:?} - Length {:?}, Error: {:?}",
            code, len, "Unexpected length value for message"
        )));
    }

    match stream.read_exact(&mut bytes[slice_start..slice_end]).await {
        Ok(_) => (),
        Err(err) => {
            CURRENT_MEMORY.fetch_add(-len as i64, Ordering::Relaxed);
            return Err(Error::SocketError(format!(
                "Error reading message from socket - Code: {:?}, Error: {:?}",
                code, err
            )));
        }
    };

    Ok(bytes)
}

/// Read a complete message from the socket.
pub async fn read_message<S>(stream: &mut S, max_memory_usage: u64) -> Result<BytesMut, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let (code, len) = read_message_header(stream).await?;
    if len > MAX_MESSAGE_SIZE {
        proxy_copy_data_with_timeout(
            Duration::from_millis(get_config().general.proxy_copy_data_timeout),
            stream,
            &mut tokio::io::sink(),
            len as usize - mem::size_of::<i32>(),
        )
        .await?;
        return Err(Error::MaxMessageSize);
    };
    let current_memory = CURRENT_MEMORY.load(Ordering::Relaxed);
    if current_memory > max_memory_usage as i64 {
        error!("reached memory limit while processing code '{}' message len '{}' current memory usage: '{}' maximum memory usage: '{}'",
            code as char, len, current_memory, max_memory_usage);
        proxy_copy_data_with_timeout(
            Duration::from_millis(get_config().general.proxy_copy_data_timeout),
            stream,
            &mut tokio::io::sink(),
            len as usize - mem::size_of::<i32>(),
        )
        .await?;
        return Err(Error::CurrentMemoryUsage);
    }
    CURRENT_MEMORY.fetch_add(len as i64, Ordering::Relaxed);
    let bytes = read_message_data(stream, code, len).await?;
    CURRENT_MEMORY.fetch_add(-len as i64, Ordering::Relaxed);
    Ok(bytes)
}

pub async fn proxy_copy_data_with_timeout<R, W>(
    duration: tokio::time::Duration,
    read: &mut R,
    write: &mut W,
    len: usize,
) -> Result<usize, Error>
where
    R: tokio::io::AsyncRead + std::marker::Unpin,
    W: tokio::io::AsyncWrite + std::marker::Unpin,
{
    match timeout(duration, proxy_copy_data(read, write, len)).await {
        Ok(Ok(len)) => Ok(len),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(ProxyTimeout),
    }
}

pub async fn proxy_copy_data<R, W>(read: &mut R, write: &mut W, len: usize) -> Result<usize, Error>
where
    R: tokio::io::AsyncRead + std::marker::Unpin,
    W: tokio::io::AsyncWrite + std::marker::Unpin,
{
    const MAX_BUFFER_CHUNK: usize = 4096; // гарантия того что вызовы read из
                                          // буфферизированного stream 8kb будет быстрым.
    let mut bytes_remained = len;
    let mut bytes_readed: usize;
    let mut buffer_size: usize = MAX_BUFFER_CHUNK;
    if buffer_size > len {
        buffer_size = len
    }
    let mut buffer = [0; MAX_BUFFER_CHUNK];
    loop {
        // read.
        match read.read(&mut buffer[..buffer_size]).await {
            Ok(n) => bytes_readed = n,
            Err(err) => {
                return Err(Error::SocketError(format!(
                    "Error reading message from socket - Error: {:?}",
                    err
                )))
            }
        }
        if bytes_readed == 0 {
            // EOS if the remote closes its end gracefully you’ll get a read of 0;
            // if it does it ungracefully (eg sends a RST packet) then you’ll get an Error variant of the Result.
            return Err(Error::SocketError(
                "Error reading message from socket - Error: EOS".to_string(),
            ));
        }
        // write.
        match write.write_all(&buffer[..bytes_readed]).await {
            Ok(_) => (),
            Err(err) => {
                return Err(Error::SocketError(format!(
                    "Error write message to socket - Error: {:?}",
                    err
                )))
            }
        }
        bytes_remained -= bytes_readed;
        if bytes_remained == 0 {
            break;
        }
        // последний кусок чтобы не прочитать лишнего из read.
        if buffer_size > bytes_remained {
            buffer_size = bytes_remained;
        }
    }
    match write.flush().await {
        Ok(_) => (),
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Error flush message to socket - Error: {:?}",
                err
            )))
        }
    }
    Ok(len)
}

pub fn server_parameter_message(key: &str, value: &str) -> BytesMut {
    let mut server_info = BytesMut::new();

    let null_byte_size = 1;
    let len: usize =
        mem::size_of::<i32>() + key.len() + null_byte_size + value.len() + null_byte_size;

    server_info.put_slice("S".as_bytes());
    server_info.put_i32(len.try_into().unwrap());
    server_info.put_slice(key.as_bytes());
    server_info.put_bytes(0, 1);
    server_info.put_slice(value.as_bytes());
    server_info.put_bytes(0, 1);

    server_info
}

pub fn configure_unix_socket(stream: &UnixStream) {
    let sock_ref = SockRef::from(stream);
    let conf = get_config();

    match sock_ref.set_linger(Some(Duration::from_secs(conf.general.tcp_so_linger))) {
        Ok(_) => {}
        Err(err) => error!("Could not configure unix_so_linger for socket: {}", err),
    }
    match sock_ref.set_send_buffer_size(conf.general.unix_socket_buffer_size) {
        Ok(_) => {}
        Err(err) => error!(
            "Could not configure set_send_buffer_size for socket: {}",
            err
        ),
    }
    match sock_ref.set_recv_buffer_size(conf.general.unix_socket_buffer_size) {
        Ok(_) => {}
        Err(err) => error!(
            "Could not configure set_recv_buffer_size for socket: {}",
            err
        ),
    }
}

pub fn configure_tcp_socket(stream: &TcpStream) {
    let sock_ref = SockRef::from(stream);
    let conf = get_config();

    match sock_ref.set_linger(Some(Duration::from_secs(conf.general.tcp_so_linger))) {
        Ok(_) => {}
        Err(err) => error!("Could not configure tcp_so_linger for socket: {}", err),
    }

    match sock_ref.set_nodelay(conf.general.tcp_no_delay) {
        Ok(_) => {}
        Err(err) => error!("Could not configure no delay for socket: {}", err),
    }

    match sock_ref.set_keepalive(true) {
        Ok(_) => {
            match sock_ref.set_tcp_keepalive(
                &TcpKeepalive::new()
                    .with_interval(Duration::from_secs(conf.general.tcp_keepalives_interval))
                    .with_retries(conf.general.tcp_keepalives_count)
                    .with_time(Duration::from_secs(conf.general.tcp_keepalives_idle)),
            ) {
                Ok(_) => (),
                Err(err) => error!("Could not configure tcp_keepalive for socket: {}", err),
            }
        }
        Err(err) => error!("Could not configure socket: {}", err),
    }
}

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

pub enum ExtendedProtocolData {
    Parse {
        data: BytesMut,
        metadata: Option<(Arc<Parse>, u64)>,
    },
    Bind {
        data: BytesMut,
        metadata: Option<String>,
    },
    Describe {
        data: BytesMut,
        metadata: Option<String>,
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

    pub fn create_new_bind(data: BytesMut, metadata: Option<String>) -> Self {
        Self::Bind { data, metadata }
    }

    pub fn create_new_describe(data: BytesMut, metadata: Option<String>) -> Self {
        Self::Describe { data, metadata }
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
        let mut cursor = Cursor::new(buf);
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
        let mut cursor = Cursor::new(buf);
        // Skip the code and length
        cursor.advance(mem::size_of::<u8>() + mem::size_of::<i32>());
        cursor.read_string()
    }

    /// Hashes the parse statement to be used as a key in the global cache
    pub fn get_hash(&self) -> u64 {
        // TODO: Take a look at which hashing function is being used
        let mut hasher = DefaultHasher::new();

        let concatenated = format!(
            "{}{}{}",
            self.query,
            self.num_params,
            self.param_types
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );

        concatenated.hash(&mut hasher);

        hasher.finish()
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
        let mut cursor = Cursor::new(buf);
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
            // There is special occasion when the parameter is NULL
            // In that case, param length is defined as -1
            // So if the passed parameter len is over 0
            if param_len > 0 {
                let mut param = BytesMut::with_capacity(param_len as usize);
                param.resize(param_len as usize, b'0');
                cursor.copy_to_slice(&mut param);
                // we push and the length and the parameter into vector
                param_values.push((param_len, param));
            } else {
                // otherwise we push a tuple with -1 and 0-len BytesMut
                // which means that after encountering -1 postgres proceeds
                // to processing another parameter
                param_values.push((param_len, BytesMut::new()));
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
        let mut cursor = Cursor::new(buf);
        // Skip the code and length
        cursor.advance(mem::size_of::<u8>() + mem::size_of::<i32>());
        cursor.read_string()?;
        cursor.read_string()
    }

    /// Renames the prepared statement to a new name
    pub fn rename(buf: BytesMut, new_name: &str) -> Result<BytesMut, Error> {
        let mut cursor = Cursor::new(&buf);
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
        let mut cursor = Cursor::new(bytes);
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
        let mut cursor = Cursor::new(bytes);
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

pub fn close_complete() -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(b'3');
    bytes.put_i32(4);
    bytes
}

// from https://www.postgresql.org/docs/12/protocol-error-fields.html
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

// TODO: implement with https://docs.rs/derive_more/latest/derive_more/
impl Display for PgErrorMsg {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[severity: {}]", self.severity)?;
        write!(f, "[code: {}]", self.code)?;
        write!(f, "[message: {}]", self.message)?;
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
        for msg_part in error_msg.split(|v| *v == MESSAGE_TERMINATOR) {
            if msg_part.is_empty() {
                continue;
            }

            let msg_content = match String::from_utf8_lossy(&msg_part[1..]).parse() {
                Ok(c) => c,
                Err(err) => {
                    return Err(Error::ServerMessageParserError(format!(
                        "could not parse server message field. err {:?}",
                        err
                    )))
                }
            };

            match &msg_part[0] {
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

#[cfg(test)]
mod tests {
    use crate::messages::{
        check_query_response, command_complete, deallocate_response, flush, parse_complete,
        ready_for_query, set_messages_right_place, PgErrorMsg,
    };
    use bytes::{BufMut, BytesMut};
    use log::{error, info};
    use std::mem;

    fn field(kind: char, content: &str) -> Vec<u8> {
        format!("{kind}{content}\0").as_bytes().to_vec()
    }

    #[test]
    fn check_query_test() {
        let a = check_query_response();
        assert_eq!(a.len(), 11)
    }

    #[test]
    fn deallocate_response_test() {
        let a = deallocate_response();
        assert_eq!(a.len(), 22)
    }

    #[test]
    fn make_parse_complete_test() {
        let mut in_msg = parse_complete();
        assert_eq!(
            parse_complete().len(),
            set_messages_right_place(in_msg.to_vec())
                .expect("parsing")
                .len()
        );
        in_msg.put(flush());
        assert_eq!(
            parse_complete().len() + flush().len(),
            set_messages_right_place(in_msg.to_vec())
                .expect("parsing")
                .len()
        );
        in_msg.put(ready_for_query(true));
        assert_eq!(
            parse_complete().len() + flush().len() + ready_for_query(true).len(),
            set_messages_right_place(in_msg.to_vec())
                .expect("parsing")
                .len()
        );
    }

    pub fn bind_complete() -> BytesMut {
        let mut bytes = BytesMut::with_capacity(mem::size_of::<u8>() + mem::size_of::<i32>());

        bytes.put_u8(b'2');
        bytes.put_i32(4);
        bytes
    }

    pub fn row_description() -> BytesMut {
        let mut bytes = BytesMut::with_capacity(mem::size_of::<u8>() + mem::size_of::<i32>());

        bytes.put_u8(b't');
        bytes.put_i32(4);
        bytes
    }

    fn show_headers(msg: BytesMut) -> String {
        let in_msg_len = msg.len();
        let mut cursor = 0;
        let mut result = Vec::new();
        loop {
            if cursor == in_msg_len {
                break;
            }
            result.push(msg[cursor]);
            cursor += 1;
            let len_ref = match <[u8; 4]>::try_from(&msg[cursor..cursor + 4]) {
                Ok(len_ref) => len_ref,
                _ => {
                    panic!("size")
                }
            };
            let mut len = i32::from_be_bytes(len_ref) as usize;
            len -= 4;
            cursor += 4;
            cursor += len;
        }
        String::from_utf8_lossy(&result).to_string()
    }

    #[test]
    fn make_parse_complete_test_double() {
        let mut in_msg = parse_complete(); // 1
        in_msg.put(parse_complete()); // 1
        in_msg.put(bind_complete()); // 2
        in_msg.put(row_description()); // t
        in_msg.put(command_complete("1")); // C
        in_msg.put(bind_complete()); // 2
        in_msg.put(row_description()); // t
        in_msg.put(command_complete("2")); // C
        in_msg.put(ready_for_query(false)); // Z
        let out_msg = set_messages_right_place(in_msg.to_vec()).expect("parse");
        println!("112tC2tCZ");
        assert_eq!(show_headers(out_msg), "12tC12tCZ".to_string());
    }

    #[test]
    fn parse_fields() {
        let mut complete_msg = vec![];
        let severity = "FATAL";
        complete_msg.extend(field('S', severity));
        complete_msg.extend(field('V', severity));

        let error_code = "29P02";
        complete_msg.extend(field('C', error_code));
        let message = "password authentication failed for user \"wrong_user\"";
        complete_msg.extend(field('M', message));
        let detail_msg = "super detailed message";
        complete_msg.extend(field('D', detail_msg));
        let hint_msg = "hint detail here";
        complete_msg.extend(field('H', hint_msg));
        complete_msg.extend(field('P', "123"));
        complete_msg.extend(field('p', "234"));
        let internal_query = "SELECT * from foo;";
        complete_msg.extend(field('q', internal_query));
        let where_msg = "where goes here";
        complete_msg.extend(field('W', where_msg));
        let schema_msg = "schema_name";
        complete_msg.extend(field('s', schema_msg));
        let table_msg = "table_name";
        complete_msg.extend(field('t', table_msg));
        let column_msg = "column_name";
        complete_msg.extend(field('c', column_msg));
        let data_type_msg = "type_name";
        complete_msg.extend(field('d', data_type_msg));
        let constraint_msg = "constraint_name";
        complete_msg.extend(field('n', constraint_msg));
        let file_msg = "pg_doorman.c";
        complete_msg.extend(field('F', file_msg));
        complete_msg.extend(field('L', "335"));
        let routine_msg = "my_failing_routine";
        complete_msg.extend(field('R', routine_msg));

        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(true)
            .init();

        info!(
            "full message: {}",
            PgErrorMsg::parse(&complete_msg).unwrap()
        );
        assert_eq!(
            PgErrorMsg {
                severity_localized: severity.to_string(),
                severity: severity.to_string(),
                code: error_code.to_string(),
                message: message.to_string(),
                detail: Some(detail_msg.to_string()),
                hint: Some(hint_msg.to_string()),
                position: Some(123),
                internal_position: Some(234),
                internal_query: Some(internal_query.to_string()),
                where_context: Some(where_msg.to_string()),
                schema_name: Some(schema_msg.to_string()),
                table_name: Some(table_msg.to_string()),
                column_name: Some(column_msg.to_string()),
                data_type_name: Some(data_type_msg.to_string()),
                constraint_name: Some(constraint_msg.to_string()),
                file_name: Some(file_msg.to_string()),
                line: Some(335),
                routine: Some(routine_msg.to_string()),
            },
            PgErrorMsg::parse(&complete_msg).unwrap()
        );

        let mut only_mandatory_msg = vec![];
        only_mandatory_msg.extend(field('S', severity));
        only_mandatory_msg.extend(field('V', severity));
        only_mandatory_msg.extend(field('C', error_code));
        only_mandatory_msg.extend(field('M', message));
        only_mandatory_msg.extend(field('D', detail_msg));

        let err_fields = PgErrorMsg::parse(&only_mandatory_msg).unwrap();
        info!("only mandatory fields: {}", &err_fields);
        error!(
            "server error: {}: {}",
            err_fields.severity, err_fields.message
        );
        assert_eq!(
            PgErrorMsg {
                severity_localized: severity.to_string(),
                severity: severity.to_string(),
                code: error_code.to_string(),
                message: message.to_string(),
                detail: Some(detail_msg.to_string()),
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
            },
            PgErrorMsg::parse(&only_mandatory_msg).unwrap()
        );
    }
}
