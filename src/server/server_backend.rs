// Implementation of the PostgreSQL server (database) protocol.

// Standard library imports
use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::string::ToString;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// External crate imports
use bytes::{Buf, BufMut, BytesMut};
use log::{error, info, warn};
use lru::LruCache;
use tokio::io::{AsyncReadExt, BufStream};

// Internal crate imports
use crate::auth::scram_client::ScramSha256;
use crate::config::{get_config, Address, User};
use crate::errors::{Error, ServerIdentifier};
use crate::messages::PgErrorMsg;
use crate::messages::{
    read_message_data, simple_query, startup, sync, BytesMutReader, Close, Parse,
};
use crate::pool::{ClientServerMap, CANCELED_PIDS};
use crate::stats::ServerStats;

use super::authentication::handle_authentication;
use super::cleanup::CleanupState;
use super::parameters::ServerParameters;
use super::startup_error::handle_startup_error;
use super::stream::{create_tcp_stream_inner, create_unix_stream_inner, StreamInner};
use super::{prepared_statements, protocol_io, startup_cancel};

/// Buffer flush threshold in bytes (8 KiB).
/// When the buffer reaches this size, it will be flushed to avoid excessive memory usage.
const BUFFER_FLUSH_THRESHOLD: usize = 8192;

/// Represents a connection to a PostgreSQL server (backend).
///
/// This structure maintains the state of a single connection to a PostgreSQL database server,
/// including connection details, transaction state, buffering, and statistics.
/// The connection can be reused across multiple client sessions through connection pooling.
#[derive(Debug)]
pub struct Server {
    /// Server address configuration including host, port, database, username, and role (primary/replica).
    pub(crate) address: Address,

    /// Buffered TCP or Unix socket stream for communication with the PostgreSQL server.
    pub(crate) stream: BufStream<StreamInner>,

    /// Response buffer for accumulating server messages before forwarding them to the client.
    /// This allows batching multiple messages and reduces the number of write operations.
    pub(crate) buffer: BytesMut,

    /// Server runtime parameters received during startup (e.g., client_encoding, TimeZone, DateStyle).
    /// These parameters are tracked and synchronized with clients to maintain session consistency.
    pub(crate) server_parameters: ServerParameters,

    /// PostgreSQL backend process ID, used for query cancellation requests.
    process_id: i32,

    /// Secret key associated with the backend process, required for query cancellation.
    secret_key: i32,

    /// Transaction state: true if the server is currently inside a transaction block.
    pub(crate) in_transaction: bool,

    /// Indicates whether more data is available from the server to be read.
    /// Set to false when ReadyForQuery message is received.
    pub(crate) data_available: bool,

    /// COPY mode state: true when the server is in COPY IN or COPY OUT mode.
    /// In this mode, data transfer follows a different protocol.
    pub(crate) in_copy_mode: bool,

    /// Async mode state: true when using Flush messages instead of Sync.
    /// In async mode, the server doesn't wait for ReadyForQuery after each command.
    async_mode: bool,

    /// Number of expected responses in async mode.
    /// Decremented when receiving terminating messages (CommandComplete, BindComplete, etc.).
    /// When reaches 0, we know all expected responses have been received.
    expected_responses: u32,

    /// Connection health flag: true if the connection is broken and should be removed from the pool.
    /// Set to true on protocol errors, I/O errors, or unexpected server behavior.
    pub(crate) bad: bool,

    /// Tracks whether the connection needs cleanup (RESET ALL, DEALLOCATE ALL, CLOSE ALL)
    /// before being returned to the pool. Set when SET, PREPARE, or DECLARE statements are executed.
    pub(crate) cleanup_state: CleanupState,

    /// Shared mapping of client-to-server connections for query cancellation support.
    /// Allows canceling queries by mapping client process IDs to server process IDs.
    client_server_map: ClientServerMap,

    /// Timestamp when this connection was established to the server.
    connected_at: chrono::naive::NaiveDateTime,

    /// Statistics collector for this server connection (bytes sent/received, queries executed, etc.).
    pub stats: Arc<ServerStats>,

    /// Application name of the client currently using this server connection.
    /// Updated when the connection is checked out from the pool.
    application_name: String,

    /// Timestamp of the last successful I/O operation (send or receive).
    /// Used to detect idle connections and implement connection timeouts.
    pub last_activity: SystemTime,

    /// Configuration flag: if true, execute cleanup statements (RESET ALL, etc.) on dirty connections
    /// before returning them to the pool. If false, discard dirty connections instead.
    cleanup_connections: bool,

    /// Configuration flag: if true, log when server parameters change for debugging purposes.
    pub(crate) log_client_parameter_status_changes: bool,

    /// LRU cache of prepared statement names currently registered on this server connection.
    /// When the cache is full, evicted statements are automatically closed on the server.
    /// None if prepared statement caching is disabled.
    pub(crate) prepared_statement_cache: Option<LruCache<String, ()>>,

    /// Queue of prepared statement names currently being registered on the server.
    /// Used to track Parse messages that haven't been confirmed yet.
    pub(crate) registering_prepared_statement: VecDeque<String>,

    /// Maximum message size (in bytes) before switching to streaming mode for large DataRow messages.
    /// Messages larger than this threshold are streamed directly to avoid excessive memory usage.
    /// A value of 0 disables streaming.
    pub(crate) max_message_size: i32,
}

impl std::fmt::Display for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "[{}]-{}@{}:{}/{}/{}",
            self.process_id,
            self.address.username,
            self.address.host,
            self.address.port,
            self.address.database,
            self.application_name
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

    /// Check if the connection is alive by sending a minimal query (`;`).
    /// Uses the provided timeout for the operation.
    /// Returns Ok(()) if connection is alive, Err if dead or timeout exceeded.
    pub async fn check_alive(&mut self, timeout: Duration) -> Result<(), Error> {
        let query = simple_query(";");

        self.send_and_flush_timeout(&query, timeout).await?;

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

    /// Returns the PostgreSQL backend process ID for this connection.
    /// Used for query cancellation and connection tracking.
    #[inline(always)]
    pub fn get_process_id(&self) -> i32 {
        self.process_id
    }

    /// Returns a copy of all server parameters as a HashMap.
    /// Includes runtime parameters like client_encoding, TimeZone, DateStyle, etc.
    #[inline(always)]
    pub fn server_parameters_as_hashmap(&self) -> HashMap<String, String> {
        self.server_parameters.as_hashmap()
    }

    /// Receive data from the server in response to a client request.
    /// This method must be called multiple times while `self.is_data_available()` is true
    /// in order to receive all data the server has to offer.
    pub async fn recv<C>(
        &mut self,
        client_stream: C,
        client_server_parameters: Option<&mut ServerParameters>,
    ) -> Result<BytesMut, Error>
    where
        C: tokio::io::AsyncWrite + std::marker::Unpin,
    {
        protocol_io::recv(self, client_stream, client_server_parameters).await
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

    /// Drains any remaining data from the server that hasn't been read yet.
    /// This is used to synchronize the connection state when data is unexpectedly available.
    /// All received data is discarded (sent to a sink).
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

    /// Returns true if the server is in async mode (using Flush instead of Sync).
    /// In async mode, the server doesn't send ReadyForQuery after each command.
    #[inline(always)]
    pub fn is_async(&self) -> bool {
        self.async_mode
    }

    /// Sends messages to the server and flushes the write buffer with a timeout.
    /// Returns an error if the operation doesn't complete within the specified duration.
    pub async fn send_and_flush_timeout(
        &mut self,
        messages: &BytesMut,
        duration: Duration,
    ) -> Result<(), Error> {
        protocol_io::send_and_flush_timeout(self, messages, duration).await
    }

    /// Sends messages to the server and flushes the write buffer immediately.
    /// This ensures all data is transmitted to the server without delay.
    pub async fn send_and_flush(&mut self, messages: &BytesMut) -> Result<(), Error> {
        protocol_io::send_and_flush(self, messages).await
    }

    /// If the server is still inside a transaction.
    /// If the client disconnects while the server is in a transaction, we will clean it up.
    #[inline(always)]
    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }

    /// Returns true if the server is currently in COPY mode (COPY IN or COPY OUT).
    /// In COPY mode, data transfer follows a different protocol than normal queries.
    #[inline(always)]
    pub fn in_copy_mode(&self) -> bool {
        self.in_copy_mode
    }

    /// Returns a string representation of the server address (host:port/database@user).
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
            info!(
                "Server {} returned with session state altered, discarding state ({}) for application {}",
                self, self.cleanup_state, self.application_name
            );
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
        self.in_copy_mode = false;
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

    /// Sets the number of expected responses in async mode.
    /// This is calculated from the batch operations before sending to server.
    #[inline(always)]
    pub fn set_expected_responses(&mut self, count: u32) {
        self.expected_responses = count;
    }

    /// Returns the current number of expected responses.
    #[inline(always)]
    pub fn expected_responses(&self) -> u32 {
        self.expected_responses
    }

    /// Decrements the expected response counter.
    /// Called when receiving terminating messages in async mode.
    #[inline(always)]
    pub fn decrement_expected(&mut self) {
        self.expected_responses = self.expected_responses.saturating_sub(1);
    }

    /// Resets expected responses to 0.
    /// Called on ErrorResponse in async mode since error aborts remaining operations.
    #[inline(always)]
    pub fn reset_expected_responses(&mut self) {
        self.expected_responses = 0;
    }

    fn add_prepared_statement_to_cache(&mut self, name: &str) -> Option<String> {
        prepared_statements::add_to_cache(&mut self.prepared_statement_cache, &self.stats, name)
    }

    fn remove_prepared_statement_from_cache(&mut self, name: &str) {
        prepared_statements::remove_from_cache(
            &mut self.prepared_statement_cache,
            &self.stats,
            name,
        );
    }

    /// Register a prepared statement on the server.
    ///
    /// # Arguments
    /// * `parse` - The Parse message containing query text and parameters
    /// * `server_name` - The name to use on the server (may differ from parse.name for async clients)
    /// * `should_send_parse_to_server` - Whether to actually send Parse to server
    pub async fn register_prepared_statement(
        &mut self,
        parse: &Parse,
        server_name: &str,
        should_send_parse_to_server: bool,
    ) -> Result<(), Error> {
        if !self.has_prepared_statement(server_name) {
            self.registering_prepared_statement
                .push_back(server_name.to_string());

            let mut bytes = BytesMut::new();

            if should_send_parse_to_server {
                // Use server_name instead of parse.name for async clients
                let parse_bytes = parse.to_bytes_with_name(server_name)?;
                bytes.extend_from_slice(&parse_bytes);
            }

            // If we evict something, we need to close it on the server
            // We do this by adding it to the messages we're sending to the server before the sync
            if let Some(evicted_name) = self.add_prepared_statement_to_cache(server_name) {
                self.remove_prepared_statement_from_cache(&evicted_name);
                let close_bytes: BytesMut = Close::new(&evicted_name).try_into()?;
                bytes.extend_from_slice(&close_bytes);
            };

            // If we have a parse or close we need to send to the server, send them and sync
            if !bytes.is_empty() {
                bytes.extend_from_slice(&sync());

                // Temporarily disable async mode so that recv() waits for
                // ReadyForQuery instead of exiting immediately when
                // expected_responses == 0.  Without this, CloseComplete and
                // ReadyForQuery from eviction stay in the TCP buffer and
                // corrupt the next server roundtrip.
                let was_async = self.is_async();
                let saved_expected = self.expected_responses();
                if was_async {
                    self.set_async_mode(false);
                }

                self.send_and_flush(&bytes).await?;

                let mut noop = tokio::io::sink();
                loop {
                    self.recv(&mut noop, None).await?;

                    if !self.is_data_available() {
                        break;
                    }
                }

                // Restore async mode state for the ongoing client pipeline.
                if was_async {
                    self.set_async_mode(true);
                    self.set_expected_responses(saved_expected);
                }
            }
        };

        // If it's not there, something went bad, I'm guessing bad syntax or permissions error
        // on the server.
        if !self.has_prepared_statement(server_name) {
            Err(Error::PreparedStatementError)
        } else {
            Ok(())
        }
    }

    /// Claim this server as mine for the purposes of query cancellation.
    pub fn claim(&mut self, process_id: i32, secret_key: i32) {
        self.client_server_map.insert(
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
        prepared_statements::has(&self.prepared_statement_cache, &self.stats, name)
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
        startup_cancel::cancel(host, port, process_id, secret_key).await
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
            .server_username
            .as_ref()
            .unwrap_or(&user.username)
            .clone();
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
                        server_identifier.clone(),
                    ));
                }
            };

            let len = match stream.read_i32().await {
                Ok(len) => len,
                Err(err) => {
                    return Err(Error::ServerStartupError(
                        format!("Failed to read message length during server startup: {err}"),
                        server_identifier.clone(),
                    ));
                }
            };

            match code {
                // Authentication
                'R' => {
                    let auth_code = stream.read_i32().await.map_err(|_| {
                        Error::ServerStartupError(
                            "Failed to read authentication code from server".into(),
                            server_identifier.clone(),
                        )
                    })?;

                    handle_authentication(
                        &mut stream,
                        auth_code,
                        len,
                        user,
                        &mut scram_client_auth,
                        &server_identifier,
                    )
                    .await?;
                }

                // ErrorResponse
                'E' => {
                    return handle_startup_error(&mut stream, len, &server_identifier)
                        .await
                        .map(|_| unreachable!());
                }

                // Notice
                'N' => {
                    let mut msg = read_message_data(&mut stream, code as u8, len).await?;
                    let _ = msg.get_u8();
                    let _ = msg.get_i32();
                    if let Ok(msg) = PgErrorMsg::parse(&msg) {
                        error!(
                            "Server startup messages (severity: {} code: {} message: {})",
                            msg.severity, msg.code, msg.message
                        )
                    };
                }

                // ParameterStatus
                'S' => {
                    let mut bytes = read_message_data(&mut stream, code as u8, len).await?;
                    let _ = bytes.get_u8();
                    let _ = bytes.get_i32();
                    let key = bytes.read_string().unwrap();
                    let value = bytes.read_string().unwrap();

                    // Save the parameter so we can pass it to the client later.
                    server_parameters.set_param(key, value, true);
                }

                // BackendKeyData
                'K' => {
                    // The frontend must save these values if it wishes to be able to issue CancelRequest messages later.
                    process_id = stream.read_i32().await.map_err(|_| {
                        Error::ServerStartupError(
                            "process id message".into(),
                            server_identifier.clone(),
                        )
                    })?;

                    secret_key = stream.read_i32().await.map_err(|_| {
                        Error::ServerStartupError(
                            "secret key message".into(),
                            server_identifier.clone(),
                        )
                    })?;
                }

                // ReadyForQuery
                'Z' => {
                    let _idle = read_message_data(&mut stream, code as u8, len).await?;

                    let server = Server {
                        address: address.to_owned(),
                        stream: BufStream::new(stream),
                        buffer: BytesMut::with_capacity(BUFFER_FLUSH_THRESHOLD),
                        server_parameters,
                        process_id,
                        secret_key,
                        in_transaction: false,
                        in_copy_mode: false,
                        data_available: false,
                        bad: false,
                        async_mode: false,
                        expected_responses: 0,
                        cleanup_state: CleanupState::new(),
                        client_server_map,
                        connected_at: chrono::offset::Utc::now().naive_utc(),
                        stats,
                        application_name,
                        last_activity: SystemTime::now(),
                        cleanup_connections,
                        log_client_parameter_status_changes,
                        prepared_statement_cache: match prepared_statement_cache_size {
                            0 => None,
                            _ => Some(LruCache::new(
                                NonZeroUsize::new(prepared_statement_cache_size).unwrap(),
                            )),
                        },
                        registering_prepared_statement: VecDeque::new(),
                        max_message_size: config.general.message_size_to_be_stream.as_bytes()
                            as i32,
                    };
                    server.stats.update_process_id(process_id);

                    return Ok(server);
                }

                // We have an unexpected message from the server during this exchange.
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
            crate::utils::format_duration(&duration)
        );
    }
}
