// Standard library imports
use std::sync::atomic::Ordering;

// External crate imports
use bytes::{BufMut, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

// Internal crate imports
use crate::errors::Error;
use crate::errors::Error::ProxyTimeout;
use crate::messages::{CURRENT_MEMORY, MAX_MESSAGE_SIZE};

/// Write all data in the buffer to the TcpStream.
pub async fn write_all<S>(stream: &mut S, buf: BytesMut) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    match stream.write_all(&buf).await {
        Ok(_) => Ok(()),
        Err(err) => Err(Error::SocketError(format!(
            "Error writing to socket - Error: {err:?}"
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
            "Error writing to socket: {err:?}"
        ))),
    }
}

/// Write all the data in the buffer to the TcpStream and flush the stream.
pub async fn write_all_flush<S>(stream: &mut S, buf: &[u8]) -> Result<(), Error>
where
    S: tokio::io::AsyncWrite + std::marker::Unpin,
{
    match stream.write_all(buf).await {
        Ok(_) => match stream.flush().await {
            Ok(_) => Ok(()),
            Err(err) => Err(Error::SocketError(format!(
                "Error flushing socket: {err:?}"
            ))),
        },
        Err(err) => Err(Error::SocketError(format!(
            "Error writing to socket: {err:?}"
        ))),
    }
}

/// Read message header.
pub async fn read_message_header<S>(stream: &mut S) -> Result<(u8, i32), Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let code = match stream.read_u8().await {
        Ok(code) => code,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Error reading message code from socket - Error {err:?}"
            )))
        }
    };
    let len = match stream.read_i32().await {
        Ok(len) => len,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Error reading message len from socket - Code: {code:?}, Error: {err:?}"
            )))
        }
    };

    Ok((code, len))
}

/// Read message data.
pub async fn read_message_data<S>(stream: &mut S, code: u8, len: i32) -> Result<BytesMut, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    if len < 4 {
        return Err(Error::ProtocolSyncError(format!(
            "Message length is too small: {len}"
        )));
    }

    if len > MAX_MESSAGE_SIZE {
        return Err(Error::ProtocolSyncError(format!(
            "Message length is too large: {len}"
        )));
    }

    let total_len = len as usize + 1; // code(1) + len(4) + data
    let mut buf = BytesMut::with_capacity(total_len);
    buf.put_u8(code);
    buf.put_i32(len);
    buf.resize(total_len, 0);

    match stream.read_exact(&mut buf[5..]).await {
        Ok(_) => Ok(buf),
        Err(err) => Err(Error::SocketError(format!(
            "Error reading message data from socket - Code: {code:?}, Error: {err:?}"
        ))),
    }
}

#[inline]
pub async fn read_message<S>(stream: &mut S, max_memory_usage: u64) -> Result<BytesMut, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let (code, len) = read_message_header(stream).await?;
    let prev = CURRENT_MEMORY.fetch_add(len as i64, Ordering::Relaxed);
    if (prev + len as i64) as u64 > max_memory_usage {
        CURRENT_MEMORY.fetch_sub(len as i64, Ordering::Relaxed);
        return Err(Error::CurrentMemoryUsage);
    }
    let result = read_message_data(stream, code, len).await;
    CURRENT_MEMORY.fetch_sub(len as i64, Ordering::Relaxed);
    result
}

/// Shrink threshold: buffers above this are replaced after small messages.
const READ_BUF_SHRINK_THRESHOLD: usize = 65536; // 64 KB
/// Default read buffer capacity.
const READ_BUF_DEFAULT_CAPACITY: usize = 8192; // 8 KB

/// Read a message into a reusable buffer. Returns owned BytesMut via split(),
/// keeping the backing capacity in `buf` for the next call. Zero heap
/// allocations for messages that fit in the existing capacity.
///
/// After a large message (>64KB), the buffer is replaced with a fresh 8KB
/// allocation to prevent permanent bloat from a single oversized result.
#[inline]
pub async fn read_message_reuse<S>(
    stream: &mut S,
    buf: &mut BytesMut,
    max_memory_usage: u64,
) -> Result<BytesMut, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let (code, len) = read_message_header(stream).await?;

    if len < 4 {
        return Err(Error::ProtocolSyncError(format!(
            "Message length is too small: {len}"
        )));
    }
    if len > MAX_MESSAGE_SIZE {
        return Err(Error::ProtocolSyncError(format!(
            "Message length is too large: {len}"
        )));
    }

    let prev = CURRENT_MEMORY.fetch_add(len as i64, Ordering::Relaxed);
    if (prev + len as i64) as u64 > max_memory_usage {
        CURRENT_MEMORY.fetch_sub(len as i64, Ordering::Relaxed);
        return Err(Error::CurrentMemoryUsage);
    }

    let total_len = len as usize + 1;
    buf.clear();
    buf.reserve(total_len);
    buf.put_u8(code);
    buf.put_i32(len);
    buf.resize(total_len, 0);

    let result = match stream.read_exact(&mut buf[5..]).await {
        Ok(_) => Ok(buf.split()),
        Err(err) => Err(Error::SocketError(format!(
            "Error reading message data from socket - Code: {code:?}, Error: {err:?}"
        ))),
    };

    CURRENT_MEMORY.fetch_sub(len as i64, Ordering::Relaxed);

    // Shrink oversized buffer after the data has been split off.
    if buf.capacity() > READ_BUF_SHRINK_THRESHOLD && total_len < READ_BUF_DEFAULT_CAPACITY {
        *buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);
    }

    result
}

/// Read message body into a reusable buffer when header is already consumed.
/// Used by server recv() loop where read_message_header() is called separately.
/// Same zero-alloc semantics as read_message_reuse, but skips header read.
#[inline]
pub async fn read_message_body_reuse<S>(
    stream: &mut S,
    buf: &mut BytesMut,
    code: u8,
    len: i32,
) -> Result<BytesMut, Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
{
    let total_len = len as usize + 1;
    buf.clear();
    buf.reserve(total_len);
    buf.put_u8(code);
    buf.put_i32(len);
    buf.resize(total_len, 0);

    match stream.read_exact(&mut buf[5..]).await {
        Ok(_) => Ok(buf.split()),
        Err(err) => Err(Error::SocketError(format!(
            "Error reading message data from socket - Code: {code:?}, Error: {err:?}"
        ))),
    }
}

/// Copy data from one stream to another with a timeout.
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

/// Copy data from one stream to another.
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
                    "Error reading from socket: {err:?}"
                )))
            }
        };
        if bytes_readed == 0 {
            return Err(Error::SocketError(
                "Error reading from socket: connection closed".to_string(),
            ));
        }

        // write.
        match write.write_all(&buffer[..bytes_readed]).await {
            Ok(_) => {}
            Err(err) => {
                return Err(Error::SocketError(format!(
                    "Error writing to socket: {err:?}"
                )))
            }
        };

        bytes_remained -= bytes_readed;
        if bytes_remained == 0 {
            break;
        }
        if bytes_remained < buffer_size {
            buffer_size = bytes_remained;
        }
    }
    Ok(len)
}
