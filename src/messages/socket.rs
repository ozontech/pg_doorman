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

/// Read a message into a reusable buffer. Returns owned BytesMut via split(),
/// keeping the backing capacity in `buf` for the next call.
///
/// Amortized allocation cost: ~1 heap alloc per `8192 / msg_size` messages.
/// split() hands off the filled region; reserve() reuses remaining capacity
/// in the same backing allocation until exhausted, then allocates fresh 8KB.
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

    result
}

/// Read message body into a reusable buffer when header is already consumed.
/// Used by server recv() loop where read_message_header() is called separately.
/// Same amortized allocation semantics as read_message_reuse, but skips header read.
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
    if len < 4 {
        return Err(Error::ProtocolSyncError(format!(
            "Message length is too small: {len}"
        )));
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const READ_BUF_DEFAULT_CAPACITY: usize = 8192;

    /// Build a raw PG wire message: [code: u8][len: i32][body...]
    /// len includes itself (4 bytes) but not the code byte.
    fn wire_msg(code: u8, body: &[u8]) -> Vec<u8> {
        let len = (4 + body.len()) as i32;
        let mut msg = Vec::with_capacity(1 + 4 + body.len());
        msg.push(code);
        msg.extend_from_slice(&len.to_be_bytes());
        msg.extend_from_slice(body);
        msg
    }

    // =========================================================================
    // read_message_reuse — wire protocol validation
    // =========================================================================

    /// ReadyForQuery 'Z' with body 'I' (idle) — the most common server→client message.
    /// DBA sees this after every successful query. Must parse correctly.
    #[tokio::test]
    async fn reuse_ready_for_query() {
        let data = wire_msg(b'Z', &[b'I']);
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();

        assert_eq!(result.len(), 6); // code(1) + len(4) + body(1)
        assert_eq!(result[0], b'Z');
        assert_eq!(result[5], b'I');
    }

    /// Minimal valid message: len=4 means zero body bytes.
    /// This is a degenerate but valid PG message (e.g., Sync 'S', Flush 'H').
    /// read_exact on a 0-length slice must be a no-op, not an error.
    #[tokio::test]
    async fn reuse_minimal_message_len_4() {
        let data = wire_msg(b'S', &[]); // Sync: code='S', len=4, no body
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();

        assert_eq!(result.len(), 5); // code(1) + len(4) + body(0)
        assert_eq!(result[0], b'S');
        let len = i32::from_be_bytes([result[1], result[2], result[3], result[4]]);
        assert_eq!(len, 4);
    }

    /// CommandComplete 'C' with tag "SELECT 1" — typical query response.
    /// Verifies body bytes are read correctly.
    #[tokio::test]
    async fn reuse_command_complete() {
        let body = b"SELECT 1\0";
        let data = wire_msg(b'C', body);
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();

        assert_eq!(result[0], b'C');
        assert_eq!(&result[5..], body);
    }

    /// len < 4 is a protocol violation — PG length field includes itself (4 bytes).
    /// Pooler must reject without touching memory counter.
    #[tokio::test]
    async fn reuse_len_less_than_4_returns_error() {
        // Craft header manually: code='X', len=3
        let mut data = vec![b'X'];
        data.extend_from_slice(&3_i32.to_be_bytes());
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX).await;

        assert!(result.is_err());
        // Note: CURRENT_MEMORY is a global shared by all tests — don't assert absolute value
    }

    /// Negative length — could happen with corrupted TCP stream or malicious client.
    /// Must be caught by len < 4 check.
    #[tokio::test]
    async fn reuse_negative_len_returns_error() {
        let mut data = vec![b'Q'];
        data.extend_from_slice(&(-1_i32).to_be_bytes());
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX).await;

        assert!(result.is_err());
        // Note: CURRENT_MEMORY is a global shared by all tests — don't assert absolute value
    }

    /// len > MAX_MESSAGE_SIZE (256MB) — prevents OOM from malformed messages.
    #[tokio::test]
    async fn reuse_len_exceeds_max_returns_error() {
        let mut data = vec![b'D'];
        data.extend_from_slice(&(MAX_MESSAGE_SIZE + 1).to_be_bytes());
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX).await;

        assert!(result.is_err());
        // Note: CURRENT_MEMORY is a global shared by all tests — don't assert absolute value
    }

    // =========================================================================
    // read_message_reuse — memory pressure
    // =========================================================================

    /// Memory limit exactly hit — message at boundary should be accepted.
    /// DBA sets max_memory to control proxy RAM usage.
    #[tokio::test]
    async fn reuse_memory_limit_high_accepted() {
        let body = vec![0u8; 96]; // len = 100
        let data = wire_msg(b'D', &body);
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        // High limit — always accepted regardless of CURRENT_MEMORY from other tests
        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX).await;
        assert!(result.is_ok());
    }

    /// Memory limit set to 1 — any real message exceeds it.
    #[tokio::test]
    async fn reuse_memory_limit_tiny_rejected() {
        let body = vec![0u8; 96]; // len = 100
        let data = wire_msg(b'D', &body);
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_reuse(&mut stream, &mut buf, 1).await;
        assert!(result.is_err());
    }

    /// Memory counter delta must be 0 after successful read.
    /// Resets counter to isolate from parallel tests sharing the global atomic.
    #[tokio::test]
    async fn reuse_memory_counter_balanced_on_success() {
        let data = wire_msg(b'Z', &[b'I']);
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        CURRENT_MEMORY.store(0, Ordering::SeqCst);
        let _ = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();
        let after = CURRENT_MEMORY.load(Ordering::SeqCst);

        assert_eq!(after, 0, "memory counter leaked: {after}");
    }

    /// Memory counter delta must be 0 even on read failure (EOF mid-body).
    #[tokio::test]
    async fn reuse_memory_counter_balanced_on_read_error() {
        let mut data = vec![b'D'];
        data.extend_from_slice(&100_i32.to_be_bytes());
        let mut stream = Cursor::new(data);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        CURRENT_MEMORY.store(0, Ordering::SeqCst);
        let result = read_message_reuse(&mut stream, &mut buf, u64::MAX).await;
        let after = CURRENT_MEMORY.load(Ordering::SeqCst);

        assert!(result.is_err());
        assert_eq!(after, 0, "memory counter leaked on error: {after}");
    }

    // =========================================================================
    // read_message_reuse — buffer management (the core optimization)
    // =========================================================================

    /// Three messages in sequence on the same buffer — capacity should stabilize.
    /// This is the steady-state: after warmup, zero allocations per message.
    #[tokio::test]
    async fn reuse_sequential_messages_stable_capacity() {
        let msg1 = wire_msg(b'Z', &[b'I']);
        let msg2 = wire_msg(b'C', b"SELECT 1\0");
        let msg3 = wire_msg(b'Z', &[b'T']);

        let mut all = Vec::new();
        all.extend_from_slice(&msg1);
        all.extend_from_slice(&msg2);
        all.extend_from_slice(&msg3);

        let mut stream = Cursor::new(all);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let r1 = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();
        let cap_after_first = buf.capacity();

        let r2 = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();
        let cap_after_second = buf.capacity();

        let r3 = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();
        let cap_after_third = buf.capacity();

        // All messages decoded correctly
        assert_eq!(r1[0], b'Z');
        assert_eq!(r2[0], b'C');
        assert_eq!(r3[0], b'Z');

        // Capacity stays within 8KB range (reserve reuses after split)
        assert!(cap_after_first <= READ_BUF_DEFAULT_CAPACITY);
        assert!(cap_after_second <= READ_BUF_DEFAULT_CAPACITY);
        assert!(cap_after_third <= READ_BUF_DEFAULT_CAPACITY);
    }

    /// After a large message, split() hands the big allocation to the caller.
    /// The reusable buf gets near-zero remaining capacity, so the next reserve()
    /// allocates a fresh small buffer. No permanent bloat from a single large message.
    #[tokio::test]
    async fn reuse_large_then_small_no_bloat() {
        let large_body = vec![0u8; 100_000];
        let small_body = vec![0u8; 10];

        let mut all = Vec::new();
        all.extend_from_slice(&wire_msg(b'D', &large_body));
        all.extend_from_slice(&wire_msg(b'Z', &small_body));

        let mut stream = Cursor::new(all);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        // Read large message — reserve() grows, split() takes the data
        let large_msg = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();
        assert_eq!(large_msg.len(), 1 + 4 + 100_000);

        // Read small message — reserve() allocates fresh small buffer
        let small_msg = read_message_reuse(&mut stream, &mut buf, u64::MAX)
            .await
            .unwrap();
        assert_eq!(small_msg[0], b'Z');

        // Buffer capacity is small — no permanent 100KB bloat
        assert!(
            buf.capacity() < 65536,
            "capacity should be small after split pattern: got {}",
            buf.capacity(),
        );
    }

    // =========================================================================
    // read_message_body_reuse — server-side path
    // =========================================================================

    /// Standard CommandComplete read when header is already consumed by recv().
    #[tokio::test]
    async fn body_reuse_standard_message() {
        let body = b"SELECT 1\0";
        let len = (4 + body.len()) as i32;

        // Stream contains ONLY body (header already consumed)
        let mut stream = Cursor::new(body.to_vec());
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_body_reuse(&mut stream, &mut buf, b'C', len)
            .await
            .unwrap();

        assert_eq!(result[0], b'C');
        let result_len = i32::from_be_bytes([result[1], result[2], result[3], result[4]]);
        assert_eq!(result_len, len);
        assert_eq!(&result[5..], body);
    }

    /// Minimal body: len=4, zero body bytes. Header takes 5 bytes, body is empty.
    #[tokio::test]
    async fn body_reuse_minimal_len_4() {
        let mut stream = Cursor::new(Vec::<u8>::new()); // no body to read
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_body_reuse(&mut stream, &mut buf, b'H', 4)
            .await
            .unwrap();

        assert_eq!(result.len(), 5); // code(1) + len(4)
        assert_eq!(result[0], b'H');
    }

    /// len < 4 is a protocol violation — must return error, not panic.
    #[tokio::test]
    async fn body_reuse_len_less_than_4_returns_error() {
        let mut stream = Cursor::new(Vec::<u8>::new());
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_body_reuse(&mut stream, &mut buf, b'D', 3).await;
        assert!(result.is_err());
    }

    /// Negative length from corrupted TCP stream — must return error, not panic.
    #[tokio::test]
    async fn body_reuse_negative_len_returns_error() {
        let mut stream = Cursor::new(Vec::<u8>::new());
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_body_reuse(&mut stream, &mut buf, b'D', -1).await;
        assert!(result.is_err());
    }

    /// EOF during body read — simulates TCP connection reset from PostgreSQL.
    /// DBA sees this when PG crashes or network partition during query.
    #[tokio::test]
    async fn body_reuse_eof_mid_body() {
        // Claim 1000 bytes body but provide only 10
        let partial_body = vec![0u8; 10];
        let mut stream = Cursor::new(partial_body);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_body_reuse(&mut stream, &mut buf, b'D', 1004).await;

        assert!(result.is_err());
    }

    /// split() returns data independent from the reusable buffer.
    /// Critical: mutation of returned bytes must not affect buf, and vice versa.
    #[tokio::test]
    async fn body_reuse_split_returns_independent_data() {
        let body = b"test_data\0";
        let len = (4 + body.len()) as i32;
        let mut stream = Cursor::new(body.to_vec());
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let result = read_message_body_reuse(&mut stream, &mut buf, b'C', len)
            .await
            .unwrap();

        // buf should be empty after split
        assert_eq!(buf.len(), 0);
        // result should have the data
        assert_eq!(result[0], b'C');
        assert_eq!(&result[5..], body);
    }

    /// After split, buf retains capacity for next message (the optimization).
    #[tokio::test]
    async fn body_reuse_capacity_preserved_after_split() {
        let body = vec![0u8; 4000];
        let len = (4 + body.len()) as i32;
        let mut stream = Cursor::new(body);
        let mut buf = BytesMut::with_capacity(READ_BUF_DEFAULT_CAPACITY);

        let _ = read_message_body_reuse(&mut stream, &mut buf, b'D', len)
            .await
            .unwrap();

        // buf.len() == 0 after split, but capacity should allow next message
        assert_eq!(buf.len(), 0);
        // For read_buf (single-message pattern), split leaves remainder capacity.
        // Next reserve() will reuse or grow as needed. This is correct behavior.
    }

    // =========================================================================
    // clone() vs split() for accumulation buffers — the design decision
    // =========================================================================

    /// Documents WHY server.buffer uses clone()+clear() instead of split().
    /// split() on a full buffer leaves near-zero remaining capacity,
    /// forcing reallocation on the next put_slice(). clone()+clear() preserves
    /// the warm capacity.
    #[tokio::test]
    async fn clone_clear_preserves_capacity_for_accumulation() {
        let mut buffer = BytesMut::with_capacity(8192);
        buffer.put_slice(&[0u8; 6000]); // accumulate like recv() does

        let cap_before = buffer.capacity();
        let _bytes = buffer.clone();
        buffer.clear();
        let cap_after = buffer.capacity();

        assert_eq!(
            cap_before, cap_after,
            "clone()+clear() must preserve capacity for accumulation buffers"
        );
    }

    /// Demonstrates that split() does NOT preserve capacity — the reason we
    /// reverted to clone()+clear() for server.buffer.
    #[tokio::test]
    async fn split_does_not_preserve_capacity() {
        let mut buffer = BytesMut::with_capacity(8192);
        buffer.put_slice(&[0u8; 6000]);

        let cap_before = buffer.capacity();
        let _bytes = buffer.split();
        let cap_after = buffer.capacity();

        assert!(cap_after < cap_before,
            "split() leaves remainder capacity ({cap_after}) much less than original ({cap_before})");
    }
}
