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
use crate::config::{get_config, tls, Address, BackendAuthMethod, User};
use crate::errors::{Error, ServerIdentifier};
use crate::messages::PgErrorMsg;
use crate::messages::{
    read_message_data, simple_query, startup, sync, BytesMutReader, Close, Parse,
};
use crate::pool::{CancelTarget, ClientServerMap, CANCELED_PIDS};
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
    pub(crate) buffer: BytesMut,

    /// Reusable read buffer for message parsing. Avoids heap allocation per message —
    /// clear()+reserve() reuses existing capacity. Cleared on checkin for defence-in-depth.
    pub(crate) read_buf: BytesMut,

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

    /// True when prepared statements were added to the LRU cache via
    /// register_prepared_statement(should_send_parse_to_server=false) but
    /// the client buffer has not yet been flushed to PostgreSQL (Sync/Flush
    /// not received). If the client disconnects before flushing, checkin_cleanup
    /// uses this flag to trigger DEALLOCATE ALL and clear the stale cache.
    pub(crate) has_pending_cache_entries: bool,

    /// Statements evicted from the server LRU during the current batch but
    /// whose Close has NOT yet been sent to PostgreSQL. The statements still
    /// exist on PostgreSQL — Close is deferred until Sync/Flush completes so
    /// that any Bind referencing them in the client buffer succeeds first.
    pub(crate) deferred_eviction_closes: Vec<String>,

    /// Whether this connection uses TLS. Determines if cancel requests use TLS.
    connected_with_tls: bool,

    /// Session mode flag: true when the pool operates in session mode.
    /// In session mode, PostgreSQL ErrorResponse in async mode does not mark connection as bad,
    /// because the connection remains valid and the client can continue using it.
    pub(crate) session_mode: bool,

    /// Maximum message size (in bytes) before switching to streaming mode for large DataRow messages.
    /// Messages larger than this threshold are streamed directly to avoid excessive memory usage.
    /// A value of 0 disables streaming.
    pub(crate) max_message_size: i32,

    /// Large message header saved when recv() needs to return accumulated buffer first.
    /// The large DataRow/CopyData will be streamed on the next recv() call.
    pub(crate) pending_large_message: Option<(u8, i32)>,

    /// Reason for closing this connection, set before dropping.
    /// Used by Drop to produce a single log line with cause and effect.
    pub(crate) close_reason: Option<String>,
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
        error!(
            "[{}@{}] server marked bad pid={}: {reason}",
            self.address.username, self.address.pool_name, self.process_id
        );
        self.bad = true;
    }

    /// Returns a future that completes when the server socket becomes readable.
    /// Between queries in a transaction, BufStream is empty (everything was read
    /// up to ReadyForQuery), so readable on the underlying socket correctly
    /// reflects new data from the server (e.g., FATAL after idle_in_transaction_session_timeout).
    pub async fn server_readable(&self) {
        let _ = self.stream.get_ref().readable().await;
    }

    /// Verify that server_readable() readiness is genuine, not spurious.
    /// Returns true if the connection is alive (WouldBlock = no real data).
    /// Returns false if the server sent data or closed the connection (dead).
    pub fn check_server_alive(&self) -> bool {
        if self.stream.get_ref().is_tls() {
            // For TLS connections, readable() fires on raw TCP socket readiness.
            // Calling try_read() on the raw socket would consume bytes that the
            // TLS layer hasn't processed, corrupting the session.
            //
            // On an idle PostgreSQL connection, the raw socket should never become
            // readable (PostgreSQL does not send unsolicited data, and TLS
            // renegotiation is disabled since PG14). If readable() fired, the
            // server disconnected or sent an error — treat as dead.
            return false;
        }
        let mut buf = [0u8; 1];
        matches!(
            self.stream.get_ref().try_read(&mut buf),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
        )
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
        warn!(
            "[{}@{}] draining unread data from server pid={}",
            self.address.username, self.address.pool_name, self.process_id
        );
        loop {
            if !self.is_data_available() {
                self.stats.wait_idle();
                break;
            }
            self.stats.wait_reading();
            match self.recv(&mut tokio::io::sink(), None).await {
                Ok(_) => self.stats.wait_idle(),
                Err(err_read_response) => {
                    error!(
                        "[{}@{}] server read error pid={}: {err_read_response}",
                        self.address.username, self.address.pool_name, self.process_id
                    );
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
        self.pending_large_message = None;
        if self.in_copy_mode() {
            warn!(
                "[{}@{}] server returned in copy-mode pid={}",
                self.address.username, self.address.pool_name, self.process_id
            );
            self.mark_bad("returned in copy-mode");
            return Err(Error::ProtocolSyncError(format!(
                "Protocol synchronization error: Server {} (database: {}, user: {}) was returned to the pool while still in COPY mode. This may indicate a client disconnected during a COPY operation.",
                self.address.host, self.address.database, self.address.username
            )));
        }
        if self.is_data_available() {
            warn!(
                "[{}@{}] server returned with data available pid={}",
                self.address.username, self.address.pool_name, self.process_id
            );
            self.mark_bad("returned with data available");
            return Err(Error::ProtocolSyncError(format!(
                "Protocol synchronization error: Server {} (database: {}, user: {}) was returned to the pool while still having data available. This may indicate a client disconnected before receiving all query results.",
                self.address.host, self.address.database, self.address.username
            )));
        }
        if !self.buffer.is_empty() {
            warn!(
                "[{}@{}] server returned with non-empty buffer pid={}",
                self.address.username, self.address.pool_name, self.process_id
            );
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
            warn!(
                "[{}@{}] server returned in transaction, rolling back pid={}",
                self.address.username, self.address.pool_name, self.process_id
            );
            self.small_simple_query("ROLLBACK").await?;
        }

        // If the client added prepared statements to the cache but disconnected
        // before Sync/Flush, the cache contains entries that were never sent to
        // PostgreSQL. Force DEALLOCATE ALL to re-synchronize.
        if self.has_pending_cache_entries {
            self.cleanup_state.needs_cleanup_prepare = true;
            self.has_pending_cache_entries = false;
        }

        // If eviction Closes were deferred but never sent (client disconnected
        // before Sync), the LRU and PostgreSQL are out of sync. DEALLOCATE ALL
        // cleans up both the deferred entries and any other stale state.
        if !self.deferred_eviction_closes.is_empty() {
            self.cleanup_state.needs_cleanup_prepare = true;
            self.deferred_eviction_closes.clear();
        }

        // Client disconnected but it performed session-altering operations such as
        // SET statement_timeout to 1 or create a prepared statement. We clear that
        // to avoid leaking state between clients. For performance reasons we only
        // send `RESET ALL` if we think the session is altered instead of just sending
        // it before each checkin.
        if self.cleanup_state.needs_cleanup() && self.cleanup_connections {
            info!(
                "[{}@{}] session state cleanup pid={}: {}",
                self.address.username, self.address.pool_name, self.process_id, self.cleanup_state
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
                    let cache_size = self.prepared_statement_cache.as_ref().unwrap().len();
                    info!(
                        "[{}@{}] clearing prepared statement cache pid={}: session state reset ({} entries)",
                        self.address.username, self.address.pool_name, self.process_id, cache_size
                    );
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

            // Track that we added to cache without sending Parse to PostgreSQL.
            // The actual Parse is deferred in the client buffer until Sync/Flush.
            // If the client disconnects before flushing, checkin_cleanup will
            // detect this flag and trigger DEALLOCATE ALL to fix the desync.
            if !should_send_parse_to_server {
                self.has_pending_cache_entries = true;
            }

            // If we evict something, defer the Close until after the current batch
            // completes (Sync/Flush). The evicted statement still exists on PostgreSQL,
            // so any Bind referencing it in the client buffer will succeed.
            // send_deferred_eviction_closes() sends the Close after Sync.
            if let Some(evicted_name) = self.add_prepared_statement_to_cache(server_name) {
                self.remove_prepared_statement_from_cache(&evicted_name);
                self.deferred_eviction_closes.push(evicted_name);
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
            CancelTarget {
                process_id: self.process_id,
                secret_key: self.secret_key,
                host: self.address.host.clone(),
                port: self.address.port,
                server_tls: self.address.server_tls.clone(),
                connected_with_tls: self.connected_with_tls,
                pool_name: self.address.pool_name.clone(),
            },
        );
    }

    /// Determines if the server already has a prepared statement with the given name.
    /// Checks both the LRU cache and the deferred eviction list (statements evicted
    /// from LRU but not yet Closed on PostgreSQL — they still exist there).
    #[inline]
    pub fn has_prepared_statement(&mut self, name: &str) -> bool {
        if self.deferred_eviction_closes.iter().any(|n| n == name) {
            self.stats.prepared_cache_hit();
            return true;
        }
        prepared_statements::has(&mut self.prepared_statement_cache, &self.stats, name)
    }

    /// Send Close+Sync for all deferred eviction entries and consume responses.
    /// Called after the client batch is flushed (Sync/Flush) so that Binds
    /// referencing evicted statements have already been processed by PostgreSQL.
    pub async fn send_deferred_eviction_closes(&mut self) -> Result<(), Error> {
        if self.deferred_eviction_closes.is_empty() {
            return Ok(());
        }

        let mut bytes = BytesMut::new();
        for name in self.deferred_eviction_closes.drain(..) {
            let close_bytes: BytesMut = Close::new(&name).try_into()?;
            bytes.extend_from_slice(&close_bytes);
        }
        bytes.extend_from_slice(&sync());

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

        if was_async {
            self.set_async_mode(true);
            self.set_expected_responses(saved_expected);
        }

        Ok(())
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
        server_tls: &tls::ServerTlsConfig,
        connected_with_tls: bool,
        pool_name: &str,
    ) -> Result<(), Error> {
        startup_cancel::cancel(
            host,
            port,
            process_id,
            secret_key,
            server_tls,
            connected_with_tls,
            pool_name,
        )
        .await
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
        session_mode: bool,
    ) -> Result<Server, Error> {
        let config = get_config();

        log::debug!(
            "[{}@{}] Server::startup connecting to {}:{} (server_tls_mode={})",
            user.username,
            database,
            address.host,
            address.port,
            address.server_tls.mode
        );

        let mut stream = if address.host.starts_with('/') {
            create_unix_stream_inner(&address.host, address.port).await?
        } else {
            create_tcp_stream_inner(
                &address.host,
                address.port,
                &address.server_tls,
                &address.pool_name,
            )
            .await?
        };

        let connected_with_tls = matches!(&stream, StreamInner::TCPTls { .. });
        log::debug!(
            "[{}@{}] Server connection to {}:{} established (tls={})",
            user.username,
            database,
            address.host,
            address.port,
            connected_with_tls
        );

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
        let server_identifier =
            ServerIdentifier::new(username.clone(), database, &address.pool_name);

        let backend_auth_snapshot = address.backend_auth.as_ref().map(|ba| ba.read().clone());

        let mut scram_client_auth = match &backend_auth_snapshot {
            Some(BackendAuthMethod::ScramPassthrough(client_key)) => {
                Some(ScramSha256::from_client_key(client_key.clone()))
            }
            Some(BackendAuthMethod::ScramPending) => {
                // SCRAM passthrough configured but ClientKey not yet available.
                // Fall through to server_password if available; otherwise None
                // (backend SASL auth will fail with a clear error).
                warn!(
                    "[{}@{}] backend connection attempted before first client SCRAM auth (ScramPending), \
                     falling back to server_password",
                    address.username, address.pool_name
                );
                if let (Some(_), Some(server_password)) =
                    (&user.server_username, &user.server_password)
                {
                    Some(ScramSha256::new(server_password))
                } else {
                    None
                }
            }
            _ => {
                // Existing logic: create from server_password
                if let (Some(_), Some(server_password)) =
                    (&user.server_username, &user.server_password)
                {
                    Some(ScramSha256::new(server_password))
                } else {
                    None
                }
            }
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
                        backend_auth_snapshot.as_ref(),
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
                        warn!(
                            "[{}@{}] startup notice: severity={}, code={}, message={}",
                            address.username,
                            address.pool_name,
                            msg.severity,
                            msg.code,
                            msg.message
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
                            "failed to read process ID during startup".into(),
                            server_identifier.clone(),
                        )
                    })?;

                    secret_key = stream.read_i32().await.map_err(|_| {
                        Error::ServerStartupError(
                            "failed to read secret key during startup".into(),
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
                        read_buf: BytesMut::with_capacity(BUFFER_FLUSH_THRESHOLD),
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
                        has_pending_cache_entries: false,
                        deferred_eviction_closes: Vec::new(),
                        connected_with_tls,
                        session_mode,
                        max_message_size: config.general.message_size_to_be_stream.as_bytes()
                            as i32,
                        pending_large_message: None,
                        close_reason: None,
                    };
                    server.stats.update_process_id(process_id);
                    server.stats.set_tls(connected_with_tls);

                    return Ok(server);
                }

                // We have an unexpected message from the server during this exchange.
                _ => {
                    error!("[{}@{}] unexpected message code '{}' (ASCII: {}) during server startup to {}:{}", server_identifier.username, server_identifier.pool_name, code, code as u8, address.host, address.port);
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
                Err(err) => warn!(
                    "[{}@{}] failed to send Terminate to server pid={}: {err}",
                    self.address.username, self.address.pool_name, self.process_id
                ),
                _ => warn!(
                    "[{}@{}] incomplete Terminate sent to server pid={}",
                    self.address.username, self.address.pool_name, self.process_id
                ),
            };
        }

        let now = chrono::offset::Utc::now().naive_utc();
        let duration = now - self.connected_at;
        let session = crate::utils::format_duration(&duration);

        match (&self.close_reason, self.bad) {
            (Some(reason), _) => info!(
                "[{}@{}] server closed pid={}: {}, session={}",
                self.address.username, self.address.pool_name, self.process_id, reason, session,
            ),
            (None, true) => info!(
                "[{}@{}] server terminated pid={}, session={}",
                self.address.username, self.address.pool_name, self.process_id, session,
            ),
            (None, false) => info!(
                "[{}@{}] server closed pid={}, session={}",
                self.address.username, self.address.pool_name, self.process_id, session,
            ),
        }
    }
}
