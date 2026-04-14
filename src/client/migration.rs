use bytes::{Buf, BufMut, BytesMut};
use log::{error, info, warn};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::Arc;
use tokio::io::{split, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

#[cfg(feature = "tls-migration")]
use std::ffi::c_void;

use crate::client::buffer_pool::PooledBuffer;
use crate::client::core::{CachedStatement, Client, PreparedStatementKey};
use crate::client::util::PREPARED_STATEMENT_COUNTER;
use crate::config::get_config;
use crate::errors::Error;
use crate::messages::config_socket::configure_tcp_socket;
use crate::messages::Parse;
use crate::pool::{get_pool, ClientServerMap, ConnectionPool};
use crate::server::ServerParameters;
use crate::stats::ClientStats;

use super::core::PreparedStatementState;

const MIGRATION_MAGIC: u32 = 0x50474D47; // "PGMG"
const MIGRATION_VERSION: u16 = 1;
/// Fixed-size header: magic(4) + version(2) + connection_id(8) + secret_key(4) + transaction_mode(1)
const HEADER_SIZE: usize = 4 + 2 + 8 + 4 + 1;
const MAX_PREPARED_ENTRIES: usize = 100_000;
const MAX_QUERY_LEN: usize = 10 * 1024 * 1024; // 10 MB
const MAX_RECV_BUF: usize = 64 * 1024;

// FFI for our patched OpenSSL migration functions.
// Only available with the tls-migration feature (vendored patched OpenSSL).
#[cfg(feature = "tls-migration")]
#[allow(dead_code)]
extern "C" {
    fn SSL_export_migration_state(ssl: *mut c_void, out: *mut *mut u8, out_len: *mut usize) -> i32;

    fn SSL_import_migration_state(
        ctx: *mut c_void,
        fd: i32,
        buf: *const u8,
        len: usize,
    ) -> *mut c_void;
}

/// Export TLS cipher state from a raw SSL* pointer.
#[cfg(feature = "tls-migration")]
fn export_tls_state_from_ptr(ssl_ptr: *mut c_void) -> Result<Vec<u8>, Error> {
    if ssl_ptr.is_null() {
        return Err(Error::ClientError("null SSL pointer".into()));
    }
    unsafe {
        let mut out: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        // SAFETY: ssl_ptr is a valid SSL* from the TlsStream, which is still alive
        // at the idle point when migration runs.
        let ret = SSL_export_migration_state(ssl_ptr, &mut out, &mut out_len);
        if ret != 1 || out.is_null() {
            return Err(Error::ClientError(
                "SSL_export_migration_state failed".into(),
            ));
        }
        let data = std::slice::from_raw_parts(out, out_len).to_vec();
        openssl_sys::OPENSSL_free(out as *mut c_void);
        Ok(data)
    }
}

/// Payload sent over the migration socket.
/// Drop closes the dup'd fd if it was not consumed by sendmsg.
pub struct MigrationPayload {
    pub state: BytesMut,
    pub fd: RawFd,
    /// Opaque TLS cipher state from SSL_export_migration_state.
    /// None for plain TCP connections.
    pub tls_state: Option<Vec<u8>>,
}

impl Drop for MigrationPayload {
    fn drop(&mut self) {
        if self.fd >= 0 {
            // SAFETY: fd was obtained via libc::dup() in prepare_migration and is
            // owned exclusively by this struct. Closing a valid owned fd is safe.
            unsafe { libc::close(self.fd) };
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

fn put_str(buf: &mut BytesMut, s: &str) {
    buf.put_u16(s.len() as u16);
    buf.put_slice(s.as_bytes());
}

fn get_str(buf: &mut impl Buf) -> Result<String, Error> {
    require(buf, 2)?;
    let len = buf.get_u16() as usize;
    require(buf, len)?;
    let mut v = vec![0u8; len];
    buf.copy_to_slice(&mut v);
    String::from_utf8(v).map_err(|_| Error::ClientError("migration: invalid utf8".into()))
}

/// Check that buf has at least `need` bytes remaining.
fn require(buf: &impl Buf, need: usize) -> Result<(), Error> {
    if buf.remaining() < need {
        return Err(Error::ClientError(format!(
            "migration: need {need} bytes, have {}",
            buf.remaining()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Client → MigrationPayload
// ---------------------------------------------------------------------------

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    /// Serialize client state and dup the fd for migration.
    /// Called at the idle point in handle() — no server checked out,
    /// no pending begin, no buffered reads.
    /// If ssl_ptr is set, exports TLS cipher state via SSL_export_migration_state.
    pub fn prepare_migration(&self) -> Result<MigrationPayload, Error> {
        let raw_fd = self
            .raw_fd
            .ok_or_else(|| Error::ClientError("no raw_fd for migration".into()))?;

        // Export TLS state if this is a TLS connection
        #[cfg(feature = "tls-migration")]
        let tls_state = if let Some(ssl_ptr) = self.ssl_ptr {
            let blob = export_tls_state_from_ptr(ssl_ptr.0)?;
            Some(blob)
        } else {
            None
        };
        #[cfg(not(feature = "tls-migration"))]
        let tls_state: Option<Vec<u8>> = None;

        // SAFETY: raw_fd is a valid open fd stored before tokio::io::split().
        // dup() creates an independent copy; if it fails we return an error.
        let dup_fd = unsafe { libc::dup(raw_fd) };
        if dup_fd < 0 {
            return Err(Error::SocketError(
                "dup() failed during migration".to_string(),
            ));
        }

        let state = self.serialize_state(tls_state.is_some());
        Ok(MigrationPayload {
            state,
            fd: dup_fd,
            tls_state,
        })
    }

    fn serialize_state(&self, use_tls: bool) -> BytesMut {
        let mut buf = BytesMut::with_capacity(512);

        buf.put_u32(MIGRATION_MAGIC);
        buf.put_u16(MIGRATION_VERSION);
        buf.put_u64(self.connection_id);
        buf.put_i32(self.secret_key);
        buf.put_u8(self.transaction_mode as u8);

        put_str(&mut buf, &self.pool_name);
        put_str(&mut buf, &self.username);

        // Address
        buf.put_u16(self.addr.port());
        let ip_str = self.addr.ip().to_string();
        buf.put_u8(ip_str.len() as u8);
        buf.put_slice(ip_str.as_bytes());

        // Server parameters
        let params = self.server_parameters.as_hashmap();
        buf.put_u16(params.len() as u16);
        for (k, v) in params {
            put_str(&mut buf, &k);
            put_str(&mut buf, &v);
        }

        serialize_prepared_state(&mut buf, &self.prepared);

        buf.put_u8(use_tls as u8);

        buf
    }
}

fn serialize_prepared_state(buf: &mut BytesMut, prepared: &PreparedStatementState) {
    buf.put_u8(prepared.enabled as u8);
    buf.put_u8(prepared.async_client as u8);

    let cache_entries: Vec<_> = prepared.cache.iter().collect();
    buf.put_u32(cache_entries.len() as u32);
    for (key, cached) in &cache_entries {
        match key {
            PreparedStatementKey::Named(name) => {
                buf.put_u8(0);
                put_str(buf, name);
            }
            PreparedStatementKey::Anonymous(hash) => {
                buf.put_u8(1);
                buf.put_u64(*hash);
            }
        }
        buf.put_u64(cached.hash);
        let query = cached.parse.query();
        buf.put_u32(query.len() as u32);
        buf.put_slice(query.as_bytes());
        let param_types = cached.parse.param_types();
        buf.put_i16(param_types.len() as i16);
        for &pt in param_types {
            buf.put_i32(pt);
        }
    }
}

// ---------------------------------------------------------------------------
// Deserialization + reconstruction
// ---------------------------------------------------------------------------

struct DeserializedState {
    connection_id: u64,
    secret_key: i32,
    transaction_mode: bool,
    pool_name: String,
    username: String,
    addr: std::net::SocketAddr,
    server_parameters: ServerParameters,
    prepared_enabled: bool,
    async_client: bool,
    prepared_entries: Vec<PreparedEntry>,
    #[allow(dead_code)]
    use_tls: bool,
}

struct PreparedEntry {
    key: PreparedStatementKey,
    hash: u64,
    query: String,
    param_types: Vec<i32>,
}

fn deserialize_state(mut buf: BytesMut) -> Result<DeserializedState, Error> {
    require(&buf, HEADER_SIZE)?;

    let magic = buf.get_u32();
    if magic != MIGRATION_MAGIC {
        return Err(Error::ClientError(format!(
            "migration: bad magic {magic:#x}"
        )));
    }
    let version = buf.get_u16();
    if version != MIGRATION_VERSION {
        return Err(Error::ClientError(format!(
            "migration: unsupported version {version}"
        )));
    }

    let connection_id = buf.get_u64();
    let secret_key = buf.get_i32();
    let transaction_mode = buf.get_u8() != 0;

    let pool_name = get_str(&mut buf)?;
    let username = get_str(&mut buf)?;

    // Address
    require(&buf, 3)?; // port(2) + ip_len(1)
    let port = buf.get_u16();
    let ip_len = buf.get_u8() as usize;
    require(&buf, ip_len)?;
    let mut ip_bytes = vec![0u8; ip_len];
    buf.copy_to_slice(&mut ip_bytes);
    let ip_str = std::str::from_utf8(&ip_bytes).map_err(|_| Error::ClientError("bad ip".into()))?;
    let ip: std::net::IpAddr = ip_str
        .parse()
        .map_err(|_| Error::ClientError("bad ip parse".into()))?;
    let addr = std::net::SocketAddr::new(ip, port);

    // Server parameters
    require(&buf, 2)?;
    let param_count = buf.get_u16() as usize;
    let mut server_parameters = ServerParameters::new();
    for _ in 0..param_count {
        let k = get_str(&mut buf)?;
        let v = get_str(&mut buf)?;
        server_parameters.set_param(&k, &v, true);
    }

    // Prepared statements
    require(&buf, 2 + 4)?; // enabled(1) + async(1) + count(4)
    let prepared_enabled = buf.get_u8() != 0;
    let async_client = buf.get_u8() != 0;
    let cache_count = buf.get_u32() as usize;
    if cache_count > MAX_PREPARED_ENTRIES {
        return Err(Error::ClientError(format!(
            "migration: cache_count {cache_count} exceeds limit {MAX_PREPARED_ENTRIES}"
        )));
    }
    let mut prepared_entries = Vec::with_capacity(cache_count);
    for _ in 0..cache_count {
        require(&buf, 1)?; // key_type
        let key_type = buf.get_u8();
        let key = if key_type == 0 {
            PreparedStatementKey::Named(get_str(&mut buf)?)
        } else {
            require(&buf, 8)?;
            PreparedStatementKey::Anonymous(buf.get_u64())
        };
        require(&buf, 8 + 4)?; // hash(8) + query_len(4)
        let hash = buf.get_u64();
        let query_len = buf.get_u32() as usize;
        if query_len > MAX_QUERY_LEN {
            return Err(Error::ClientError(format!(
                "migration: query_len {query_len} exceeds limit {MAX_QUERY_LEN}"
            )));
        }
        require(&buf, query_len)?;
        let mut query_bytes = vec![0u8; query_len];
        buf.copy_to_slice(&mut query_bytes);
        let query = String::from_utf8(query_bytes)
            .map_err(|_| Error::ClientError("bad query utf8".into()))?;
        require(&buf, 2)?; // num_params
        let num_params = buf.get_i16() as usize;
        require(&buf, num_params * 4)?;
        let mut param_types = Vec::with_capacity(num_params);
        for _ in 0..num_params {
            param_types.push(buf.get_i32());
        }
        prepared_entries.push(PreparedEntry {
            key,
            hash,
            query,
            param_types,
        });
    }

    require(&buf, 1)?;
    let use_tls = buf.get_u8() != 0;

    Ok(DeserializedState {
        connection_id,
        secret_key,
        transaction_mode,
        pool_name,
        username,
        addr,
        server_parameters,
        prepared_enabled,
        async_client,
        prepared_entries,
        use_tls,
    })
}

/// Reconstruct a Client from a migrated fd + serialized state.
pub async fn reconstruct_client(
    fd: RawFd,
    state_buf: BytesMut,
    client_server_map: ClientServerMap,
) -> Result<Client<tokio::io::ReadHalf<TcpStream>, tokio::io::WriteHalf<TcpStream>>, Error> {
    let state = deserialize_state(state_buf)?;

    // SAFETY: fd was received via SCM_RIGHTS from the old process and is a valid,
    // open TCP socket. This call takes ownership — no other code holds this fd.
    let std_stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
    std_stream
        .set_nonblocking(true)
        .map_err(|e| Error::SocketError(format!("set_nonblocking: {e}")))?;
    let stream = TcpStream::from_std(std_stream)
        .map_err(|e| Error::SocketError(format!("from_std: {e}")))?;
    configure_tcp_socket(&stream);

    let raw_fd = Some(stream.as_raw_fd());
    let (read, write) = split(stream);

    let config = get_config();

    // Reconstruct prepared statement cache
    let pool = get_pool(&state.pool_name, &state.username);
    let prepared = reconstruct_prepared_state(
        state.prepared_enabled,
        state.async_client,
        &state.prepared_entries,
        pool.as_ref(),
        config.general.client_prepared_statements_cache_size,
    );

    let application_name = state
        .server_parameters
        .as_hashmap()
        .get("application_name")
        .cloned()
        .unwrap_or_default();

    let stats = Arc::new(ClientStats::new(
        state.connection_id,
        &application_name,
        &state.username,
        &state.pool_name,
        &state.addr.to_string(),
        crate::utils::clock::now(),
        false, // plain TCP
    ));

    Ok(Client {
        read: BufReader::new(read),
        write,
        buffer: PooledBuffer::new(),
        addr: state.addr,
        addr_str: state.addr.to_string(),
        read_buf: BytesMut::with_capacity(8192),
        connection_id: state.connection_id,
        cancel_mode: false,
        transaction_mode: state.transaction_mode,
        secret_key: state.secret_key,
        client_server_map,
        stats,
        admin: false,
        last_server_stats: None,
        connected_to_server: false,
        pool_name: state.pool_name,
        username: state.username,
        server_parameters: state.server_parameters,
        prepared,
        client_last_messages_in_tx: PooledBuffer::new(),
        max_memory_usage: config.general.max_memory_usage.as_bytes(),
        pooler_check_query_request_vec: config.general.poller_check_query_request_bytes_vec(),
        client_pending_begin: None,
        #[cfg(unix)]
        raw_fd,
        #[cfg(all(unix, feature = "tls-migration"))]
        ssl_ptr: None,
    })
}

/// Reconstruct a TLS Client from a migrated fd + serialized state + TLS blob.
#[cfg(all(target_os = "linux", feature = "tls-migration"))]
pub async fn reconstruct_tls_client(
    fd: RawFd,
    state_buf: BytesMut,
    client_server_map: ClientServerMap,
    tls_blob: &[u8],
    tls_acceptor: Option<tokio_native_tls::TlsAcceptor>,
) -> Result<
    Client<
        tokio::io::ReadHalf<tokio_native_tls::TlsStream<TcpStream>>,
        tokio::io::WriteHalf<tokio_native_tls::TlsStream<TcpStream>>,
    >,
    Error,
> {
    let state = deserialize_state(state_buf)?;
    let acceptor = tls_acceptor.ok_or_else(|| Error::ClientError("no TLS acceptor".into()))?;

    // SAFETY: fd was received via SCM_RIGHTS and is a valid TCP socket.
    let std_stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
    std_stream
        .set_nonblocking(true)
        .map_err(|e| Error::SocketError(format!("set_nonblocking: {e}")))?;
    let tcp_stream = TcpStream::from_std(std_stream)
        .map_err(|e| Error::SocketError(format!("from_std: {e}")))?;
    configure_tcp_socket(&tcp_stream);

    let raw_fd = Some(tcp_stream.as_raw_fd());

    let tls_stream = acceptor
        .import_migration_state(tcp_stream, tls_blob, fd)
        .map_err(|e| Error::ClientError(format!("TLS import failed: {e}")))?;

    let ssl_ptr = Some(crate::client::core::SslRawPtr(
        tls_stream.get_ref().ssl_raw_ptr(),
    ));
    let (read, write) = split(tls_stream);

    let config = get_config();
    let pool = get_pool(&state.pool_name, &state.username);
    let prepared = reconstruct_prepared_state(
        state.prepared_enabled,
        state.async_client,
        &state.prepared_entries,
        pool.as_ref(),
        config.general.client_prepared_statements_cache_size,
    );

    let application_name = state
        .server_parameters
        .as_hashmap()
        .get("application_name")
        .cloned()
        .unwrap_or_default();

    let stats = Arc::new(ClientStats::new(
        state.connection_id,
        &application_name,
        &state.username,
        &state.pool_name,
        &state.addr.to_string(),
        crate::utils::clock::now(),
        true, // TLS
    ));

    Ok(Client {
        read: BufReader::new(read),
        write,
        buffer: PooledBuffer::new(),
        addr: state.addr,
        addr_str: state.addr.to_string(),
        read_buf: BytesMut::with_capacity(8192),
        connection_id: state.connection_id,
        cancel_mode: false,
        transaction_mode: state.transaction_mode,
        secret_key: state.secret_key,
        client_server_map,
        stats,
        admin: false,
        last_server_stats: None,
        connected_to_server: false,
        pool_name: state.pool_name,
        username: state.username,
        server_parameters: state.server_parameters,
        prepared,
        client_last_messages_in_tx: PooledBuffer::new(),
        max_memory_usage: config.general.max_memory_usage.as_bytes(),
        pooler_check_query_request_vec: config.general.poller_check_query_request_bytes_vec(),
        client_pending_begin: None,
        #[cfg(unix)]
        raw_fd,
        #[cfg(all(unix, feature = "tls-migration"))]
        ssl_ptr,
    })
}

fn reconstruct_prepared_state(
    enabled: bool,
    async_client: bool,
    entries: &[PreparedEntry],
    pool: Option<&ConnectionPool>,
    cache_size: usize,
) -> PreparedStatementState {
    let mut prepared = PreparedStatementState::new(enabled, cache_size);
    prepared.async_client = async_client;

    let Some(pool) = pool else {
        return prepared;
    };
    for entry in entries {
        let parse = Parse::from_parts(&entry.query, &entry.param_types);
        let hash = entry.hash;
        let Some(shared_parse) = pool.register_parse_to_cache(hash, &parse) else {
            continue;
        };
        let async_name = if async_client {
            Some(format!(
                "DOORMAN_async_{}",
                PREPARED_STATEMENT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ))
        } else {
            None
        };
        let cached = CachedStatement {
            parse: shared_parse,
            hash,
            async_name,
        };
        prepared.cache.put(entry.key.clone(), cached);
    }
    prepared
}

// ---------------------------------------------------------------------------
// SCM_RIGHTS fd passing
// ---------------------------------------------------------------------------

/// Send a migration payload (fd + state) over a Unix socket.
/// After successful send, the fd in payload is set to -1 to prevent double-close.
pub fn send_migration_fd(socket_fd: RawFd, payload: &mut MigrationPayload) -> Result<(), Error> {
    let tls_data = payload.tls_state.as_deref().unwrap_or(&[]);
    let state_len = payload.state.len() as u32;
    let tls_len = tls_data.len() as u32;
    let mut msg_buf = Vec::with_capacity(4 + payload.state.len() + 4 + tls_data.len());
    msg_buf.extend_from_slice(&state_len.to_be_bytes());
    msg_buf.extend_from_slice(&payload.state);
    msg_buf.extend_from_slice(&tls_len.to_be_bytes());
    msg_buf.extend_from_slice(tls_data);

    let iov = libc::iovec {
        iov_base: msg_buf.as_ptr() as *mut libc::c_void,
        iov_len: msg_buf.len(),
    };

    let fd_to_send = payload.fd;
    // SAFETY: CMSG_SPACE returns the correct buffer size for one RawFd.
    let cmsg_space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cmsg_buf = vec![0u8; cmsg_space];

    // SAFETY: zeroed msghdr is a valid initial state for sendmsg.
    let mut msghdr: libc::msghdr = unsafe { std::mem::zeroed() };
    msghdr.msg_iov = &iov as *const _ as *mut _;
    msghdr.msg_iovlen = 1;
    msghdr.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msghdr.msg_controllen = cmsg_space as _;

    // SAFETY: cmsg_buf is correctly sized via CMSG_SPACE. CMSG_FIRSTHDR, CMSG_LEN,
    // CMSG_DATA return valid pointers into cmsg_buf. fd_to_send is a valid open fd.
    // sendmsg is called with a valid msghdr pointing to valid iov and cmsg buffers.
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msghdr);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::copy_nonoverlapping(
            &fd_to_send as *const RawFd as *const u8,
            libc::CMSG_DATA(cmsg),
            std::mem::size_of::<RawFd>(),
        );

        let ret = libc::sendmsg(socket_fd, &msghdr, 0);
        if ret < 0 {
            return Err(Error::SocketError(format!(
                "sendmsg: {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    // SAFETY: sendmsg duplicated the fd into the receiver's fd table.
    // We close our copy to avoid a leak. Setting fd = -1 prevents Drop from
    // closing it again.
    unsafe { libc::close(payload.fd) };
    payload.fd = -1;
    Ok(())
}

/// Receive a migration payload (fd + state + optional TLS state) from a Unix socket.
/// Returns (raw_fd, state_bytes, tls_state) or error on EOF/failure.
pub fn recv_migration_fd(socket_fd: RawFd) -> Result<(RawFd, BytesMut, Option<Vec<u8>>), Error> {
    let mut recv_buf = vec![0u8; MAX_RECV_BUF];
    let iov = libc::iovec {
        iov_base: recv_buf.as_mut_ptr() as *mut libc::c_void,
        iov_len: recv_buf.len(),
    };

    // SAFETY: CMSG_SPACE returns the correct buffer size for one RawFd.
    let cmsg_space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cmsg_buf = vec![0u8; cmsg_space];

    // SAFETY: zeroed msghdr is a valid initial state for recvmsg.
    let mut msghdr: libc::msghdr = unsafe { std::mem::zeroed() };
    msghdr.msg_iov = &iov as *const _ as *mut _;
    msghdr.msg_iovlen = 1;
    msghdr.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msghdr.msg_controllen = cmsg_space as _;

    // SAFETY: msghdr points to valid iov and cmsg buffers. recvmsg fills them.
    let n = unsafe { libc::recvmsg(socket_fd, &mut msghdr, 0) };
    if n <= 0 {
        return Err(Error::SocketError(if n == 0 {
            "migration socket closed".to_string()
        } else {
            format!("recvmsg: {}", std::io::Error::last_os_error())
        }));
    }
    let n = n as usize;

    // Extract fd from cmsg
    let mut received_fd: RawFd = -1;
    // SAFETY: CMSG_FIRSTHDR and CMSG_NXTHDR return valid pointers into the
    // cmsg_buf that was filled by recvmsg, or null when exhausted.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msghdr);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                std::ptr::copy_nonoverlapping(
                    libc::CMSG_DATA(cmsg),
                    &mut received_fd as *mut RawFd as *mut u8,
                    std::mem::size_of::<RawFd>(),
                );
            }
            cmsg = libc::CMSG_NXTHDR(&msghdr, cmsg);
        }
    }

    if received_fd < 0 {
        return Err(Error::SocketError("migration: no fd in cmsg".to_string()));
    }

    if n < 4 {
        // SAFETY: received_fd is a valid fd from cmsg, close it to avoid leak.
        unsafe { libc::close(received_fd) };
        return Err(Error::SocketError(
            "migration: message too short for length prefix".to_string(),
        ));
    }

    let state_len =
        u32::from_be_bytes([recv_buf[0], recv_buf[1], recv_buf[2], recv_buf[3]]) as usize;
    let data_received = n - 4;

    let mut state = BytesMut::with_capacity(state_len);
    if data_received <= state_len {
        state.put_slice(&recv_buf[4..4 + data_received]);
    } else {
        state.put_slice(&recv_buf[4..4 + state_len]);
    }

    // If we didn't get all data in one recvmsg, read the rest
    while state.len() < state_len {
        let remaining = state_len - state.len();
        let chunk_size = remaining.min(recv_buf.len());
        // SAFETY: recv_buf is valid, socket_fd is a valid connected Unix socket.
        let n = unsafe {
            libc::recv(
                socket_fd,
                recv_buf.as_mut_ptr() as *mut libc::c_void,
                chunk_size,
                0,
            )
        };
        if n <= 0 {
            // SAFETY: received_fd is valid, close to avoid leak on error.
            unsafe { libc::close(received_fd) };
            return Err(Error::SocketError("migration: truncated state".into()));
        }
        state.put_slice(&recv_buf[..n as usize]);
    }

    // Read TLS state length + data (follows the app state)
    // May need to read more data from socket if not all arrived in first recvmsg
    let mut tls_header = [0u8; 4];
    let mut tls_header_read = 0usize;

    // Check if tls_len header is already in our buffer
    let leftover = data_received.saturating_sub(state_len);
    if leftover >= 4 {
        // TLS length header is in the buffer
        let off = 4 + state_len;
        tls_header.copy_from_slice(&recv_buf[off..off + 4]);
        tls_header_read = 4;
    } else if leftover > 0 {
        tls_header[..leftover].copy_from_slice(&recv_buf[4 + state_len..4 + state_len + leftover]);
        tls_header_read = leftover;
    }

    // Read remaining TLS header bytes if needed
    while tls_header_read < 4 {
        // SAFETY: recv_buf is valid, socket_fd is a valid connected Unix socket.
        let n = unsafe {
            libc::recv(
                socket_fd,
                recv_buf.as_mut_ptr() as *mut libc::c_void,
                4 - tls_header_read,
                0,
            )
        };
        if n <= 0 {
            unsafe { libc::close(received_fd) };
            return Err(Error::SocketError("migration: truncated tls header".into()));
        }
        let n = n as usize;
        tls_header[tls_header_read..tls_header_read + n].copy_from_slice(&recv_buf[..n]);
        tls_header_read += n;
    }

    let tls_len = u32::from_be_bytes(tls_header) as usize;
    let tls_state = if tls_len > 0 {
        let mut tls_buf = vec![0u8; tls_len];
        // Check if some TLS data was already in the original recv buffer
        let tls_data_offset = 4 + state_len + 4;
        let mut tls_read = if n > tls_data_offset {
            let avail = (n - tls_data_offset).min(tls_len);
            tls_buf[..avail].copy_from_slice(&recv_buf[tls_data_offset..tls_data_offset + avail]);
            avail
        } else {
            0
        };

        while tls_read < tls_len {
            let remaining = tls_len - tls_read;
            let chunk = remaining.min(recv_buf.len());
            // SAFETY: recv_buf and socket_fd are valid.
            let nr = unsafe {
                libc::recv(
                    socket_fd,
                    recv_buf.as_mut_ptr() as *mut libc::c_void,
                    chunk,
                    0,
                )
            };
            if nr <= 0 {
                unsafe { libc::close(received_fd) };
                return Err(Error::SocketError("migration: truncated tls state".into()));
            }
            let nr = nr as usize;
            tls_buf[tls_read..tls_read + nr].copy_from_slice(&recv_buf[..nr]);
            tls_read += nr;
        }
        Some(tls_buf)
    } else {
        None
    };

    Ok((received_fd, state, tls_state))
}

// ---------------------------------------------------------------------------
// Sender / receiver tasks
// ---------------------------------------------------------------------------

/// Sender task: runs in the OLD process.
/// Reads MigrationPayload from channel, sends over Unix socket.
pub async fn migration_sender_task(socket_fd: RawFd, mut rx: mpsc::Receiver<MigrationPayload>) {
    while let Some(mut payload) = rx.recv().await {
        match send_migration_fd(socket_fd, &mut payload) {
            Ok(()) => {}
            Err(e) => {
                warn!("migration send failed: {e}");
                // payload Drop will close the dup'd fd
            }
        }
    }
    info!("migration sender: channel closed, closing socket");
    // SAFETY: socket_fd is the parent end of the socketpair, owned by this task.
    unsafe { libc::close(socket_fd) };
}

/// Receiver task: runs in the NEW process.
/// Reads migrated clients from Unix socket, reconstructs and spawns them.
pub async fn migration_receiver_task(
    socket_fd: RawFd,
    client_server_map: ClientServerMap,
    _tls_acceptor: Option<tokio_native_tls::TlsAcceptor>,
) {
    #[cfg(all(target_os = "linux", feature = "tls-migration"))]
    let tls_acceptor = _tls_acceptor;
    use crate::app::server::CURRENT_CLIENT_COUNT;
    use std::sync::atomic::Ordering;

    info!("migration receiver: listening for migrated clients");

    loop {
        let result = tokio::task::spawn_blocking(move || recv_migration_fd(socket_fd)).await;

        match result {
            Ok(Ok((fd, state_buf, tls_state))) => {
                if let Some(_tls_blob) = tls_state {
                    #[cfg(all(target_os = "linux", feature = "tls-migration"))]
                    {
                        let csm = client_server_map.clone();
                        let acceptor = tls_acceptor.clone();
                        let tls_blob = _tls_blob;
                        tokio::spawn(async move {
                            match reconstruct_tls_client(fd, state_buf, csm, &tls_blob, acceptor)
                                .await
                            {
                                Ok(mut client) => {
                                    CURRENT_CLIENT_COUNT.fetch_add(1, Ordering::SeqCst);
                                    info!(
                                        "[{}@{} #c{}] migrated TLS client from {}",
                                        client.username,
                                        client.pool_name,
                                        client.connection_id,
                                        client.addr
                                    );
                                    let result = client.handle().await;
                                    if !client.is_admin() && result.is_err() {
                                        client.disconnect_stats();
                                    }
                                    CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                                }
                                Err(e) => {
                                    error!("failed to reconstruct migrated TLS client: {e}");
                                    unsafe { libc::close(fd) };
                                }
                            }
                        });
                    }
                    #[cfg(not(all(target_os = "linux", feature = "tls-migration")))]
                    {
                        warn!("TLS migration not available; closing fd");
                        let _ = (state_buf, _tls_blob);
                        unsafe { libc::close(fd) };
                    }
                    continue;
                }
                let csm = client_server_map.clone();
                tokio::spawn(async move {
                    match reconstruct_client(fd, state_buf, csm).await {
                        Ok(mut client) => {
                            CURRENT_CLIENT_COUNT.fetch_add(1, Ordering::SeqCst);
                            info!(
                                "[{}@{} #c{}] migrated client accepted from {}",
                                client.username,
                                client.pool_name,
                                client.connection_id,
                                client.addr
                            );
                            let result = client.handle().await;
                            if !client.is_admin() && result.is_err() {
                                warn!(
                                    "[{}@{} #c{}] migrated client {} error: {}",
                                    client.username,
                                    client.pool_name,
                                    client.connection_id,
                                    client.addr,
                                    result.as_ref().unwrap_err()
                                );
                                client.disconnect_stats();
                            }
                            CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                        }
                        Err(e) => {
                            error!("failed to reconstruct migrated client: {e}");
                            // SAFETY: fd was received via SCM_RIGHTS, close to avoid leak.
                            unsafe { libc::close(fd) };
                        }
                    }
                });
            }
            Ok(Err(e)) => {
                info!("migration receiver done: {e}");
                break;
            }
            Err(e) => {
                error!("migration receiver panic: {e}");
                break;
            }
        }
    }

    // SAFETY: socket_fd is the child end of the socketpair, owned by this task.
    unsafe { libc::close(socket_fd) };
    info!("migration receiver: stopped");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_str_get_str_roundtrip() {
        let mut buf = BytesMut::new();
        put_str(&mut buf, "hello");
        put_str(&mut buf, "");
        put_str(&mut buf, "мир"); // multibyte utf-8

        let mut cur = buf.freeze();
        assert_eq!(get_str(&mut cur).unwrap(), "hello");
        assert_eq!(get_str(&mut cur).unwrap(), "");
        assert_eq!(get_str(&mut cur).unwrap(), "мир");
        assert_eq!(cur.remaining(), 0);
    }

    #[test]
    fn get_str_truncated() {
        let mut buf = BytesMut::new();
        buf.put_u16(100); // claims 100 bytes but has none
        let mut cur = buf.freeze();
        assert!(get_str(&mut cur).is_err());
    }

    #[test]
    fn get_str_empty_buf() {
        let mut cur = BytesMut::new().freeze();
        assert!(get_str(&mut cur).is_err());
    }

    #[test]
    fn deserialize_rejects_bad_magic() {
        let mut buf = BytesMut::new();
        buf.put_u32(0xDEADBEEF);
        buf.put_u16(1);
        buf.put_slice(&[0; 13]); // fill to HEADER_SIZE
        assert!(deserialize_state(buf).is_err());
    }

    #[test]
    fn deserialize_rejects_bad_version() {
        let mut buf = BytesMut::new();
        buf.put_u32(MIGRATION_MAGIC);
        buf.put_u16(99);
        buf.put_slice(&[0; 13]);
        assert!(deserialize_state(buf).is_err());
    }

    #[test]
    fn deserialize_rejects_truncated_header() {
        let buf = BytesMut::from(&[0u8; 5][..]);
        assert!(deserialize_state(buf).is_err());
    }

    #[test]
    fn deserialize_rejects_truncated_body() {
        let mut buf = BytesMut::new();
        buf.put_u32(MIGRATION_MAGIC);
        buf.put_u16(MIGRATION_VERSION);
        buf.put_u64(42); // connection_id
        buf.put_i32(1); // secret_key
        buf.put_u8(1); // transaction_mode
                       // missing pool_name, username, etc.
        assert!(deserialize_state(buf).is_err());
    }

    #[test]
    fn deserialize_rejects_excessive_cache_count() {
        let mut buf = BytesMut::new();
        buf.put_u32(MIGRATION_MAGIC);
        buf.put_u16(MIGRATION_VERSION);
        buf.put_u64(1); // connection_id
        buf.put_i32(1); // secret_key
        buf.put_u8(0); // transaction_mode

        put_str(&mut buf, "mydb"); // pool_name
        put_str(&mut buf, "user"); // username

        buf.put_u16(5432); // port
        buf.put_u8(9); // ip_len
        buf.put_slice(b"127.0.0.1");

        buf.put_u16(0); // 0 server params

        buf.put_u8(1); // prepared_enabled
        buf.put_u8(0); // async_client
        buf.put_u32(u32::MAX); // cache_count = 4 billion

        assert!(deserialize_state(buf).is_err());
    }

    #[test]
    fn serialize_deserialize_roundtrip_minimal() {
        // Build a minimal serialized state by hand (no Client needed)
        let mut buf = BytesMut::new();
        buf.put_u32(MIGRATION_MAGIC);
        buf.put_u16(MIGRATION_VERSION);
        buf.put_u64(12345); // connection_id
        buf.put_i32(-42); // secret_key
        buf.put_u8(1); // transaction_mode = true

        put_str(&mut buf, "testdb");
        put_str(&mut buf, "testuser");

        // Address: 192.168.1.1:5432
        buf.put_u16(5432);
        let ip = "192.168.1.1";
        buf.put_u8(ip.len() as u8);
        buf.put_slice(ip.as_bytes());

        // 1 server parameter
        buf.put_u16(1);
        put_str(&mut buf, "application_name");
        put_str(&mut buf, "myapp");

        // Prepared statements: enabled, not async, 1 entry
        buf.put_u8(1); // enabled
        buf.put_u8(0); // async_client
        buf.put_u32(1); // cache_count

        // Entry: Named("stmt1"), hash=0xABCD, query="SELECT 1", params=[23]
        buf.put_u8(0); // Named
        put_str(&mut buf, "stmt1");
        buf.put_u64(0xABCD);
        let query = "SELECT 1";
        buf.put_u32(query.len() as u32);
        buf.put_slice(query.as_bytes());
        buf.put_i16(1); // 1 param
        buf.put_i32(23); // int4 OID

        buf.put_u8(0); // use_tls = false

        let state = deserialize_state(buf).unwrap();
        assert_eq!(state.connection_id, 12345);
        assert_eq!(state.secret_key, -42);
        assert!(state.transaction_mode);
        assert_eq!(state.pool_name, "testdb");
        assert_eq!(state.username, "testuser");
        assert_eq!(state.addr.port(), 5432);
        assert_eq!(state.addr.ip().to_string(), "192.168.1.1");
        assert!(state.prepared_enabled);
        assert!(!state.async_client);
        assert_eq!(state.prepared_entries.len(), 1);
        assert_eq!(
            state.prepared_entries[0].key,
            PreparedStatementKey::Named("stmt1".into())
        );
        assert_eq!(state.prepared_entries[0].hash, 0xABCD);
        assert_eq!(state.prepared_entries[0].query, "SELECT 1");
        assert_eq!(state.prepared_entries[0].param_types, vec![23]);
    }

    #[test]
    fn serialize_deserialize_ipv6() {
        let mut buf = BytesMut::new();
        buf.put_u32(MIGRATION_MAGIC);
        buf.put_u16(MIGRATION_VERSION);
        buf.put_u64(1);
        buf.put_i32(1);
        buf.put_u8(0);

        put_str(&mut buf, "db");
        put_str(&mut buf, "u");

        let ipv6 = "::1";
        buf.put_u16(5433);
        buf.put_u8(ipv6.len() as u8);
        buf.put_slice(ipv6.as_bytes());

        buf.put_u16(0); // no params
        buf.put_u8(0); // prepared disabled
        buf.put_u8(0); // not async
        buf.put_u32(0); // no cache entries
        buf.put_u8(0); // no tls

        let state = deserialize_state(buf).unwrap();
        assert_eq!(state.addr.ip().to_string(), "::1");
        assert_eq!(state.addr.port(), 5433);
    }

    #[test]
    fn serialize_deserialize_anonymous_prepared() {
        let mut buf = BytesMut::new();
        buf.put_u32(MIGRATION_MAGIC);
        buf.put_u16(MIGRATION_VERSION);
        buf.put_u64(1);
        buf.put_i32(1);
        buf.put_u8(0);
        put_str(&mut buf, "db");
        put_str(&mut buf, "u");
        buf.put_u16(5432);
        buf.put_u8(9);
        buf.put_slice(b"127.0.0.1");
        buf.put_u16(0);

        buf.put_u8(1); // enabled
        buf.put_u8(0); // not async
        buf.put_u32(1); // 1 entry

        // Anonymous entry
        buf.put_u8(1); // Anonymous
        buf.put_u64(0xDEAD); // key_hash
        buf.put_u64(0xDEAD); // hash
        let q = "SELECT $1";
        buf.put_u32(q.len() as u32);
        buf.put_slice(q.as_bytes());
        buf.put_i16(0); // no params

        buf.put_u8(0); // no tls

        let state = deserialize_state(buf).unwrap();
        assert_eq!(state.prepared_entries.len(), 1);
        assert_eq!(
            state.prepared_entries[0].key,
            PreparedStatementKey::Anonymous(0xDEAD)
        );
    }
}
