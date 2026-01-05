// Implementation of the PostgreSQL server (database) protocol.

// Standard library imports
use std::collections::{HashMap, HashSet, VecDeque};
use std::mem;
use std::num::NonZeroUsize;
use std::string::ToString;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// External crate imports
use bytes::{Buf, BufMut, BytesMut};
use log::{error, info, warn};
use lru::LruCache;
use once_cell::sync::Lazy;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, BufStream};
use tokio::net::{TcpStream, UnixStream};
use tokio::time::timeout;
// Internal crate imports
use crate::auth::jwt::{new_claims, sign_with_jwt_priv_key};
use crate::config::{get_config, Address, User, VERSION};
use crate::constants::*;
use crate::errors::Error::MaxMessageSize;
use crate::errors::{Error, ServerIdentifier};
use crate::messages::BytesMutReader;
use crate::messages::*;
use crate::pool::{ClientServerMap, CANCELED_PIDS};
use crate::scram_client::ScramSha256;
use crate::stats::ServerStats;

const COMMAND_COMPLETE_BY_SET: &[u8; 4] = b"SET\0";
const COMMAND_COMPLETE_BY_DECLARE: &[u8; 15] = b"DECLARE CURSOR\0";
const COMMAND_SAVEPOINT: &[u8; 10] = b"SAVEPOINT\0";
const COMMAND_COMPLETE_BY_DEALLOCATE_ALL: &[u8; 15] = b"DEALLOCATE ALL\0";
const COMMAND_COMPLETE_BY_DISCARD_ALL: &[u8; 12] = b"DISCARD ALL\0";

pin_project! {
    #[project = SteamInnerProj]
    #[derive(Debug)]
    pub enum StreamInner {
        TCPPlain {
            #[pin]
            stream: TcpStream,
        },
        UnixSocket {
            #[pin]
            stream: UnixStream,
        },
    }
}

impl AsyncWrite for StreamInner {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let this = self.project();
        match this {
            SteamInnerProj::TCPPlain { stream } => stream.poll_write(cx, buf),
            SteamInnerProj::UnixSocket { stream } => stream.poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let this = self.project();
        match this {
            SteamInnerProj::TCPPlain { stream } => stream.poll_flush(cx),
            SteamInnerProj::UnixSocket { stream } => stream.poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let this = self.project();
        match this {
            SteamInnerProj::TCPPlain { stream } => stream.poll_shutdown(cx),
            SteamInnerProj::UnixSocket { stream } => stream.poll_shutdown(cx),
        }
    }
}

impl AsyncRead for StreamInner {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.project();
        match this {
            SteamInnerProj::TCPPlain { stream } => stream.poll_read(cx, buf),
            SteamInnerProj::UnixSocket { stream } => stream.poll_read(cx, buf),
        }
    }
}

impl StreamInner {
    pub fn try_write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            StreamInner::TCPPlain { stream } => stream.try_write(buf),
            StreamInner::UnixSocket { stream } => stream.try_write(buf),
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct CleanupState {
    /// If server connection requires RESET ALL before checkin because of set statement
    needs_cleanup_set: bool,

    /// If server connection requires DEALLOCATE ALL before checkin because of prepare statement
    needs_cleanup_prepare: bool,

    /// If server connection requires CLOSE ALL before checkin because of declare statement
    needs_cleanup_declare: bool,
}

impl CleanupState {
    fn new() -> Self {
        CleanupState {
            needs_cleanup_set: false,
            needs_cleanup_prepare: false,
            needs_cleanup_declare: false,
        }
    }

    #[inline(always)]
    fn needs_cleanup(&self) -> bool {
        self.needs_cleanup_set || self.needs_cleanup_prepare || self.needs_cleanup_declare
    }

    #[inline(always)]
    fn set_true(&mut self) {
        self.needs_cleanup_set = true;
        self.needs_cleanup_prepare = true;
        self.needs_cleanup_declare = true;
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.needs_cleanup_set = false;
        self.needs_cleanup_prepare = false;
        self.needs_cleanup_declare = false;
    }
}

impl std::fmt::Display for CleanupState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SET: {}, PREPARE: {}, DECLARE: {}",
            self.needs_cleanup_set, self.needs_cleanup_prepare, self.needs_cleanup_declare
        )
    }
}

static TRACKED_PARAMETERS: Lazy<HashSet<String>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("client_encoding".to_string());
    set.insert("DateStyle".to_string());
    set.insert("TimeZone".to_string());
    set.insert("standard_conforming_strings".to_string());
    set.insert("application_name".to_string());
    set
});

#[derive(Debug, Clone)]
pub struct ServerParameters {
    parameters: HashMap<String, String>,
}

impl Default for ServerParameters {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerParameters {
    pub fn new() -> Self {
        ServerParameters {
            parameters: HashMap::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
    }
    pub fn admin() -> Self {
        let mut server_parameters = ServerParameters {
            parameters: HashMap::new(),
        };

        server_parameters.set_param("client_encoding".to_string(), "UTF8".to_string(), false);
        server_parameters.set_param("DateStyle".to_string(), "ISO, MDY".to_string(), false);
        server_parameters.set_param("TimeZone".to_string(), "Etc/UTC".to_string(), false);
        server_parameters.set_param("server_version".to_string(), VERSION.to_string(), true);
        server_parameters.set_param("server_encoding".to_string(), "UTF-8".to_string(), true);
        server_parameters.set_param(
            "standard_conforming_strings".to_string(),
            "on".to_string(),
            false,
        );
        // (64 bit = on) as of PostgreSQL 10, this is always on.
        server_parameters.set_param("integer_datetimes".to_string(), "on".to_string(), false);
        server_parameters.set_param(
            "application_name".to_string(),
            "pg_doorman".to_string(),
            false,
        );

        server_parameters
    }

    /// returns true if a tracked parameter was set, false if it was a non-tracked parameter
    /// if startup is false, then then only tracked parameters will be set
    pub fn set_param(&mut self, mut key: String, value: String, startup: bool) {
        // The startup parameter will send uncapitalized keys but parameter status packets will send capitalized keys
        if key == "timezone" {
            key = "TimeZone".to_string();
        } else if key == "datestyle" {
            key = "DateStyle".to_string();
        };

        if TRACKED_PARAMETERS.contains(&key) || startup {
            self.parameters.insert(key, value);
        }
    }

    pub fn set_from_hashmap(&mut self, parameters: HashMap<String, String>, startup: bool) {
        for (key, value) in parameters {
            self.set_param(key.to_string(), value.to_string(), startup);
        }
    }

    // Gets the diff of the parameters
    #[inline(always)]
    fn compare_params(&self, incoming_parameters: &ServerParameters) -> HashMap<String, String> {
        let mut diff = HashMap::new();

        // iterate through tracked parameters
        for key in TRACKED_PARAMETERS.iter() {
            if let Some(incoming_value) = incoming_parameters.parameters.get(key) {
                if let Some(value) = self.parameters.get(key) {
                    if value != incoming_value {
                        diff.insert(key.to_string(), incoming_value.to_string());
                    }
                }
            }
        }

        diff
    }

    pub fn get_application_name(&self) -> &String {
        // Can unwrap because we set it in the constructor
        self.parameters.get("application_name").unwrap()
    }

    fn add_parameter_message(key: &str, value: &str, buffer: &mut BytesMut) {
        buffer.put_u8(b'S');

        // 4 is len of i32, the plus for the null terminator
        let len = 4 + key.len() + 1 + value.len() + 1;

        buffer.put_i32(len as i32);

        buffer.put_slice(key.as_bytes());
        buffer.put_u8(0);
        buffer.put_slice(value.as_bytes());
        buffer.put_u8(0);
    }
}

impl From<&ServerParameters> for BytesMut {
    fn from(server_parameters: &ServerParameters) -> Self {
        let mut bytes = BytesMut::new();

        for (key, value) in &server_parameters.parameters {
            ServerParameters::add_parameter_message(key, value, &mut bytes);
        }

        bytes
    }
}

// pub fn compare

/// Server state.
#[derive(Debug)]
pub struct Server {
    /// Server host, e.g. localhost,
    /// port, e.g. 5432, and role, e.g. primary or replica.
    address: Address,

    /// Server connection.
    stream: BufStream<StreamInner>,

    /// Our server response buffer. We buffer data before we give it to the client.
    buffer: BytesMut,

    /// Server information the server sent us over on startup.
    server_parameters: ServerParameters,

    /// Backend id and secret key used for query cancellation.
    process_id: i32,
    secret_key: i32,

    /// Is the server inside a transaction or idle.
    in_transaction: bool,

    /// Is the server inside a transaction and aborted.
    is_aborted: bool,

    /// Is there more data for the client to read.
    data_available: bool,

    /// Is the server in copy-in or copy-out modes
    in_copy_mode: bool,

    /// Is the server in async mode (Flush instead of Sync)
    async_mode: bool,

    /// Is the server broken? We'll remote it from the pool if so.
    bad: bool,

    /// If server connection requires reset statements before checkin
    cleanup_state: CleanupState,

    /// Mapping of clients and servers used for query cancellation.
    client_server_map: ClientServerMap,

    /// Server connected at.
    connected_at: chrono::naive::NaiveDateTime,

    /// Reports various metrics, e.g. data sent & received.
    pub stats: Arc<ServerStats>,

    /// Application name using the server at the moment.
    application_name: String,

    /// Last time that a successful server send or response happened
    pub last_activity: SystemTime,

    /// Should clean up dirty connections?
    cleanup_connections: bool,

    /// Transaction use savepoint?
    use_savepoint: bool,

    /// Log client parameter status changes
    log_client_parameter_status_changes: bool,

    /// Prepared statements
    prepared_statement_cache: Option<LruCache<String, ()>>,

    /// Prepared statement being currently registered on the server.
    registering_prepared_statement: VecDeque<String>,

    /// Max message size
    max_message_size: i32,
}

impl std::fmt::Display for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "[{}]-vp-{}-{}@{}:{}/{}",
            self.process_id,
            self.address.virtual_pool_id,
            self.address.username,
            self.address.host,
            self.address.port,
            self.address.database
        )
    }
}

impl Server {
    /// Execute an arbitrary query against the server.
    /// It will use the simple query protocol.
    /// Result will not be returned, so this is useful for things like `SET` or `ROLLBACK`.
    pub async fn small_simple_query(&mut self, query: &str) -> Result<(), Error> {
        let query = simple_query(query);

        self.send_and_flush(&query).await?;

        let mut noop = tokio::io::sink();
        loop {
            match self.recv(&mut noop, None).await {
                Ok(_) => (),
                Err(err) => return Err(err),
            }

            if !self.data_available {
                break;
            }
        }

        Ok(())
    }

    #[inline(always)]
    pub fn get_process_id(&self) -> i32 {
        self.process_id
    }

    #[inline(always)]
    pub fn server_parameters_as_hashmap(&self) -> HashMap<String, String> {
        self.server_parameters.parameters.clone()
    }

    /// Receive data from the server in response to a client request.
    /// This method must be called multiple times while `self.is_data_available()` is true
    /// in order to receive all data the server has to offer.
    pub async fn recv<C>(
        &mut self,
        mut client_stream: C,
        mut client_server_parameters: Option<&mut ServerParameters>,
    ) -> Result<BytesMut, Error>
    where
        C: tokio::io::AsyncWrite + std::marker::Unpin,
    {
        loop {
            self.stats.wait_reading();

            // In async mode, use a short timeout to avoid blocking when no more data available
            let (code_u8, message_len) = if self.is_async() {
                match tokio::time::timeout(
                    Duration::from_millis(100),
                    read_message_header(&mut self.stream),
                )
                .await
                {
                    Ok(result) => result?,
                    Err(_) => {
                        // Timeout - no more data available in async mode
                        self.data_available = false;
                        break;
                    }
                }
            } else {
                read_message_header(&mut self.stream).await?
            };
            // if message server is too big.
            if self.max_message_size > 0
                && message_len > self.max_message_size
                && code_u8 as char == 'D'
            {
                // send current buffer + header.
                self.buffer.put_u8(code_u8);
                self.buffer.put_i32(message_len);
                let prev_bad = self.bad;
                self.bad = true;
                write_all_flush(&mut client_stream, &self.buffer).await?;
                match proxy_copy_data_with_timeout(
                    Duration::from_millis(get_config().general.proxy_copy_data_timeout),
                    &mut self.stream,
                    &mut client_stream,
                    message_len as usize - mem::size_of::<i32>(),
                )
                .await
                {
                    Ok(_) => (),
                    Err(err) => {
                        self.mark_bad(err.to_string().as_str());
                        return Err(err);
                    }
                }
                if !prev_bad {
                    self.bad = false;
                }
                self.stats
                    .data_received(self.buffer.len() + message_len as usize);
                self.last_activity = SystemTime::now();
                self.data_available = true;
                self.buffer.clear();
                self.stats.wait_idle();
                return Ok(self.buffer.clone());
            }
            // COPY protocol, 'd'
            if self.max_message_size > 0
                && message_len > self.max_message_size
                && code_u8 as char == 'd'
            {
                // send current buffer + header.
                self.buffer.put_u8(code_u8);
                self.buffer.put_i32(message_len);
                let prev_bad = self.bad;
                self.bad = true;
                write_all_flush(&mut client_stream, &self.buffer).await?;
                proxy_copy_data(
                    &mut self.stream,
                    &mut client_stream,
                    message_len as usize - mem::size_of::<i32>(),
                )
                .await?;
                self.bad = prev_bad;
                self.stats
                    .data_received(self.buffer.len() + message_len as usize);
                self.last_activity = SystemTime::now();
                self.buffer.clear();
                self.stats.wait_idle();
                return Ok(self.buffer.clone());
            }

            if message_len > MAX_MESSAGE_SIZE {
                error!(
                    "Message size limit exceeded for server connection to {} (database: {}, user: {}). Received message size: {} bytes, maximum allowed: {} bytes. Connection will be terminated.",
                    self.address.host, self.address.database, self.address.username, message_len, MAX_MESSAGE_SIZE
                );
                self.mark_bad(format!("Message size limit exceeded: {message_len} bytes (max: {MAX_MESSAGE_SIZE} bytes)").as_str());
                return Err(MaxMessageSize);
            }

            let mut message = match read_message_data(&mut self.stream, code_u8, message_len).await
            {
                Ok(message) => {
                    self.stats.wait_idle();
                    message
                }
                Err(err) => {
                    error!(
                        "Terminating server connection to {} (database: {}, user: {}) while reading message data. Error details: {err}",
                        self.address.host, self.address.database, self.address.username
                    );
                    self.mark_bad(format!("Failed to read message data: {err}").as_str());
                    return Err(err);
                }
            };

            // Buffer the message we'll forward to the client later.
            self.buffer.put(&message[..]);

            let code = message.get_u8() as char;
            let _len = message.get_i32();

            match code {
                // ReadyForQuery
                'Z' => {
                    let transaction_state = message.get_u8() as char;

                    match transaction_state {
                        // In transaction.
                        'T' => {
                            self.is_aborted = false;
                            self.in_transaction = true;
                        }

                        // Idle, transaction over.
                        'I' => {
                            self.is_aborted = false;
                            self.in_transaction = false;
                        }

                        // Some error occurred, the transaction was rolled back.
                        'E' => {
                            self.is_aborted = true;
                            self.in_transaction = true;
                            if let Ok(msg) = PgErrorMsg::parse(&message) {
                                error!(
                                    "Transaction error on server {} (database: {}, user: {}). Transaction was rolled back. Details: [Severity: {}, Code: {}, Message: \"{}\", Hint: \"{}\", Position: {}]",
                                    self.address.host,
                                    self.address.database,
                                    self.address.username,
                                    msg.severity,
                                    msg.code,
                                    msg.message,
                                    msg.hint.as_deref().unwrap_or("none"),
                                    msg.position.unwrap_or(0)
                                );
                            } else {
                                error!(
                                    "Transaction error on server {} (database: {}, user: {}). Transaction was rolled back. Could not parse error details.",
                                    self.address.host, self.address.database, self.address.username
                                );
                            }
                        }

                        // Something totally unexpected, this is not a Postgres server we know.
                        _ => {
                            let err = Error::ProtocolSyncError(format!(
                                "Protocol synchronization error with server {} (database: {}, user: {}). Received unknown transaction state character: '{}' (ASCII: {}). This may indicate an incompatible PostgreSQL server version or a corrupted message.",
                                self.address.host,
                                self.address.database,
                                self.address.username,
                                transaction_state,
                                transaction_state as u8
                            ));
                            error!("{err}");
                            self.mark_bad(format!("Protocol sync error: unknown transaction state '{transaction_state}'").as_str());
                            return Err(err);
                        }
                    };

                    // There is no more data available from the server.
                    self.data_available = false;
                    break;
                }

                // ErrorResponse
                'E' => {
                    if let Ok(msg) = PgErrorMsg::parse(&message) {
                        let transaction_status = if self.in_transaction {
                            "in active transaction"
                        } else {
                            "not in transaction"
                        };
                        let copy_mode_status = if self.in_copy_mode {
                            "in COPY mode"
                        } else {
                            "not in COPY mode"
                        };

                        error!(
                            "PostgreSQL server error from {} (database: {}, user: {}). Status: [{}, {}]. Error details: [Severity: {}, Code: {}, Message: \"{}\", Hint: \"{}\", Detail: \"{}\", Position: {}]",
                            self.address.host,
                            self.address.database,
                            self.address.username,
                            transaction_status,
                            copy_mode_status,
                            msg.severity,
                            msg.code,
                            msg.message,
                            msg.hint.as_deref().unwrap_or("none"),
                            msg.detail.as_deref().unwrap_or("none"),
                            msg.position.unwrap_or(0)
                        );
                    } else {
                        error!(
                            "PostgreSQL server error from {} (database: {}, user: {}). Could not parse error details.",
                            self.address.host, self.address.database, self.address.username
                        );
                    }

                    // Exit COPY mode if we're in it
                    if self.in_copy_mode {
                        self.in_copy_mode = false;
                    }

                    // Reset prepared statements cache on error since we can't determine the exact cause
                    if self.prepared_statement_cache.is_some() {
                        self.cleanup_state.needs_cleanup_prepare = true;
                    }

                    // Handle async mode errors
                    if self.is_async() {
                        self.data_available = false;
                        self.cleanup_state.needs_cleanup();
                        self.mark_bad("PostgreSQL error in asynchronous operation mode");
                    }
                }

                // CommandComplete
                'C' => {
                    if self.in_copy_mode {
                        self.in_copy_mode = false;
                    }
                    // CommandComplete SET одинаковый для set local и set, чистим.
                    if message.len() == 4 && message.to_vec().eq(COMMAND_COMPLETE_BY_SET) {
                        self.cleanup_state.needs_cleanup_set = true;
                    }
                    if message.len() == 10 && message.to_vec().eq(COMMAND_SAVEPOINT) {
                        self.use_savepoint = true;
                    }
                    if message.len() == 15 && message.to_vec().eq(COMMAND_COMPLETE_BY_DECLARE) {
                        self.cleanup_state.needs_cleanup_declare = true;
                    }
                    if message.len() == 12 && message.to_vec().eq(COMMAND_COMPLETE_BY_DISCARD_ALL) {
                        self.registering_prepared_statement.clear();
                        if self.prepared_statement_cache.is_some() {
                            warn!("Cleanup server {self} prepared statements cache (DISCARD ALL)");
                            self.prepared_statement_cache.as_mut().unwrap().clear();
                        }
                    }
                    if message.len() == 15
                        && message.to_vec().eq(COMMAND_COMPLETE_BY_DEALLOCATE_ALL)
                    {
                        self.registering_prepared_statement.clear();
                        if self.prepared_statement_cache.is_some() {
                            warn!(
                                "Cleanup server {self} prepared statements cache (DEALLOCATE ALL)"
                            );
                            self.prepared_statement_cache.as_mut().unwrap().clear();
                        }
                    }
                    // CommandComplete doesn't have more data after it
                    // The general exit condition will handle breaking the loop
                }

                'S' => {
                    let key = message.read_string().unwrap();
                    let value = message.read_string().unwrap();

                    if let Some(client_server_parameters) = client_server_parameters.as_mut() {
                        client_server_parameters.set_param(key.clone(), value.clone(), false);
                        if self.log_client_parameter_status_changes {
                            info!("Server {self}: client parameter status change: {key} = {value}")
                        }
                    }

                    self.server_parameters.set_param(key, value, false);
                }

                // DataRow
                'D' => {
                    // More data is available after this message, this is not the end of the reply.
                    self.data_available = true;

                    // Don't flush yet, the more we buffer, the faster this goes...up to a limit.
                    if self.buffer.len() >= 8196 {
                        break;
                    }
                }

                // CopyInResponse: copy is starting from client to server.
                'G' => {
                    self.in_copy_mode = true;
                    break;
                }

                // CopyOutResponse: copy is starting from the server to the client.
                'H' => {
                    self.in_copy_mode = true;
                    self.data_available = true;
                    break;
                }

                // CopyData
                'd' => {
                    // Don't flush yet, buffer until we reach limit
                    if self.buffer.len() >= 8196 {
                        break;
                    }
                }

                // CopyDone
                // Buffer until ReadyForQuery shows up, so don't exit the loop yet.
                'c' => (),

                // ParseComplete
                // Response to Parse message in extended query protocol
                '1' => {
                    if self.is_async() {
                        self.data_available = false;
                    }
                }

                // BindComplete
                // Response to Bind message in extended query protocol
                '2' => {
                    if self.is_async() {
                        self.data_available = false;
                    }
                }

                // CloseComplete
                // Response to Close message in extended query protocol
                '3' => {
                    if self.is_async() {
                        self.data_available = false;
                    }
                }

                // ParameterDescription
                // Response to Describe message for a statement
                't' => {
                    if self.is_async() {
                        self.data_available = false;
                    }
                }

                // PortalSuspended
                // Indicates that Execute completed but portal still has rows
                's' => {
                    if self.is_async() {
                        self.data_available = false;
                    }
                }

                // NoData
                // Response to Describe when statement/portal produces no rows
                // https://www.postgresql.org/docs/current/protocol-flow.html
                'n' => {
                    if self.is_async() {
                        self.data_available = false;
                    }
                }

                // Anything else, e.g. errors, notices, etc.
                // Keep buffering until ReadyForQuery shows up.
                _ => (),
            };
        }

        let bytes = self.buffer.clone();

        // Keep track of how much data we got from the server for stats.
        self.stats.data_received(bytes.len());

        // Clear the buffer for next query.
        self.buffer.clear();

        // Clean server rss.
        if self.buffer.len() > 8196 {
            self.buffer = BytesMut::with_capacity(8196);
        }

        // Successfully received data from server
        self.last_activity = SystemTime::now();

        // Pass the data back to the client.
        Ok(bytes)
    }

    /// Indicate that this server connection cannot be re-used and must be discarded.
    pub fn mark_bad(&mut self, reason: &str) {
        error!("Server {self} marked bad, reason: {reason}");
        self.bad = true;
    }

    /// Server & client are out of sync, we must discard this connection.
    /// This happens with clients that misbehave.
    pub fn is_bad(&self) -> bool {
        if self.bad {
            return self.bad;
        };
        false
    }

    pub async fn wait_available(&mut self) {
        if !self.is_data_available() {
            self.stats.wait_idle();
            return;
        }
        warn!("Reading available data from server: {self}");
        loop {
            if !self.is_data_available() {
                self.stats.wait_idle();
                break;
            }
            self.stats.wait_reading();
            match self.recv(&mut tokio::io::sink(), None).await {
                Ok(_) => self.stats.wait_idle(),
                Err(err_read_response) => {
                    error!("Server {self} while reading available data: {err_read_response:?}");
                    break;
                }
            }
        }
    }

    #[inline(always)]
    pub fn is_async(&self) -> bool {
        self.async_mode
    }

    pub async fn send_and_flush_timeout(
        &mut self,
        messages: &BytesMut,
        duration: Duration,
    ) -> Result<(), Error> {
        match timeout(duration, self.send_and_flush(messages)).await {
            Ok(result) => result,
            Err(err) => {
                self.mark_bad("flush timeout error");
                error!("Flush timeout for server {} (database: {}, user: {}). Operation took longer than the configured timeout: {}", 
                      self.address.host, self.address.database, self.address.username, err);
                Err(Error::FlushTimeout)
            }
        }
    }
    pub async fn send_and_flush(&mut self, messages: &BytesMut) -> Result<(), Error> {
        self.stats.data_sent(messages.len());
        self.stats.wait_writing();

        match write_all_flush(&mut self.stream, messages).await {
            Ok(_) => {
                // Successfully sent to server
                self.stats.wait_idle();
                self.last_activity = SystemTime::now();
                Ok(())
            }
            Err(err) => {
                self.stats.wait_idle();
                error!(
                    "Terminating connection to server {} (database: {}, user: {}) due to error: {}",
                    self.address.host, self.address.database, self.address.username, err
                );
                self.mark_bad("flush to server error");
                Err(err)
            }
        }
    }

    /// If the server is still inside a transaction.
    /// If the client disconnects while the server is in a transaction, we will clean it up.
    #[inline(always)]
    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }

    /// If the server is in a transaction and the transaction was aborted.
    /// If the client disconnects while the server is in a transaction, we will clean it up.
    #[inline(always)]
    pub fn in_aborted(&self) -> bool {
        self.in_transaction && self.is_aborted && (!self.use_savepoint)
    }

    #[inline(always)]
    pub fn in_copy_mode(&self) -> bool {
        self.in_copy_mode
    }

    #[inline(always)]
    pub fn address_to_string(&self) -> String {
        self.address.to_string()
    }

    /// Perform any necessary cleanup before putting the server
    /// connection back in the pool
    pub async fn checkin_cleanup(&mut self) -> Result<(), Error> {
        if self.in_copy_mode() {
            warn!("Server {self} returned while still in copy-mode");
            self.mark_bad("returned in copy-mode");
            return Err(Error::ProtocolSyncError(format!(
                "Protocol synchronization error: Server {} (database: {}, user: {}) was returned to the pool while still in COPY mode. This may indicate a client disconnected during a COPY operation.",
                self.address.host, self.address.database, self.address.username
            )));
        }
        if self.is_data_available() {
            warn!("Server {self} returned while still has data available");
            self.mark_bad("returned with data available");
            return Err(Error::ProtocolSyncError(format!(
                "Protocol synchronization error: Server {} (database: {}, user: {}) was returned to the pool while still having data available. This may indicate a client disconnected before receiving all query results.",
                self.address.host, self.address.database, self.address.username
            )));
        }
        if !self.buffer.is_empty() {
            warn!("Server {self} returned while buffer is not empty");
            self.mark_bad("returned with not-empty buffer");
            return Err(Error::ProtocolSyncError(format!(
                "Protocol synchronization error: Server {} (database: {}, user: {}) was returned to the pool with a non-empty buffer. This may indicate a client disconnected before the server response was fully processed.",
                self.address.host, self.address.database, self.address.username
            )));
        }
        // Client disconnected with an open transaction on the server connection.
        // Pgbouncer behavior is to close the server connection but that can cause
        // server connection thrashing if clients repeatedly do this.
        // Instead, we ROLLBACK that transaction before putting the connection back in the pool
        if self.in_transaction() {
            warn!("Server {self} returned while still in transaction, rolling back transaction",);
            self.small_simple_query("ROLLBACK").await?;
        }

        // Client disconnected but it performed session-altering operations such as
        // SET statement_timeout to 1 or create a prepared statement. We clear that
        // to avoid leaking state between clients. For performance reasons we only
        // send `RESET ALL` if we think the session is altered instead of just sending
        // it before each checkin.
        if self.cleanup_state.needs_cleanup() && self.cleanup_connections {
            info!("Server {} returned with session state altered, discarding state ({}) for application {}",
                self, self.cleanup_state, self.application_name);
            let mut reset_string = String::from("RESET ROLE;");

            if self.cleanup_state.needs_cleanup_set {
                reset_string.push_str("RESET ALL;");
            };

            if self.cleanup_state.needs_cleanup_prepare {
                reset_string.push_str("DEALLOCATE ALL;");
            };

            if self.cleanup_state.needs_cleanup_declare {
                reset_string.push_str("CLOSE ALL;");
            };

            self.small_simple_query(&reset_string).await?;
            if self.cleanup_state.needs_cleanup_prepare {
                // flush prepared.
                self.registering_prepared_statement.clear();
                if self.prepared_statement_cache.is_some() {
                    warn!("Cleanup server {self} prepared statements cache");
                    self.prepared_statement_cache.as_mut().unwrap().clear();
                }
            }
            self.cleanup_state.reset();
        }
        self.in_transaction = false;
        self.is_aborted = false;
        self.in_copy_mode = false;
        self.use_savepoint = false;
        Ok(())
    }

    /// We don't buffer all of server responses, e.g. COPY OUT produces too much data.
    /// The client is responsible to call `self.recv()` while this method returns true.
    #[inline(always)]
    pub fn is_data_available(&self) -> bool {
        self.data_available
    }

    /// Switch to async mode, flushing messages as soon
    /// as we receive them without buffering or waiting for "ReadyForQuery".
    #[inline(always)]
    pub fn set_async_mode(&mut self, async_mode: bool) {
        self.async_mode = async_mode
    }

    fn add_prepared_statement_to_cache(&mut self, name: &str) -> Option<String> {
        let cache = match &mut self.prepared_statement_cache {
            Some(cache) => cache,
            None => return None,
        };

        self.stats.prepared_cache_add();

        // If we evict something, we need to close it on the server
        if let Some((evicted_name, _)) = cache.push(name.to_string(), ()) {
            if evicted_name != name {
                return Some(evicted_name);
            }
        };

        None
    }

    fn remove_prepared_statement_from_cache(&mut self, name: &str) {
        let cache = match &mut self.prepared_statement_cache {
            Some(cache) => cache,
            None => return,
        };

        self.stats.prepared_cache_remove();
        cache.pop(name);
    }

    pub async fn register_prepared_statement(
        &mut self,
        parse: &Parse,
        should_send_parse_to_server: bool,
    ) -> Result<(), Error> {
        if !self.has_prepared_statement(&parse.name) {
            self.registering_prepared_statement
                .push_back(parse.name.clone());

            let mut bytes = BytesMut::new();

            if should_send_parse_to_server {
                let parse_bytes: BytesMut = parse.try_into()?;
                bytes.extend_from_slice(&parse_bytes);
            }

            // If we evict something, we need to close it on the server
            // We do this by adding it to the messages we're sending to the server before the sync
            if let Some(evicted_name) = self.add_prepared_statement_to_cache(&parse.name) {
                self.remove_prepared_statement_from_cache(&evicted_name);
                let close_bytes: BytesMut = Close::new(&evicted_name).try_into()?;
                bytes.extend_from_slice(&close_bytes);
            };

            // If we have a parse or close we need to send to the server, send them and sync
            if !bytes.is_empty() {
                bytes.extend_from_slice(&sync());

                self.send_and_flush(&bytes).await?;

                let mut noop = tokio::io::sink();
                loop {
                    self.recv(&mut noop, None).await?;

                    if !self.is_data_available() {
                        break;
                    }
                }
            }
        };

        // If it's not there, something went bad, I'm guessing bad syntax or permissions error
        // on the server.
        if !self.has_prepared_statement(&parse.name) {
            Err(Error::PreparedStatementError)
        } else {
            Ok(())
        }
    }

    /// Claim this server as mine for the purposes of query cancellation.
    pub fn claim(&mut self, process_id: i32, secret_key: i32) {
        let mut guard = self.client_server_map.lock();
        guard.insert(
            (process_id, secret_key),
            (
                self.process_id,
                self.secret_key,
                self.address.host.clone(),
                self.address.port,
            ),
        );
    }

    /// Determines if the server already has a prepared statement with the given name.
    /// Updates the prepared statement cache hit/miss counters.
    #[inline]
    pub fn has_prepared_statement(&mut self, name: &str) -> bool {
        let cache = match &self.prepared_statement_cache {
            Some(cache) => cache,
            None => return false,
        };
        let exists = cache.contains(name);
        if exists {
            self.stats.prepared_cache_hit();
        } else {
            self.stats.prepared_cache_miss();
        }
        exists
    }

    pub async fn sync_parameters(&mut self, parameters: &ServerParameters) -> Result<(), Error> {
        let parameter_diff = self.server_parameters.compare_params(parameters);

        if parameter_diff.is_empty() {
            return Ok(());
        }

        let mut query = String::from("");

        for (key, value) in parameter_diff {
            query.push_str(&format!("SET {key} TO '{value}';"));
        }

        let res = self.small_simple_query(&query).await;

        self.cleanup_state.reset();

        res
    }

    /// Issue a query cancellation request to the server.
    /// Uses a separate connection that's not part of the connection pool.
    pub async fn cancel(
        host: &str,
        port: u16,
        process_id: i32,
        secret_key: i32,
    ) -> Result<(), Error> {
        let mut stream = if host.starts_with('/') {
            create_unix_stream_inner(host, port).await?
        } else {
            create_tcp_stream_inner(host, port, false, false).await?
        };

        warn!("Sending CancelRequest to [{process_id}] {host}:{port}");

        let mut bytes = BytesMut::with_capacity(16);
        bytes.put_i32(16);
        bytes.put_i32(CANCEL_REQUEST_CODE);
        bytes.put_i32(process_id);
        bytes.put_i32(secret_key);

        write_all_flush(&mut stream, &bytes).await
    }

    // Marks a connection as needing cleanup at checkin
    pub fn mark_dirty(&mut self) {
        self.cleanup_state.set_true();
    }

    /// Pretend to be the Postgres client and connect to the server given host, port and credentials.
    /// Perform the authentication and return the server in a ready for query state.
    #[allow(clippy::too_many_arguments)]
    pub async fn startup(
        address: &Address,
        user: &User,
        database: &str,
        client_server_map: ClientServerMap,
        stats: Arc<ServerStats>,
        cleanup_connections: bool,
        log_client_parameter_status_changes: bool,
        prepared_statement_cache_size: usize,
        application_name: String,
    ) -> Result<Server, Error> {
        let config = get_config();

        let mut stream = if address.host.starts_with('/') {
            create_unix_stream_inner(&address.host, address.port).await?
        } else {
            create_tcp_stream_inner(
                &address.host,
                address.port,
                config.general.server_tls,
                config.general.verify_server_certificate,
            )
            .await?
        };

        let username = user
            .clone()
            .server_username
            .unwrap_or(user.clone().username);
        // StartupMessage
        startup(
            &mut stream,
            username.clone(),
            database,
            application_name.clone(),
        )
        .await?;

        let mut process_id: i32 = 0;
        let mut secret_key: i32 = 0;
        let server_identifier = ServerIdentifier::new(username.clone(), database);

        let mut scram_client_auth = if let (Some(_), Some(server_password)) =
            (&user.server_username, &user.server_password)
        {
            Some(ScramSha256::new(server_password))
        } else {
            None
        };
        let mut server_parameters = ServerParameters::new();

        loop {
            let code = match stream.read_u8().await {
                Ok(code) => code as char,
                Err(err) => {
                    return Err(Error::ServerStartupError(
                        format!("Failed to read message code during server startup: {err}"),
                        server_identifier,
                    ));
                }
            };

            let len = match stream.read_i32().await {
                Ok(len) => len,
                Err(err) => {
                    return Err(Error::ServerStartupError(
                        format!("Failed to read message length during server startup: {err}"),
                        server_identifier,
                    ));
                }
            };

            match code {
                // Authentication
                'R' => {
                    // Determine which kind of authentication is required, if any.
                    let auth_code = match stream.read_i32().await {
                        Ok(auth_code) => auth_code,
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "Failed to read authentication code from server".into(),
                                server_identifier,
                            ));
                        }
                    };
                    match auth_code {
                        AUTHENTICATION_SUCCESSFUL => (),
                        /* SASL begin */
                        SASL => {
                            match scram_client_auth {
                                None => {
                                    return Err(Error::ServerAuthError(
                                        "server wants sasl auth, but it is not configured".into(),
                                        server_identifier,
                                    ));
                                }
                                Some(_) => {
                                    let sasl_len = (len - 8) as usize;
                                    let mut sasl_auth = vec![0u8; sasl_len];

                                    match stream.read_exact(&mut sasl_auth).await {
                                        Ok(_) => (),
                                        Err(_) => {
                                            return Err(Error::ServerStartupError(
                                                "Failed to read SASL authentication message from server".into(),
                                                server_identifier,
                                            ))
                                        }
                                    };

                                    let sasl_type =
                                        String::from_utf8_lossy(&sasl_auth[..sasl_len - 2]);

                                    if sasl_type.contains(SCRAM_SHA_256) {
                                        // Generate client message.
                                        let sasl_response =
                                            scram_client_auth.as_mut().unwrap().message();

                                        // SASLInitialResponse (F)
                                        let mut res = BytesMut::new();
                                        res.put_u8(b'p');

                                        // length + String length + length + length of sasl response
                                        res.put_i32(
                                            4 // i32 size
                                        + SCRAM_SHA_256.len() as i32 // length of SASL version string,
                                        + 1 // Null terminator for the SASL version string,
                                        + 4 // i32 size
                                        + sasl_response.len() as i32, // length of SASL response
                                        );

                                        res.put_slice(format!("{SCRAM_SHA_256}\0").as_bytes());
                                        res.put_i32(sasl_response.len() as i32);
                                        res.put(sasl_response);

                                        write_all_flush(&mut stream, &res).await?;
                                    } else {
                                        error!("Unsupported SCRAM version: {sasl_type}");
                                        return Err(Error::ServerAuthError(
                                            format!("Unsupported SCRAM version: {sasl_type}"),
                                            server_identifier,
                                        ));
                                    }
                                }
                            }
                        }
                        SASL_CONTINUE => {
                            let mut sasl_data = vec![0u8; (len - 8) as usize];

                            match stream.read_exact(&mut sasl_data).await {
                                Ok(_) => (),
                                Err(_) => {
                                    return Err(Error::ServerStartupError(
                                        "Failed to read SASL continuation message from server"
                                            .into(),
                                        server_identifier,
                                    ))
                                }
                            };

                            let msg = BytesMut::from(&sasl_data[..]);
                            let sasl_response = scram_client_auth.as_mut().unwrap().update(&msg)?;

                            // SASLResponse
                            let mut res = BytesMut::new();
                            res.put_u8(b'p');
                            res.put_i32(4 + sasl_response.len() as i32);
                            res.put(sasl_response);

                            write_all_flush(&mut stream, &res).await?;
                        }
                        SASL_FINAL => {
                            let mut sasl_final = vec![0u8; len as usize - 8];
                            match stream.read_exact(&mut sasl_final).await {
                                Ok(_) => (),
                                Err(_) => {
                                    return Err(Error::ServerStartupError(
                                        "sasl final message".into(),
                                        server_identifier,
                                    ))
                                }
                            };

                            match scram_client_auth
                                .as_mut()
                                .unwrap()
                                .finish(&BytesMut::from(&sasl_final[..]))
                            {
                                Ok(_) => (),
                                Err(err) => {
                                    return Err(err);
                                }
                            };
                        }
                        /* SASL end */
                        AUTHENTICATION_CLEAR_PASSWORD => {
                            if user.server_username.is_none() || user.server_password.is_none() {
                                error!(
                                    "authentication on server {}@{} with clear auth is not configured",
                                    server_identifier.username, server_identifier.database,
                                );
                                return Err(Error::ServerAuthError(
                                    "server wants clear password authentication, but auth for this server is not configured".into(),
                                    server_identifier,
                                ));
                            }
                            let server_password =
                                <Option<String> as Clone>::clone(&user.server_password)
                                    .unwrap()
                                    .clone();
                            let server_username =
                                <Option<String> as Clone>::clone(&user.server_username)
                                    .unwrap()
                                    .clone();
                            if server_password.starts_with(JWT_PRIV_KEY_PASSWORD_PREFIX) {
                                // generate password
                                let claims = new_claims(server_username, Duration::from_secs(120));
                                let token = match sign_with_jwt_priv_key(
                                    claims,
                                    server_password
                                        .strip_prefix(JWT_PRIV_KEY_PASSWORD_PREFIX)
                                        .unwrap()
                                        .to_string(),
                                )
                                .await
                                {
                                    Ok(token) => token,
                                    Err(err) => {
                                        return Err(Error::ServerAuthError(
                                            err.to_string(),
                                            server_identifier,
                                        ))
                                    }
                                };
                                let mut password_response = BytesMut::new();
                                password_response.put_u8(b'p');
                                password_response.put_i32(token.len() as i32 + 4 + 1);
                                password_response.put_slice(token.as_bytes());
                                password_response.put_u8(b'\0');
                                match stream.try_write(&password_response) {
                                    Ok(_) => (),
                                    Err(err) => {
                                        return Err(Error::ServerAuthError(
                                            format!(
                                                "jwt authentication on the server failed: {err:?}"
                                            ),
                                            server_identifier,
                                        ));
                                    }
                                }
                            } else {
                                return Err(Error::ServerAuthError(
                                    "plain password is not supported".into(),
                                    server_identifier,
                                ));
                            }
                        }
                        MD5_ENCRYPTED_PASSWORD => {
                            if user.server_username.is_none() || user.server_password.is_none() {
                                error!(
                                    "authentication for server {}@{} with md5 auth is not configured",
                                    server_identifier.username, server_identifier.database,
                                );
                                return Err(Error::ServerAuthError(
                                    "server wants md5 authentication, but auth for this server is not configured".into(),
                                    server_identifier,
                                ));
                            } else {
                                let server_username =
                                    <Option<String> as Clone>::clone(&user.server_username)
                                        .unwrap()
                                        .clone();
                                let server_password =
                                    <Option<String> as Clone>::clone(&user.server_password)
                                        .unwrap()
                                        .clone();
                                let mut salt = BytesMut::with_capacity(4);
                                match stream.read_buf(&mut salt).await {
                                    Ok(_) => (),
                                    Err(err) => {
                                        return Err(Error::ServerAuthError(
                                            format!("md5 authentication on the server: {err:?}"),
                                            server_identifier,
                                        ));
                                    }
                                }
                                let password_hash = md5_hash_password(
                                    server_username.as_str(),
                                    server_password.as_str(),
                                    salt.as_mut(),
                                );
                                let mut password_response = BytesMut::new();
                                password_response.put_u8(b'p');
                                password_response.put_i32(password_hash.len() as i32 + 4);
                                password_response.put_slice(&password_hash);
                                match stream.try_write(&password_response) {
                                    Ok(_) => (),
                                    Err(err) => {
                                        return Err(Error::ServerAuthError(
                                            format!(
                                                "md5 authentication on the server failed: {err:?}"
                                            ),
                                            server_identifier,
                                        ));
                                    }
                                }
                            }
                        }
                        _ => {
                            error!("this type of authentication on the server {}@{} is not supported, auth code: {}",
                                server_identifier.username,
                                server_identifier.database,
                                auth_code);
                            return Err(Error::ServerAuthError(
                                "authentication on the server is not supported".into(),
                                server_identifier,
                            ));
                        }
                    }
                }
                // ErrorResponse
                'E' => {
                    let error_code = match stream.read_u8().await {
                        Ok(error_code) => error_code,
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "error code message".into(),
                                server_identifier,
                            ));
                        }
                    };

                    match error_code {
                        // No error message is present in the message.
                        MESSAGE_TERMINATOR => (),

                        // An error message will be present.
                        _ => {
                            if (len as usize) < 2 * mem::size_of::<u32>() {
                                return Err(Error::ServerStartupError(
                                    "while create new connection to postgresql received error, but it's too small".to_string(),
                                    server_identifier,
                                    ));
                            }
                            let mut error = vec![0u8; len as usize - 2 * mem::size_of::<u32>()];
                            match stream.read_exact(&mut error).await {
                                Ok(_) => (),
                                Err(err) => {
                                    return Err(Error::ServerStartupError(
                                        format!("while create new connection to postgresql received error, but can't read it: {err:?}"),
                                        server_identifier,
                                    ));
                                }
                            };

                            return match PgErrorMsg::parse(&error) {
                                Ok(f) => {
                                    error!(
                                        "Get server error - {} {}: {}",
                                        f.severity, f.code, f.message
                                    );
                                    Err(Error::ServerStartupError(f.message, server_identifier))
                                }
                                Err(err) => {
                                    error!("Get unparsed server error: {err:?}");
                                    Err(Error::ServerStartupError(
                                         format!("while create new connection to postgresql received error, but can't read it: {err:?}"),
                                         server_identifier,
                                     ))
                                }
                            };
                        }
                    };

                    return Err(Error::ServerError);
                }

                // Notice
                'N' => {
                    let mut bytes = BytesMut::with_capacity(len as usize - 4);
                    bytes.resize(len as usize - mem::size_of::<i32>(), b'0');
                    match stream.read_exact(&mut bytes[..]).await {
                        Ok(_) => (),
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "read notice message".into(),
                                server_identifier,
                            ));
                        }
                    };
                    if let Ok(msg) = PgErrorMsg::parse(&bytes) {
                        error!(
                            "Server startup messages (severity: {} code: {} message: {})",
                            msg.severity, msg.code, msg.message
                        )
                    };
                }

                // ParameterStatus
                'S' => {
                    let mut bytes = BytesMut::with_capacity(len as usize - 4);
                    bytes.resize(len as usize - mem::size_of::<i32>(), b'0');

                    match stream.read_exact(&mut bytes[..]).await {
                        Ok(_) => (),
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "parameter status message".into(),
                                server_identifier,
                            ));
                        }
                    };

                    let key = bytes.read_string().unwrap();
                    let value = bytes.read_string().unwrap();

                    // Save the parameter so we can pass it to the client later.
                    // These can be server_encoding, client_encoding, server timezone, Postgres version,
                    // and many more interesting things we should know about the Postgres server we are talking to.
                    server_parameters.set_param(key, value, true);
                }

                // BackendKeyData
                'K' => {
                    // The frontend must save these values if it wishes to be able to issue CancelRequest messages later.
                    // See: <https://www.postgresql.org/docs/12/protocol-message-formats.html>.
                    process_id = match stream.read_i32().await {
                        Ok(id) => id,
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "process id message".into(),
                                server_identifier,
                            ));
                        }
                    };

                    secret_key = match stream.read_i32().await {
                        Ok(id) => id,
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "secret key message".into(),
                                server_identifier,
                            ));
                        }
                    };
                }

                // ReadyForQuery
                'Z' => {
                    let mut idle = vec![0u8; len as usize - 4];

                    match stream.read_exact(&mut idle).await {
                        Ok(_) => (),
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "transaction status message".into(),
                                server_identifier,
                            ));
                        }
                    };

                    let server = Server {
                        address: address.clone(),
                        stream: BufStream::new(stream),
                        buffer: BytesMut::with_capacity(8196),
                        server_parameters,
                        process_id,
                        secret_key,
                        in_transaction: false,
                        is_aborted: false,
                        in_copy_mode: false,
                        data_available: false,
                        bad: false,
                        async_mode: false,
                        cleanup_state: CleanupState::new(),
                        client_server_map,
                        connected_at: chrono::offset::Utc::now().naive_utc(),
                        stats,
                        application_name: application_name.clone(),
                        last_activity: SystemTime::now(),
                        cleanup_connections,
                        use_savepoint: false,
                        log_client_parameter_status_changes,
                        prepared_statement_cache: match prepared_statement_cache_size {
                            0 => None,
                            _ => Some(LruCache::new(
                                NonZeroUsize::new(prepared_statement_cache_size).unwrap(),
                            )),
                        },
                        registering_prepared_statement: VecDeque::new(),
                        max_message_size: config.general.message_size_to_be_stream as i32,
                    };
                    server.stats.update_process_id(process_id);

                    return Ok(server);
                }

                // We have an unexpected message from the server during this exchange.
                // Means we implemented the protocol wrong or we're not talking to a Postgres server.
                _ => {
                    error!("Received unexpected message code '{}' (ASCII: {}) during server startup. This may indicate an incompatible PostgreSQL server version or protocol.", code, code as u8);
                    return Err(Error::ProtocolSyncError(format!(
                        "Received unexpected message code '{}' (ASCII: {}) during server startup. This may indicate an incompatible PostgreSQL server version or protocol.",
                        code, code as u8
                    )));
                }
            };
        }
    }
}

impl Drop for Server {
    /// Try to do a clean shut down. Best effort because
    /// the socket is in non-blocking mode, so it may not be ready
    /// for a write.
    fn drop(&mut self) {
        // Update statistics
        self.stats.disconnect();
        {
            let mut guard = CANCELED_PIDS.lock();
            guard.remove(&self.process_id);
        }
        if !self.is_bad() {
            let mut bytes = BytesMut::with_capacity(5);
            bytes.put_u8(b'X');
            bytes.put_i32(4);

            match self.stream.get_mut().try_write(&bytes) {
                Ok(5) => (),
                Err(err) => warn!("Dirty server {self} shutdown: {err}"),
                _ => warn!("Dirty server {self} shutdown"),
            };
        }

        let now = chrono::offset::Utc::now().naive_utc();
        let duration = now - self.connected_at;

        let message = if self.bad {
            "Server connection terminated"
        } else {
            "Server connection closed"
        };

        info!(
            "{} {}, session duration: {}",
            message,
            self,
            crate::format_duration(&duration)
        );
    }
}

async fn create_unix_stream_inner(host: &str, port: u16) -> Result<StreamInner, Error> {
    let stream = match UnixStream::connect(&format!("{host}/.s.PGSQL.{port}")).await {
        Ok(s) => s,
        Err(err) => {
            error!("Could not connect to server: {err}");
            return Err(Error::SocketError(format!(
                "Could not connect to server: {err}"
            )));
        }
    };

    configure_unix_socket(&stream);

    Ok(StreamInner::UnixSocket { stream })
}

async fn create_tcp_stream_inner(
    host: &str,
    port: u16,
    tls: bool,
    _verify_server_certificate: bool,
) -> Result<StreamInner, Error> {
    let mut stream = match TcpStream::connect(&format!("{host}:{port}")).await {
        Ok(stream) => stream,
        Err(err) => {
            error!("Could not connect to server: {err}");
            return Err(Error::SocketError(format!(
                "Could not connect to server: {err}"
            )));
        }
    };

    // TCP timeouts.
    configure_tcp_socket(&stream);

    let stream = if tls {
        // Request a TLS connection
        ssl_request(&mut stream).await?;

        let response = match stream.read_u8().await {
            Ok(response) => response as char,
            Err(err) => {
                return Err(Error::SocketError(format!(
                    "Failed to read TLS response from server: {err}"
                )));
            }
        };

        match response {
            // Server supports TLS
            'S' => {
                error!("Connection to server via tls is not supported");
                return Err(Error::SocketError("Server TLS is unsupported".to_string()));
            }

            // Server does not support TLS
            'N' => StreamInner::TCPPlain { stream },

            // Something else?
            m => {
                return Err(Error::SocketError(format!("Received unexpected response '{}' (ASCII: {}) during TLS negotiation. Expected 'S' (supports TLS) or 'N' (does not support TLS).", m, m as u8)));
            }
        }
    } else {
        StreamInner::TCPPlain { stream }
    };
    Ok(stream)
}
