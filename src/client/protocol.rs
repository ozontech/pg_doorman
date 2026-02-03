use bytes::{BufMut, BytesMut};
use log::debug;
use std::convert::TryInto;
use std::sync::Arc;

use crate::errors::Error;
use crate::messages::{error_response, Bind, Close, Describe, Parse};
use crate::pool::ConnectionPool;
use crate::server::Server;

use super::core::{
    BatchOperation, CachedStatement, Client, ParseCompleteTarget, PreparedStatementKey,
    SkippedParse,
};
use super::PREPARED_STATEMENT_COUNTER;

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    /// Makes sure the checked out server has the prepared statement and sends it to the server if it doesn't
    pub(crate) async fn ensure_prepared_statement_is_on_server(
        &mut self,
        key: PreparedStatementKey,
        pool: &ConnectionPool,
        server: &mut Server,
    ) -> Result<(), Error> {
        let cached = self.prepared.cache.get(&key).cloned();
        match cached {
            Some(cached) => {
                debug!("Prepared statement `{key:?}` found in cache");
                // Get the server-side name (may be async_name for async clients)
                let server_name = cached.server_name().to_string();
                // In this case we want to send the parse message to the server
                // since pgcat is initiating the prepared statement on this specific server
                match self
                    .register_parse_to_server_cache(
                        true,
                        &cached.hash,
                        &cached.parse,
                        &server_name,
                        pool,
                        server,
                    )
                    .await
                {
                    Ok(_) => (),
                    Err(err) => match err {
                        Error::PreparedStatementError => {
                            debug!("Removed {key:?} from client cache");
                            self.prepared.cache.pop(&key);
                        }

                        _ => {
                            return Err(err);
                        }
                    },
                }
            }

            None => {
                return Err(Error::ClientError(format!(
                    "prepared statement `{key:?}` not found"
                )))
            }
        };

        Ok(())
    }

    /// Register the parse to the server cache and send it to the server if requested (ie. requested by pgcat)
    ///
    /// Also updates the pool LRU that this parse was used recently
    ///
    /// # Arguments
    /// * `should_send_parse_to_server` - Whether to actually send Parse to server
    /// * `hash` - Hash of the statement for pool LRU promotion
    /// * `parse` - The Parse message containing query text and parameters
    /// * `server_name` - The name to use on the server (may differ from parse.name for async clients)
    /// * `pool` - Connection pool for LRU promotion
    /// * `server` - Server connection to register on
    pub(crate) async fn register_parse_to_server_cache(
        &self,
        should_send_parse_to_server: bool,
        hash: &u64,
        parse: &Arc<Parse>,
        server_name: &str,
        pool: &ConnectionPool,
        server: &mut Server,
    ) -> Result<(), Error> {
        // We want to promote this in the pool's LRU
        pool.promote_prepared_statement_hash(hash);

        debug!("Checking for prepared statement {}", server_name);

        server
            .register_prepared_statement(parse, server_name, should_send_parse_to_server)
            .await?;

        Ok(())
    }

    /// Process Parse message immediately without buffering.
    /// Adds data directly to self.buffer or response_message_queue_buffer for cached statements.
    pub(crate) async fn process_parse_immediate(
        &mut self,
        message: BytesMut,
        pool: &ConnectionPool,
        server: &mut Server,
    ) -> Result<(), Error> {
        // Avoid parsing if prepared statements not enabled
        if !self.prepared.enabled {
            debug!("Anonymous parse message");
            let first_char_in_name = *message.get(5).unwrap_or(&0);
            if first_char_in_name != 0 {
                // This is a named prepared statement while prepared statements are disabled
                // Server connection state will need to be cleared at checkin
                server.mark_dirty();
            }
            // Add directly to buffer
            self.buffer.put(&message[..]);
            return Ok(());
        }

        let client_given_name = Parse::get_name(&message)?;
        let parse: Parse = (&message).try_into()?;

        // Compute the hash of the parse statement
        let hash = parse.get_hash();

        // Always use pool cache to get shared Arc<Parse> (saves memory for async clients too)
        let shared_parse = match pool.register_parse_to_cache(hash, &parse) {
            Some(parse) => parse,
            None => {
                return Err(Error::ClientError(format!(
                    "Could not store Prepared statement `{client_given_name}`"
                )))
            }
        };

        // For async clients, generate a unique name to avoid "prepared statement already exists" errors
        // The query text is still shared via Arc<Parse> from pool cache
        let async_name = if self.prepared.async_client {
            Some(format!(
                "DOORMAN_async_{}",
                PREPARED_STATEMENT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ))
        } else {
            None
        };

        debug!(
            "Renamed prepared statement `{}` to `{}` (async_name: {:?}) and saved to cache",
            client_given_name, shared_parse.name, async_name
        );

        // For anonymous prepared statements, use hash as key to avoid collisions
        // Save hash for anonymous prepared statement lookup
        if client_given_name.is_empty() {
            self.prepared.last_anonymous_hash = Some(hash);
        }
        let cache_key = PreparedStatementKey::from_name_or_hash(client_given_name, hash);

        let cached = CachedStatement {
            parse: shared_parse.clone(),
            hash,
            async_name: async_name.clone(),
        };
        self.prepared.cache.put(cache_key, cached);

        // Update prepared cache stats after modification
        self.update_prepared_cache_stats();

        // Determine the server-side statement name
        let server_stmt_name = async_name
            .as_deref()
            .unwrap_or(&shared_parse.name)
            .to_string();

        // Check if server already has this prepared statement
        // For async clients with unique names, this will always be false (new unique name)
        if server.has_prepared_statement(&server_stmt_name) {
            // For async clients, always send Parse to get real ParseComplete from server
            if self.prepared.async_client {
                debug!(
                    "Async client: sending Parse `{}` to server even though cached",
                    server_stmt_name
                );

                // Add parse message to buffer with the server statement name
                let parse_bytes = shared_parse
                    .as_ref()
                    .to_bytes_with_name(&server_stmt_name)?;
                self.buffer.put(&parse_bytes[..]);
            } else {
                // We don't want to send the parse message to the server
                // Track this skipped Parse - ParseComplete will be inserted before BindComplete in response
                debug!(
                    "Parse skipped for `{}` (already on server), will insert ParseComplete later",
                    server_stmt_name
                );
                // insert_at_beginning starts as false. It will be set to true later
                // if a new Parse is sent to server AFTER this skipped Parse.
                // This ensures correct ordering: ParseComplete for skipped Parse that comes
                // BEFORE new Parse should be at the beginning of the response.
                // has_bind starts as false - will be set to true when Bind is processed.
                self.prepared.skipped_parses.push(SkippedParse {
                    statement_name: server_stmt_name.clone(),
                    target: ParseCompleteTarget::BindComplete,
                    insert_at_beginning: false,
                    has_bind: false,
                });
                // Track operation order for correct ParseComplete insertion
                self.prepared
                    .batch_operations
                    .push(BatchOperation::ParseSkipped {
                        statement_name: server_stmt_name.clone(),
                    });
            }
        } else {
            debug!(
                "Prepared statement `{}` not found in server cache",
                server_stmt_name
            );
            // Register to server cache (this may send eviction close to server)
            self.register_parse_to_server_cache(
                false,
                &hash,
                &shared_parse,
                &server_stmt_name,
                pool,
                server,
            )
            .await?;

            // Before sending new Parse, mark pending skipped_parses as insert_at_beginning=true
            // because their ParseComplete should come before the ParseComplete from server.
            // BUT only if they don't have a corresponding Bind yet - if they have Bind,
            // their ParseComplete should be inserted before BindComplete, not at beginning.
            for skipped in &mut self.prepared.skipped_parses {
                if !skipped.insert_at_beginning && !skipped.has_bind {
                    skipped.insert_at_beginning = true;
                }
            }

            // Add parse message to buffer with the server statement name
            let parse_bytes = shared_parse
                .as_ref()
                .to_bytes_with_name(&server_stmt_name)?;
            self.buffer.put(&parse_bytes[..]);

            // Track that we sent a Parse to server in this batch
            self.prepared.parses_sent_in_batch += 1;

            // Track operation order for correct ParseComplete insertion
            self.prepared
                .batch_operations
                .push(BatchOperation::ParseSent {
                    statement_name: server_stmt_name.clone(),
                });
        }

        Ok(())
    }

    /// Get lookup key for prepared statement (handles anonymous statements)
    async fn get_prepared_statement_lookup_key(
        &mut self,
        client_given_name: &str,
    ) -> Result<PreparedStatementKey, Error> {
        if client_given_name.is_empty() {
            match self.prepared.last_anonymous_hash {
                Some(hash) => Ok(PreparedStatementKey::Anonymous(hash)),
                None => {
                    debug!("Got anonymous prepared statement reference but no anonymous prepared statement exists");
                    error_response(
                        &mut self.write,
                        "prepared statement \"\" does not exist",
                        "58000",
                    )
                    .await?;
                    Err(Error::ClientError(
                        "Anonymous prepared statement doesn't exist".to_string(),
                    ))
                }
            }
        } else {
            Ok(PreparedStatementKey::Named(client_given_name.to_string()))
        }
    }

    /// Process Bind message immediately without buffering.
    /// Adds data directly to self.buffer.
    pub(crate) async fn process_bind_immediate(
        &mut self,
        message: BytesMut,
        pool: &ConnectionPool,
        server: &mut Server,
    ) -> Result<(), Error> {
        // Avoid parsing if prepared statements not enabled
        if !self.prepared.enabled {
            debug!("Anonymous bind message");
            self.buffer.put(&message[..]);
            return Ok(());
        }

        let client_given_name = Bind::get_name(&message)?;
        let lookup_key = self
            .get_prepared_statement_lookup_key(&client_given_name)
            .await?;

        let cached = self.prepared.cache.get(&lookup_key).cloned();
        match cached {
            Some(cached) => {
                let server_name = cached.server_name().to_string();
                let message = Bind::rename(message, &server_name)?;

                debug!("Rewrote bind `{}` to `{}`", client_given_name, server_name);

                // Ensure prepared statement is on server
                // For async clients, Parse may NOT be in buffer if client reuses cached prepared statement
                // (e.g., asyncpg sends only Bind without Parse for cached statements)
                self.ensure_prepared_statement_is_on_server(lookup_key, pool, server)
                    .await?;

                // Mark the corresponding skipped_parse as having a Bind.
                // This prevents it from being marked as insert_at_beginning when a new Parse arrives,
                // because its ParseComplete should be inserted before BindComplete, not at beginning.
                if let Some(skipped) = self.prepared.skipped_parses.iter_mut().find(|s| {
                    s.statement_name == server_name
                        && s.target == ParseCompleteTarget::BindComplete
                        && !s.has_bind
                }) {
                    skipped.has_bind = true;
                }

                // Add directly to buffer
                self.buffer.put(&message[..]);

                // Track operation order for correct ParseComplete insertion
                self.prepared.batch_operations.push(BatchOperation::Bind {
                    statement_name: server_name,
                });

                Ok(())
            }
            None => {
                debug!("Got bind for unknown prepared statement {client_given_name:?}");

                error_response(
                    &mut self.write,
                    &format!("prepared statement \"{client_given_name}\" does not exist"),
                    "58000",
                )
                .await?;

                Err(Error::ClientError(format!(
                    "Prepared statement `{client_given_name}` doesn't exist"
                )))
            }
        }
    }

    /// Process Describe message immediately without buffering.
    /// Adds data directly to self.buffer.
    pub(crate) async fn process_describe_immediate(
        &mut self,
        message: BytesMut,
        pool: &ConnectionPool,
        server: &mut Server,
    ) -> Result<(), Error> {
        // Avoid parsing if prepared statements not enabled
        if !self.prepared.enabled {
            debug!("Anonymous describe message");
            self.buffer.put(&message[..]);
            return Ok(());
        }

        let describe: Describe = (&message).try_into()?;
        if describe.target == 'P' {
            debug!("Portal describe message");
            self.buffer.put(&message[..]);
            // Track portal describe for correct ParseComplete insertion position
            self.prepared
                .batch_operations
                .push(BatchOperation::DescribePortal);
            return Ok(());
        }

        let client_given_name = describe.statement_name.clone();
        let lookup_key = self
            .get_prepared_statement_lookup_key(&client_given_name)
            .await?;

        let cached = self.prepared.cache.get(&lookup_key).cloned();
        match cached {
            Some(cached) => {
                // Get the server-side statement name
                let server_name = cached.server_name().to_string();
                let describe = describe.rename(&server_name);

                debug!(
                    "Rewrote describe `{}` to `{}`",
                    client_given_name, describe.statement_name
                );

                // Ensure prepared statement is on server
                // For async clients, Parse may NOT be in buffer if client reuses cached prepared statement
                // (e.g., asyncpg sends only Describe without Parse for cached statements)
                self.ensure_prepared_statement_is_on_server(lookup_key, pool, server)
                    .await?;

                // If Parse was skipped for this statement, we need to insert ParseComplete
                // before ParameterDescription in the response (not before BindComplete).
                // Find and remove the skipped parse entry, then add a new one with ParameterDescription target.
                // Using position() + remove() + push() instead of iter_mut().find() to avoid issues
                // when multiple Parse operations for the same statement are skipped in a batch.
                if let Some(idx) = self.prepared.skipped_parses.iter().position(|s| {
                    s.statement_name == server_name && s.target == ParseCompleteTarget::BindComplete
                }) {
                    debug!(
                        "Parse was skipped for `{}`, will insert ParseComplete before ParameterDescription",
                        server_name
                    );
                    let insert_at_beginning = self.prepared.skipped_parses[idx].insert_at_beginning;
                    let has_bind = self.prepared.skipped_parses[idx].has_bind;
                    self.prepared.skipped_parses.remove(idx);
                    self.prepared.skipped_parses.push(SkippedParse {
                        statement_name: server_name.clone(),
                        target: ParseCompleteTarget::ParameterDescription,
                        insert_at_beginning,
                        has_bind,
                    });
                }

                // Add directly to buffer
                let describe_bytes: BytesMut = describe.try_into()?;
                self.buffer.put(&describe_bytes[..]);

                // Track operation order for correct ParseComplete insertion
                self.prepared
                    .batch_operations
                    .push(BatchOperation::Describe {
                        statement_name: server_name,
                    });

                Ok(())
            }

            None => {
                debug!("Got describe for unknown prepared statement {describe:?}");

                error_response(
                    &mut self.write,
                    &format!("prepared statement \"{client_given_name}\" does not exist"),
                    "58000",
                )
                .await?;

                Err(Error::ClientError(format!(
                    "Prepared statement `{client_given_name}` doesn't exist"
                )))
            }
        }
    }

    /// Process Close message immediately without buffering.
    /// For prepared statements: removes from cache and increments pending_close_complete counter.
    /// For others: adds data directly to self.buffer.
    pub(crate) fn process_close_immediate(&mut self, message: BytesMut) -> Result<(), Error> {
        let close: Close = (&message).try_into()?;

        // Always add Close to buffer in extended query protocol
        // This ensures Close is sent to server when followed by Flush
        self.buffer.put(&message[..]);

        // Track Close operation for correct ParseComplete insertion order
        self.prepared.batch_operations.push(BatchOperation::Close);

        // Remove from prepared statements cache if it's a named prepared statement
        if self.prepared.enabled && close.is_prepared_statement() && !close.anonymous() {
            let key = PreparedStatementKey::Named(close.name.clone());
            self.prepared.cache.pop(&key);
        }

        Ok(())
    }

    pub(crate) fn reset_buffered_state(&mut self) {
        self.buffer.clear();
        self.prepared.pending_close_complete = 0;
        self.prepared.skipped_parses.clear();
        self.prepared.parses_sent_in_batch = 0;
    }
}
