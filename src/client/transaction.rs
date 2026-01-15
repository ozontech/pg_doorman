use bytes::{BufMut, BytesMut};
use log::{debug, error, warn};
use std::ops::DerefMut;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::admin::handle_admin;
use crate::client::core::Client;
use crate::client::util::{CLIENT_COUNTER, QUERY_DEALLOCATE};
use crate::errors::Error;
use crate::messages::{
    check_query_response, command_complete, deallocate_response, error_message, error_response,
    error_response_terminal, insert_close_complete_after_last_close_complete,
    insert_parse_complete_before_bind_complete, read_message, ready_for_query, write_all_flush,
};
use crate::pool::{ConnectionPool, CANCELED_PIDS};
use crate::server::Server;
use crate::utils::comments::SqlCommentParser;

/// Buffer flush threshold in bytes (8 KiB).
/// When the buffer reaches this size, it will be flushed to avoid excessive memory usage.
const BUFFER_FLUSH_THRESHOLD: usize = 8192;

// Static ParseComplete message: '1' (1 byte) + length 4 (4 bytes big-endian)
const PARSE_COMPLETE_MSG: [u8; 5] = [b'1', 0, 0, 0, 4];

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

    /// Handle a connected and authenticated client.
    pub async fn handle(&mut self) -> Result<(), Error> {
        // The client wants to cancel a query it has issued previously.
        if self.cancel_mode {
            let (process_id, secret_key, address, port) = {
                let guard = self.client_server_map.lock();

                match guard.get(&(self.process_id, self.secret_key)) {
                    // Drop the mutex as soon as possible.
                    // We found the server the client is using for its query
                    // that it wants to cancel.
                    Some((process_id, secret_key, address, port)) => {
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

            // Opens a new separate connection to the server, sends the backend_id
            // and secret_key and then closes it for security reasons. No other interactions
            // take place.
            return Server::cancel(&address, port, process_id, secret_key).await;
        }
        self.stats.register(self.stats.clone());
        let client_counter = CLIENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        // Get a pool instance referenced by the most up-to-date
        // pointer. This ensures we always read the latest config
        // when starting a query.
        let mut pool: Option<ConnectionPool> = if self.admin {
            None
        } else {
            Some(self.get_pool(client_counter).await?)
        };

        let mut tx_counter = 0;
        let mut query_start_at: Instant;
        let mut wait_rollback_from_client: bool;
        loop {
            wait_rollback_from_client = false;
            // Read a complete message from the client, which normally would be
            // either a `Q` (query) or `P` (prepare, extended protocol).
            self.stats.idle_read();
            let message = match read_message(&mut self.read, self.max_memory_usage).await {
                Ok(message) => message,
                Err(err) => return self.process_error(err).await,
            };
            if message[0] as char == 'X' {
                self.stats.disconnect();
                return Ok(());
            }
            if self.shutdown.try_recv().is_ok() && !self.admin {
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
                match handle_admin(&mut self.write, message, self.client_server_map.clone()).await {
                    Ok(_) => (),
                    Err(err) => {
                        self.stats.disconnect();
                        return Err(err);
                    }
                }
                continue;
            }

            query_start_at = Instant::now();
            let current_pool = pool.as_ref().unwrap();

            match message[0] as char {
                'Q' => {
                    if self.pooler_check_query_request_vec.as_slice() == &message[..] {
                        // This is the first message in the transaction, since we are responding with 'IZ',
                        // then we can not expect a server connection and immediately send answer and exit transaction loop.
                        write_all_flush(&mut self.write, &check_query_response()).await?;
                        continue;
                    }
                    if message.len() < 40 && message.len() > QUERY_DEALLOCATE.len() + 5 {
                        // Do not pass simple query with deallocate, as it will run on an unknown server.
                        let query = &message[5..QUERY_DEALLOCATE.len() + 5];
                        if QUERY_DEALLOCATE == query {
                            write_all_flush(&mut self.write, &deallocate_response()).await?;
                            continue;
                        }
                    }
                }
                // Extended protocol messages (P, B, D, E, C) now go through to get a server connection
                // and are processed immediately without buffering in extended_protocol_data_buffer.
                // The server handles error states according to PostgreSQL protocol:
                // - If Parse fails, server enters "error in extended query" state
                // - All subsequent P, B, D, E, C are ignored until Sync
                // - On Sync, server sends ReadyForQuery and exits error state
                'P' | 'B' | 'D' | 'E' | 'C' | 'S' | 'H' => {
                    // Fall through to get server connection
                }

                _ => (),
            }

            {
                // start server.
                // Grab a server from the pool.
                let connecting_at = Instant::now();
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
                let server_active_at = Instant::now();

                // Server is assigned to the client in case the client wants to
                // cancel a query later.
                server.claim(self.process_id, self.secret_key);
                self.connected_to_server = true;

                // Update statistics
                self.stats.active_idle();
                self.last_server_stats = Some(server.stats.clone());

                debug!("Client {:?} talking to server {}", self.addr, server);

                if current_pool.settings.sync_server_parameters {
                    server.sync_parameters(&self.server_parameters).await?;
                }
                server.set_async_mode(false);

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

                    match code {
                        // Query
                        'Q' => {
                            self.send_and_receive_loop(Some(&message), server).await?;
                            self.stats.query();
                            server.stats.query(
                                query_start_at.elapsed().as_micros() as u64,
                                self.server_parameters.get_application_name(),
                            );
                            if server.in_aborted() {
                                wait_rollback_from_client = true;
                                break;
                            }

                            if self.complete_transaction_if_needed(server, false) {
                                self.stats.idle_read();
                                break;
                            }
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
                        }

                        // Bind
                        'B' => {
                            self.process_bind_immediate(message, current_pool, server)
                                .await?;
                        }

                        // Describe
                        // Command a client can issue to describe a previously prepared named statement.
                        'D' => {
                            self.process_describe_immediate(message, current_pool, server)
                                .await?;
                        }

                        // Execute
                        // Execute a prepared statement prepared in `P` and bound in `B`.
                        'E' => {
                            self.buffer.put(&message[..]);
                        }

                        // Close
                        // Close the prepared statement.
                        'C' => {
                            self.process_close_immediate(message)?;
                        }

                        // Sync
                        // Frontend (client) is asking for the query result now.
                        'S' | 'H' => {
                            // Add the sync/flush message to buffer
                            self.buffer.put(&message[..]);

                            if code == 'H' {
                                // For Flush, enter async mode
                                server.set_async_mode(true);
                                // Mark this client as async client forever
                                self.async_client = true;
                                debug!("Client requested flush, going async");
                            } else {
                                // For Sync, exit async mode
                                server.set_async_mode(false);
                            }

                            self.send_and_receive_loop(None, server)
                                .await
                                .inspect_err(|_| self.buffer.clear())?;

                            self.stats.query();
                            server.stats.query(
                                query_start_at.elapsed().as_micros() as u64,
                                self.server_parameters.get_application_name(),
                            );

                            self.buffer.clear();

                            if self.complete_transaction_if_needed(server, true) {
                                break;
                            }
                            if server.in_aborted() {
                                wait_rollback_from_client = true;
                                break;
                            }
                        }

                        // CopyData
                        'd' => {
                            self.ensure_copy_mode(server)?;
                            self.buffer.put(&message[..]);

                            // Want to limit buffer size
                            if self.buffer.len() > BUFFER_FLUSH_THRESHOLD {
                                // Forward the data to the server,
                                self.send_and_receive_loop(None, server)
                                    .await
                                    .inspect_err(|_| self.buffer.clear())?;
                                self.buffer.clear();
                            }
                        }

                        // CopyDone or CopyFail
                        // Copy is done, successfully or not.
                        'c' | 'f' => {
                            self.ensure_copy_mode(server)?;
                            // We may already have some copy data in the buffer, add this message to buffer
                            self.buffer.put(&message[..]);

                            self.send_and_receive_loop(None, server)
                                .await
                                .inspect_err(|_| self.buffer.clear())?;

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
                                break;
                            }
                        }

                        // Some unexpected message. We either did not implement the protocol correctly
                        // or this is not a Postgres client we're talking to.
                        _ => {
                            error!("Unexpected code: {code}");
                        }
                    }
                }
                if !server.is_async() {
                    server.checkin_cleanup().await?;
                }
                server
                    .stats
                    .add_xact_time_and_idle(server_active_at.elapsed().as_micros() as u64);
                // The server is no longer bound to us, we can't cancel it's queries anymore.
                self.release();
                server.stats.wait_idle();
            } // release server.
            if !self.client_last_messages_in_tx.is_empty() {
                self.stats.idle_write();
                write_all_flush(&mut self.write, &self.client_last_messages_in_tx).await?;
                self.client_last_messages_in_tx.clear();
            }
            self.connected_to_server = false;
            if wait_rollback_from_client {
                // release from server and wait rollback from client;
                self.wait_rollback().await?;
            }
            // change pool.
            if tx_counter % 10 == 0 && self.transaction_mode {
                pool = Some(self.get_pool(client_counter).await?);
            }
            tx_counter += 1;

            self.stats.idle_read();
            // capacity растет - вырастает rss у процесса.
            self.client_last_messages_in_tx.shrink_if_needed();
            self.buffer.shrink_if_needed();
        }
    }

    pub(crate) async fn send_and_receive_loop(
        &mut self,
        message: Option<&BytesMut>,
        server: &mut Server,
    ) -> Result<(), Error> {
        let message = message.unwrap_or(&self.buffer);

        // Send message with timeout
        server
            .send_and_flush_timeout(message, Duration::from_secs(5))
            .await?;

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

            // Insert pending ParseComplete messages before BindComplete
            // If no BindComplete found, insert at the beginning of response
            if self.pending_parse_complete > 0 {
                let (new_response, inserted) = insert_parse_complete_before_bind_complete(
                    response,
                    self.pending_parse_complete,
                );

                // If no BindComplete was found (inserted == 0), insert at the beginning
                if inserted == 0
                    && self.pending_parse_complete > 0
                    // If the server has more data, we can't insert ParseComplete messages
                    // because it would insert between DataRow messages
                    && !server.data_available
                {
                    let mut prefixed_response = BytesMut::with_capacity(
                        new_response.len() + (self.pending_parse_complete as usize * 5),
                    );

                    // Insert ParseComplete messages at the beginning
                    for _ in 0..self.pending_parse_complete {
                        prefixed_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                    }

                    // Append the original response
                    prefixed_response.extend_from_slice(&new_response);

                    response = prefixed_response;
                    self.pending_parse_complete = 0;
                } else {
                    response = new_response;
                    self.pending_parse_complete -= inserted;
                }
            }

            // Insert pending ParseComplete messages for Describe flow
            // (before ParameterDescription 't' or NoData 'n')
            if self.pending_parse_complete_for_describe > 0 && !server.data_available {
                let bytes = response.as_ref();

                // Find first ParameterDescription ('t') or NoData ('n') message
                let mut pos = 0;
                let mut insert_pos = None;
                while pos + 5 <= bytes.len() {
                    let msg_type = bytes[pos];
                    if msg_type == b't' || msg_type == b'n' {
                        insert_pos = Some(pos);
                        break;
                    }
                    let msg_len = i32::from_be_bytes([
                        bytes[pos + 1],
                        bytes[pos + 2],
                        bytes[pos + 3],
                        bytes[pos + 4],
                    ]) as usize;
                    pos += 1 + msg_len;
                }

                if let Some(insert_at) = insert_pos {
                    let count = self.pending_parse_complete_for_describe as usize;
                    let mut new_response = BytesMut::with_capacity(response.len() + count * 5);
                    new_response.extend_from_slice(&bytes[..insert_at]);
                    for _ in 0..count {
                        new_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                    }
                    new_response.extend_from_slice(&bytes[insert_at..]);
                    response = new_response;
                    self.pending_parse_complete_for_describe = 0;
                }
            }

            // Insert pending CloseComplete messages after last CloseComplete from server
            if self.pending_close_complete > 0 {
                let (new_response, inserted) = insert_close_complete_after_last_close_complete(
                    response,
                    self.pending_close_complete,
                );
                response = new_response;
                self.pending_close_complete -= inserted;
            }

            // Fast path: early release check before expensive operations
            // This is the most common case in transaction mode
            if can_fast_release
                && !server.is_data_available()
                && !server.in_transaction()
                && !server.in_copy_mode()
                && !server.is_async()
            {
                self.client_last_messages_in_tx.put(&response[..]);
                break;
            }

            // Write response to client
            self.stats.active_write();
            if let Err(err_write) = write_all_flush(&mut self.write, &response).await {
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
