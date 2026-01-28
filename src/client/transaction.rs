use bytes::{BufMut, BytesMut};
use log::{debug, error, warn};
use std::ops::DerefMut;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::utils::clock::now;

use crate::admin::handle_admin;
use crate::app::server::{CLIENTS_IN_TRANSACTIONS, SHUTDOWN_IN_PROGRESS};
use crate::client::batch_handling::PARSE_COMPLETE_MSG;
use crate::client::core::{BatchOperation, Client, PreparedStatementKey};
use crate::client::util::{is_standalone_begin, QUERY_DEALLOCATE};
use crate::errors::Error;
use crate::messages::{
    check_query_response, command_complete, deallocate_response, error_message, error_response,
    error_response_terminal, insert_close_complete_after_last_close_complete, read_message,
    ready_for_query, write_all_flush,
};
use crate::pool::CANCELED_PIDS;
use crate::server::Server;
use crate::utils::comments::SqlCommentParser;
use crate::utils::debug_messages::{log_client_to_server, log_server_to_client};

// =============================================================================
// PostgreSQL Extended Query Protocol - Documentation
// =============================================================================
//
// This module handles the PostgreSQL Extended Query Protocol, which allows
// clients to send multiple messages in a batch before requesting results.
//
// ## Protocol Message Types (Client → Server)
//
// | Code | Message   | Description                                      |
// |------|-----------|--------------------------------------------------|
// | 'P'  | Parse     | Prepare a statement (with optional name)         |
// | 'B'  | Bind      | Bind parameters to a prepared statement          |
// | 'E'  | Execute   | Execute a bound portal                           |
// | 'D'  | Describe  | Request description of statement or portal       |
// | 'C'  | Close     | Close a prepared statement or portal             |
// | 'S'  | Sync      | Synchronization point, requests results          |
// | 'H'  | Flush     | Request server to flush output (async mode)      |
// | 'Q'  | Query     | Simple query (not extended protocol)             |
// | 'X'  | Terminate | Close connection                                 |
//
// ## Protocol Message Types (Server → Client)
//
// | Code | Message              | Description                              |
// |------|----------------------|------------------------------------------|
// | '1'  | ParseComplete        | Statement was parsed successfully        |
// | '2'  | BindComplete         | Parameters were bound successfully       |
// | 'T'  | RowDescription       | Description of result columns            |
// | 'D'  | DataRow              | A row of query results                   |
// | 'C'  | CommandComplete      | Command finished (with row count)        |
// | 't'  | ParameterDescription | Description of statement parameters      |
// | 'n'  | NoData               | Statement returns no data                |
// | '3'  | CloseComplete        | Statement/portal was closed              |
// | 'Z'  | ReadyForQuery        | Server ready for next query              |
// | 'E'  | ErrorResponse        | An error occurred                        |
//
// ## Basic Extended Query Flow
//
// ```text
// Client                      Proxy                      Server
//   │                           │                           │
//   │──── Parse (P) ───────────>│                           │
//   │──── Bind (B) ────────────>│                           │
//   │──── Execute (E) ─────────>│                           │
//   │──── Sync (S) ────────────>│──── P,B,E,S ────────────>│
//   │                           │                           │
//   │                           │<─── ParseComplete (1) ────│
//   │                           │<─── BindComplete (2) ─────│
//   │                           │<─── DataRow... (D) ───────│
//   │                           │<─── CommandComplete (C) ──│
//   │<──── Response ────────────│<─── ReadyForQuery (Z) ────│
// ```
//
// ## Prepared Statement Caching
//
// pg_doorman caches prepared statements to avoid re-parsing identical queries.
// When a Parse message arrives:
//
// 1. If statement is NOT in cache → send Parse to server, cache it
// 2. If statement IS in cache AND server has it → skip Parse, inject ParseComplete
// 3. If statement IS in cache BUT server doesn't have it → send Parse to server
//
// ## Batch Processing with Cached Statements
//
// When some Parse messages are skipped (cached), we must inject ParseComplete
// responses in the correct order. Example:
//
// ```text
// Client sends:              Server receives:         Server responds:
// ┌─────────────────┐        ┌─────────────────┐      ┌─────────────────┐
// │ Parse "stmt1"   │──┐     │                 │      │                 │
// │ (cached,skip)   │  │     │                 │      │                 │
// ├─────────────────┤  │     ├─────────────────┤      ├─────────────────┤
// │ Parse "stmt2"   │──┼────>│ Parse "stmt2"   │─────>│ ParseComplete   │
// │ (new, send)     │  │     │                 │      │                 │
// ├─────────────────┤  │     ├─────────────────┤      ├─────────────────┤
// │ Bind to "stmt1" │──┼────>│ Bind to "stmt1" │─────>│ BindComplete    │
// ├─────────────────┤  │     ├─────────────────┤      ├─────────────────┤
// │ Sync            │──┘────>│ Sync            │─────>│ ReadyForQuery   │
// └─────────────────┘        └─────────────────┘      └─────────────────┘
//
// Proxy must reorder response to client:
// ┌─────────────────┐
// │ ParseComplete   │ ← injected for skipped "stmt1"
// │ ParseComplete   │ ← from server for "stmt2"
// │ BindComplete    │ ← from server
// │ ReadyForQuery   │ ← from server
// └─────────────────┘
// ```
//
// The `reorder_parse_complete_responses()` function handles this reordering
// by tracking batch operations and inserting synthetic ParseComplete messages
// at the correct positions in the response stream.
//
// ## Async Mode (Flush command)
//
// When client uses 'H' (Flush) instead of 'S' (Sync), it enters async mode.
// In async mode, prepared statement caching is disabled to avoid
// "prepared statement already exists" errors, because the client may
// send multiple Parse messages for the same statement before receiving
// responses.
//
// =============================================================================

/// Buffer flush threshold in bytes (8 KiB).
/// When the buffer reaches this size, it will be flushed to avoid excessive memory usage.
const BUFFER_FLUSH_THRESHOLD: usize = 8192;

/// Action to take after processing a message in the transaction loop
enum TransactionAction {
    /// Continue processing messages in the transaction loop
    Continue,
    /// Break out of the transaction loop (release server)
    Break,
    /// Break and wait for ROLLBACK from client (aborted transaction)
    BreakWaitRollback,
}

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    /// Send error response for aborted transaction state
    async fn send_aborted_transaction_error(&mut self) -> Result<(), Error> {
        let mut buf = error_message(
            "current transaction is aborted, commands ignored until end of transaction block",
            "25P02",
        );
        // ReadyForQuery with state 'E'
        let mut z = BytesMut::new();
        z.put_u8(b'Z');
        z.put_i32(5);
        z.put_u8(b'E');
        buf.put(z);
        self.stats.idle_write();
        write_all_flush(&mut self.write, &buf).await
    }

    /// Complete transaction statistics and check if server should be released
    /// Returns true if the transaction loop should break (server should be released)
    #[inline(always)]
    fn complete_transaction_if_needed(&mut self, server: &Server, check_async: bool) -> bool {
        if !server.in_transaction() {
            self.stats.transaction();
            server
                .stats
                .transaction(self.server_parameters.get_application_name());

            // Release server back to the pool if we are in transaction mode.
            // If we are in session mode, we keep the server until the client disconnects.
            if self.transaction_mode && !server.in_copy_mode() {
                // Don't release if in async mode (when check_async is true)
                if !check_async || !server.is_async() {
                    return true;
                }
            }
        }
        false
    }

    /// Ensure server is in copy mode, return error if not
    #[inline(always)]
    fn ensure_copy_mode(&mut self, server: &mut Server) -> Result<(), Error> {
        if !server.in_copy_mode() {
            self.stats.disconnect();
            server.mark_bad("client expects COPY mode, but server are not in");
            return Err(Error::ProtocolSyncError(
                "server not in copy mode".to_string(),
            ));
        }
        Ok(())
    }

    /// Wait for a ROLLBACK from client after server entered aborted transaction state.
    /// For any incoming message except a Simple Query with ROLLBACK, respond with:
    /// - ErrorResponse (SQLSTATE 25P02) and ReadyForQuery with status 'E' (failed tx)
    ///   For a Simple Query 'ROLLBACK', respond with:
    ///   - CommandComplete("ROLLBACK") and ReadyForQuery with status 'I' (idle)
    pub async fn wait_rollback(&mut self) -> Result<(), Error> {
        loop {
            // Read next client message
            let message = match read_message(&mut self.read, self.max_memory_usage).await {
                Ok(message) => message,
                Err(err) => return self.process_error(err).await,
            };

            let code = message[0] as char;
            match code {
                // Terminate
                'X' => {
                    debug!("Client {} sent Terminate [X]", self.addr);
                    self.stats.disconnect();
                    return Ok(());
                }
                // Simple Query
                'Q' => {
                    // Parse query string (null-terminated) - work with &str to avoid allocation
                    let sql = if message.len() >= 6 {
                        let payload = &message[5..];
                        // strip trailing NUL if present
                        let end = payload
                            .iter()
                            .position(|b| *b == 0)
                            .unwrap_or(payload.len());
                        std::str::from_utf8(&payload[..end]).unwrap_or("")
                    } else {
                        ""
                    };
                    let sql_without_comments = SqlCommentParser::new(sql).remove_comment_sql();
                    let command = sql_without_comments.trim().trim_end_matches(';').trim();
                    if command.eq_ignore_ascii_case("rollback")
                        || command.eq_ignore_ascii_case("commit")
                    {
                        // Send CommandComplete + ReadyForQuery(Idle)
                        // Pre-allocate buffer: command_complete ~20 bytes + ready_for_query 6 bytes
                        let mut res = BytesMut::with_capacity(32);
                        res.put(command_complete("ROLLBACK"));
                        res.put(ready_for_query(false)); // Idle
                        self.stats.idle_write();
                        write_all_flush(&mut self.write, &res).await?;
                        return Ok(());
                    } else {
                        self.send_aborted_transaction_error().await?;
                        // Continue waiting for rollback
                    }
                }
                // For any other kind of message, reply with the same error and continue
                _ => {
                    self.send_aborted_transaction_error().await?;
                }
            }
        }
    }

    /// Handle cancel mode - when client wants to cancel a previously issued query.
    /// Opens a new separate connection to the server, sends the backend_id
    /// and secret_key and then closes it for security reasons.
    async fn handle_cancel_mode(&self) -> Result<(), Error> {
        let (process_id, secret_key, address, port) = {
            match self
                .client_server_map
                .get(&(self.process_id, self.secret_key))
            {
                // We found the server the client is using for its query
                // that it wants to cancel.
                Some(entry) => {
                    let (process_id, secret_key, address, port) = entry.value();
                    {
                        let mut cancel_guard = CANCELED_PIDS.lock();
                        cancel_guard.insert(*process_id);
                    }
                    (*process_id, *secret_key, address.clone(), *port)
                }

                // The client doesn't know / got the wrong server,
                // we're closing the connection for security reasons.
                None => return Ok(()),
            }
        };

        Server::cancel(&address, port, process_id, secret_key).await
    }

    /// Check for pooler health check and DEALLOCATE queries, handle them without server.
    /// Returns `Ok(true)` if query was handled (caller should continue to next iteration),
    /// `Ok(false)` if query needs normal processing.
    #[inline]
    async fn try_handle_without_server(&mut self, message: &BytesMut) -> Result<bool, Error> {
        if message[0] != b'Q' {
            return Ok(false);
        }

        // Check for pooler health check query
        if message.len() == self.pooler_check_query_request_vec.len()
            && self.pooler_check_query_request_vec.as_slice() == &message[..]
        {
            write_all_flush(&mut self.write, &check_query_response()).await?;
            return Ok(true);
        }

        // Check for DEALLOCATE query and clear client prepared statements cache
        // Format: Q message = [Q:1][length:4][query][null:1]
        // QUERY_DEALLOCATE = "deallocate " (11 bytes)
        if message.len() < 60 && message.len() > QUERY_DEALLOCATE.len() + 6 {
            let query_bytes = &message[5..message.len() - 1]; // exclude null terminator

            // Case-insensitive check for "deallocate " prefix
            if query_bytes
                .get(..QUERY_DEALLOCATE.len())
                .map(|s| s.eq_ignore_ascii_case(QUERY_DEALLOCATE))
                .unwrap_or(false)
            {
                // Extract statement name after "deallocate "
                let statement_part = std::str::from_utf8(&query_bytes[QUERY_DEALLOCATE.len()..])
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(';');

                if statement_part.eq_ignore_ascii_case("all") {
                    // DEALLOCATE ALL - clear entire client cache
                    let count = self.prepared.cache.len();
                    self.prepared.cache.clear();
                    debug!(
                        "DEALLOCATE ALL: cleared {} entries from client prepared statements cache",
                        count
                    );
                } else if !statement_part.is_empty() {
                    // DEALLOCATE <name> - remove specific statement from cache
                    let key = PreparedStatementKey::Named(statement_part.to_string());
                    if self.prepared.cache.remove(&key).is_some() {
                        debug!(
                            "DEALLOCATE {}: removed from client prepared statements cache",
                            statement_part
                        );
                    }
                }

                write_all_flush(&mut self.write, &deallocate_response()).await?;
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Handle simple query (Q message).
    /// Returns the action to take after processing.
    #[inline]
    async fn handle_simple_query(
        &mut self,
        message: &BytesMut,
        server: &mut Server,
        query_start_at: quanta::Instant,
    ) -> Result<TransactionAction, Error> {
        self.execute_server_roundtrip(Some(message), server).await?;
        self.stats.query();
        server.stats.query(
            query_start_at.elapsed().as_micros() as u64,
            self.server_parameters.get_application_name(),
        );

        if server.in_aborted() {
            return Ok(TransactionAction::BreakWaitRollback);
        }

        if self.complete_transaction_if_needed(server, false) {
            self.stats.idle_read();
            return Ok(TransactionAction::Break);
        }

        Ok(TransactionAction::Continue)
    }

    /// Handle Sync (S) or Flush (H) message.
    /// Returns the action to take after processing.
    #[inline]
    async fn handle_sync_flush(
        &mut self,
        message: &BytesMut,
        server: &mut Server,
        query_start_at: quanta::Instant,
        code: char,
    ) -> Result<TransactionAction, Error> {
        // Add the sync/flush message to buffer
        self.buffer.put(&message[..]);

        if code == 'H' {
            // For Flush, enter async mode
            server.set_async_mode(true);
            // Mark this client as async client forever
            self.prepared.async_client = true;
            debug!("Client requested flush, going async");

            // If there are skipped Parse operations, send synthetic ParseComplete to client
            // BEFORE waiting for server response. This is necessary because:
            // 1. Parse was skipped (statement already cached), so server didn't receive it
            // 2. Flush was sent to server, but server has nothing to flush
            // 3. Server won't respond, causing a hang
            // By sending synthetic ParseComplete here, we satisfy client's expectation
            if !self.prepared.skipped_parses.is_empty() {
                let count = self.prepared.skipped_parses.len();
                debug!(
                    "Flush: sending {} synthetic ParseComplete for skipped Parse operations",
                    count
                );
                let mut synthetic_response = BytesMut::with_capacity(count * 5);
                for _ in 0..count {
                    synthetic_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                }
                write_all_flush(&mut self.write, &synthetic_response).await?;
                self.prepared.skipped_parses.clear();
                self.prepared.batch_operations.clear();
            }
        } else {
            // For Sync, exit async mode
            server.set_async_mode(false);
        }

        self.execute_server_roundtrip(None, server).await?;

        self.stats.query();
        server.stats.query(
            query_start_at.elapsed().as_micros() as u64,
            self.server_parameters.get_application_name(),
        );

        self.buffer.clear();
        // Reset batch state for next batch
        self.prepared.reset_batch();

        if self.complete_transaction_if_needed(server, true) {
            return Ok(TransactionAction::Break);
        }
        if server.in_aborted() {
            return Ok(TransactionAction::BreakWaitRollback);
        }

        Ok(TransactionAction::Continue)
    }

    /// Handle CopyData (d) message.
    /// Returns the action to take after processing.
    #[inline]
    async fn handle_copy_data(
        &mut self,
        message: &BytesMut,
        server: &mut Server,
    ) -> Result<TransactionAction, Error> {
        self.ensure_copy_mode(server)?;
        self.buffer.put(&message[..]);

        // Want to limit buffer size
        if self.buffer.len() > BUFFER_FLUSH_THRESHOLD {
            // Forward the data to the server
            server.send_and_flush(&self.buffer).await?;
            self.buffer.clear();
        }

        Ok(TransactionAction::Continue)
    }

    /// Handle CopyDone (c) or CopyFail (f) message.
    /// Returns the action to take after processing.
    async fn handle_copy_done_fail(
        &mut self,
        message: &BytesMut,
        server: &mut Server,
    ) -> Result<TransactionAction, Error> {
        self.ensure_copy_mode(server)?;
        // We may already have some copy data in the buffer, add this message to buffer
        self.buffer.put(&message[..]);

        server.send_and_flush(&self.buffer).await?;

        // Clear the buffer
        self.buffer.clear();

        let response = server
            .recv(&mut self.write, Some(&mut self.server_parameters))
            .await?;

        self.stats.active_write();
        match write_all_flush(&mut self.write, &response).await {
            Ok(_) => self.stats.active_idle(),
            Err(err) => {
                server.wait_available().await;
                server.mark_bad(
                    format!(
                        "flush to client {} response after copy done: {:?}",
                        self.addr, err
                    )
                    .as_str(),
                );
                return Err(err);
            }
        };

        if self.complete_transaction_if_needed(server, false) {
            return Ok(TransactionAction::Break);
        }

        Ok(TransactionAction::Continue)
    }

    /// Handle a connected and authenticated client.
    pub async fn handle(&mut self) -> Result<(), Error> {
        // The client wants to cancel a query it has issued previously.
        if self.cancel_mode {
            return self.handle_cancel_mode().await;
        }
        self.stats.register(self.stats.clone());
        let pool = match self.admin {
            true => None,
            false => Some(self.get_pool().await?),
        };

        let mut query_start_at: quanta::Instant;
        let mut wait_rollback_from_client: bool;
        loop {
            wait_rollback_from_client = false;
            self.stats.idle_read();
            let message = match read_message(&mut self.read, self.max_memory_usage).await {
                Ok(message) => message,
                Err(err) => return self.process_error(err).await,
            };
            if message[0] as char == 'X' {
                debug!("Client {} sent Terminate [X]", self.addr);
                self.stats.disconnect();
                return Ok(());
            }
            if SHUTDOWN_IN_PROGRESS.load(Ordering::Relaxed) && !self.admin {
                warn!(
                    "Dropping client {:?} because connection pooler is shutting down",
                    self.addr
                );
                error_response_terminal(&mut self.write, "pooler is shut down now", "58006")
                    .await?;
                self.stats.disconnect();
                return Ok(());
            }
            // Handle admin database queries.
            if self.admin {
                handle_admin(&mut self.write, message, self.client_server_map.clone())
                    .await
                    .inspect_err(|_| self.stats.disconnect())?;
                continue;
            }

            query_start_at = now();
            let current_pool = pool.as_ref().unwrap();

            // Handle fast queries (pooler check, DEALLOCATE) without server
            if self.try_handle_without_server(&message).await? {
                continue;
            }

            // Micro-optimization: if first message is standalone BEGIN,
            // synthesize response and defer actual BEGIN to next query.
            // BEGIN itself doesn't perform any server operations, it only
            // reserves a connection which is wasteful if client is slow.
            if is_standalone_begin(&message) && self.client_pending_begin.is_none() {
                debug!(
                    "Synthesizing response for standalone BEGIN from {}",
                    self.addr
                );

                // Send synthetic response: CommandComplete('BEGIN') + ReadyForQuery('T')
                // CommandComplete: 'C' + len(10) + "BEGIN\0"
                // ReadyForQuery: 'Z' + len(5) + 'T' (in transaction)
                const SYNTHETIC_BEGIN_RESPONSE: &[u8] = &[
                    b'C', 0, 0, 0, 10, b'B', b'E', b'G', b'I', b'N', 0, // CommandComplete
                    b'Z', 0, 0, 0, 5, b'T', // ReadyForQuery('T')
                ];
                write_all_flush(&mut self.write, SYNTHETIC_BEGIN_RESPONSE).await?;

                // Store pending BEGIN for next query
                self.client_pending_begin = Some(message);
                continue; // Return to main loop, wait for next message
            }

            // Check if we have a pending BEGIN to send with this query
            let pending_begin = self.client_pending_begin.take();

            let shutdown_in_progress = {
                // start server.
                // Grab a server from the pool.
                let connecting_at = now();
                self.stats.waiting();
                let mut conn = loop {
                    match current_pool.database.get().await {
                        Ok(mut conn) => {
                            // check server candidate in canceled pids.
                            {
                                let mut guard = CANCELED_PIDS.lock();
                                if guard.contains(&conn.get_process_id()) {
                                    guard.remove(&conn.get_process_id());
                                    conn.mark_bad("because was canceled");
                                    continue; // try to find another server.
                                }
                            }
                            // checkin_cleanup before give server to client.
                            match conn.checkin_cleanup().await {
                                Ok(()) => break conn,
                                Err(err) => {
                                    warn!(
                                        "Server {} cleanup error: {:?}",
                                        conn.address_to_string(),
                                        err
                                    );
                                    continue;
                                }
                            };
                        }
                        Err(err) => {
                            // Client is attempting to get results from the server,
                            // but we were unable to grab a connection from the pool
                            // We'll send back an error message and clean the extended
                            // protocol buffer
                            self.stats.idle_read();
                            current_pool.address.stats.error();
                            self.stats.checkout_error();

                            if message[0] as char == 'S' {
                                self.reset_buffered_state();
                            }

                            error_response(
                                &mut self.write,
                                format!("Could not get a database connection from the pool. All servers may be busy or down. Error details: {err}. Please try again later.").as_str(),
                                "53300",
                            )
                            .await?;

                            error!(
                                "Failed to get connection from pool: {{ pool_name: {:?}, username: {:?}, error: \"{:?}\" }}",
                                self.pool_name, self.username, err
                            );
                            return Err(Error::AllServersDown);
                        }
                    };
                };
                let server = conn.deref_mut();
                server.stats.active(self.stats.application_name());
                server.stats.checkout_time(
                    connecting_at.elapsed().as_micros() as u64,
                    self.stats.application_name(),
                );
                let server_active_at = now();

                // Server is assigned to the client in case the client wants to
                // cancel a query later.
                server.claim(self.process_id, self.secret_key);
                self.connected_to_server = true;

                // Signal that client is now in transaction (has server connection)
                CLIENTS_IN_TRANSACTIONS.fetch_add(1, Ordering::Relaxed);

                // Update statistics
                self.stats.active_idle();
                self.last_server_stats = Some(server.stats.clone());

                debug!("Client {:?} talking to server {}", self.addr, server);

                if current_pool.settings.sync_server_parameters {
                    server.sync_parameters(&self.server_parameters).await?;
                }
                server.set_async_mode(false);

                // If we deferred BEGIN, send it to server first (without forwarding response to client)
                // Client already received synthetic response, so we discard the real server response
                if let Some(begin_msg) = pending_begin {
                    debug!("Sending deferred BEGIN to server for client {}", self.addr);

                    // Send BEGIN to server
                    server
                        .send_and_flush_timeout(&begin_msg, Duration::from_secs(5))
                        .await?;

                    // Receive and discard response (client already got synthetic response)
                    // Using sink() to avoid forwarding to client
                    loop {
                        match server
                            .recv(&mut tokio::io::sink(), Some(&mut self.server_parameters))
                            .await
                        {
                            Ok(_) => {
                                if !server.is_data_available() {
                                    break;
                                }
                            }
                            Err(err) => {
                                server.mark_bad(&format!("deferred BEGIN failed: {:?}", err));
                                return Err(err);
                            }
                        }
                    }

                    // Reset query_start_at for the actual query
                    query_start_at = now();
                }

                let mut initial_message = Some(message);

                // Transaction loop. Multiple queries can be issued by the client here.
                // The connection belongs to the client until the transaction is over,
                // or until the client disconnects if we are in session mode.
                //
                // If the client is in session mode, no more custom protocol
                // commands will be accepted.
                loop {
                    let message = match initial_message {
                        None => {
                            self.stats.active_read();
                            match read_message(&mut self.read, self.max_memory_usage).await {
                                Ok(message) => message,
                                Err(err) => {
                                    self.stats.disconnect();
                                    server.checkin_cleanup().await?;
                                    return self.process_error(err).await;
                                }
                            }
                        }

                        Some(message) => {
                            initial_message = None;
                            message
                        }
                    };
                    self.stats.active_idle();

                    // The message will be forwarded to the server intact. We still would like to
                    // parse it below to figure out what to do with it.

                    // Safe to unwrap because we know this message has a certain length and has the code
                    // This reads the first byte without advancing the internal pointer and mutating the bytes
                    let code = *message.first().unwrap() as char;

                    // Process message and get action
                    let action = match code {
                        // Query
                        'Q' => {
                            self.handle_simple_query(&message, server, query_start_at)
                                .await?
                        }

                        // Terminate
                        'X' => {
                            // принудительно закрываем чтобы не допустить длинную транзакцию
                            server.checkin_cleanup().await?;
                            self.stats.disconnect();
                            self.release();
                            return Ok(());
                        }

                        // Parse
                        // The query with placeholders is here, e.g. `SELECT * FROM users WHERE email = $1 AND active = $2`.
                        'P' => {
                            self.process_parse_immediate(message, current_pool, server)
                                .await?;
                            TransactionAction::Continue
                        }

                        // Bind
                        'B' => {
                            self.process_bind_immediate(message, current_pool, server)
                                .await?;
                            TransactionAction::Continue
                        }

                        // Describe
                        // Command a client can issue to describe a previously prepared named statement.
                        'D' => {
                            self.process_describe_immediate(message, current_pool, server)
                                .await?;
                            TransactionAction::Continue
                        }

                        // Execute
                        // Execute a prepared statement prepared in `P` and bound in `B`.
                        'E' => {
                            self.buffer.put(&message[..]);
                            // Track Execute for correct ParseComplete insertion position
                            self.prepared.batch_operations.push(BatchOperation::Execute);
                            TransactionAction::Continue
                        }

                        // Close
                        // Close the prepared statement.
                        'C' => {
                            self.process_close_immediate(message)?;
                            TransactionAction::Continue
                        }

                        // Sync or Flush
                        // Frontend (client) is asking for the query result now.
                        'S' | 'H' => {
                            self.handle_sync_flush(&message, server, query_start_at, code)
                                .await?
                        }

                        // CopyData
                        'd' => self.handle_copy_data(&message, server).await?,

                        // CopyDone or CopyFail
                        // Copy is done, successfully or not.
                        'c' | 'f' => self.handle_copy_done_fail(&message, server).await?,

                        // Some unexpected message. We either did not implement the protocol correctly
                        // or this is not a Postgres client we're talking to.
                        _ => {
                            error!("Unexpected code: {code}");
                            TransactionAction::Continue
                        }
                    };

                    // Handle the action returned by message processor
                    match action {
                        TransactionAction::Continue => {}
                        TransactionAction::Break => break,
                        TransactionAction::BreakWaitRollback => {
                            wait_rollback_from_client = true;
                            break;
                        }
                    }
                }
                // Check if shutdown is in progress - if so, mark server as bad to release PG connection
                // and prepare to send error to client on next query
                let shutdown_in_progress = SHUTDOWN_IN_PROGRESS.load(Ordering::Relaxed);
                if shutdown_in_progress {
                    server.mark_bad("graceful shutdown - releasing server connection");
                } else if !server.is_async() {
                    server.checkin_cleanup().await?;
                }
                server
                    .stats
                    .add_xact_time_and_idle(server_active_at.elapsed().as_micros() as u64);
                // The server is no longer bound to us, we can't cancel it's queries anymore.
                self.release();
                server.stats.wait_idle();
                shutdown_in_progress
            }; // release server.

            if !self.client_last_messages_in_tx.is_empty() {
                self.stats.idle_write(); // go to idle_read if success.
                write_all_flush(&mut self.write, &self.client_last_messages_in_tx).await?;
                self.client_last_messages_in_tx.clear();
            }

            // Signal that client finished transaction (released server connection)
            CLIENTS_IN_TRANSACTIONS.fetch_sub(1, Ordering::Relaxed);
            self.connected_to_server = false;

            // If shutdown is in progress, send error to client and exit
            if shutdown_in_progress {
                error_response_terminal(&mut self.write, "pooler is shut down now", "58006")
                    .await?;
                self.stats.disconnect();
                return Ok(());
            }

            if wait_rollback_from_client {
                // release from server and wait rollback from client;
                self.wait_rollback().await?;
            }

            self.stats.idle_read();
            // capacity растет - вырастает rss у процесса.
            self.client_last_messages_in_tx.shrink_if_needed();
            self.buffer.shrink_if_needed();
        }
    }

    pub(crate) async fn execute_server_roundtrip(
        &mut self,
        message: Option<&BytesMut>,
        server: &mut Server,
    ) -> Result<(), Error> {
        let message = message.unwrap_or(&self.buffer);

        // Send message with timeout
        server
            .send_and_flush_timeout(message, Duration::from_secs(5))
            .await?;

        // Debug log: client -> server
        log_client_to_server(&self.addr.to_string(), server.get_process_id(), message);

        // Pre-calculate fast release conditions (avoids repeated checks)
        let can_fast_release = self.transaction_mode;

        // Single initial state update
        self.stats.active_idle();

        // Read all data the server has to offer, which can be multiple messages
        // buffered in 8 KiB chunks.
        loop {
            let mut response = match server
                .recv(&mut self.write, Some(&mut self.server_parameters))
                .await
            {
                Ok(msg) => msg,
                Err(err) => {
                    server.wait_available().await;
                    let mut msg = String::with_capacity(64);
                    use std::fmt::Write;
                    let _ = write!(msg, "loop with client {}: {:?}", self.addr, err);
                    server.mark_bad(&msg);
                    return Err(err);
                }
            };

            // Insert pending ParseComplete messages based on batch_operations order
            // This ensures ParseComplete messages are inserted in the correct position
            // relative to other responses (ParameterDescription, BindComplete, etc.)
            if !self.prepared.batch_operations.is_empty()
                && !self.prepared.skipped_parses.is_empty()
            {
                response = self.reorder_parse_complete_responses(response);
            }

            // Insert pending CloseComplete messages after last CloseComplete from server
            if self.prepared.pending_close_complete > 0 {
                let (new_response, inserted) = insert_close_complete_after_last_close_complete(
                    response,
                    self.prepared.pending_close_complete,
                );
                response = new_response;
                self.prepared.pending_close_complete -= inserted;
            }

            // Debug log: server -> client (after all modifications to show what client actually receives)
            log_server_to_client(&self.addr.to_string(), server.get_process_id(), &response);

            // Fast path: early release check before expensive operations
            // This is the most common case in transaction mode
            // Don't use fast_release when there are pending prepared statement operations
            // to avoid protocol violations if client disconnects before receiving the response
            if can_fast_release
                && !server.is_data_available()
                && !server.in_transaction()
                && !server.in_copy_mode()
                && !server.is_async()
                && self.prepared.skipped_parses.is_empty()
                && self.prepared.pending_close_complete == 0
            {
                self.client_last_messages_in_tx.put(&response[..]);
                break;
            }

            // Write response to client
            self.stats.active_write();
            if let Err(err_write) = write_all_flush(&mut self.write, &response).await {
                warn!(
                    "Write to client {} failed: {:?}, draining server [{}] data",
                    self.addr,
                    err_write,
                    server.get_process_id()
                );
                server.wait_available().await;
                if server.is_async() || server.in_copy_mode() {
                    server.mark_bad(
                        format!("flush to client {} {:?}", self.addr, err_write).as_str(),
                    );
                    return Err(err_write);
                }
            }

            self.stats.active_idle();

            // Early exit check
            if !server.is_data_available() || server.in_aborted() {
                break;
            }
        }

        Ok(())
    }
}
