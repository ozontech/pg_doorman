use bytes::{BufMut, BytesMut};
use log::debug;
use std::convert::TryInto;
use std::sync::Arc;

use crate::errors::Error;
use crate::messages::{error_response, Bind, Close, Describe, Parse};
use crate::pool::ConnectionPool;
use crate::server::Server;

use super::core::{Client, PreparedStatementKey};

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
        match self.prepared_statements.get(&key) {
            Some((parse, hash)) => {
                debug!("Prepared statement `{key:?}` found in cache");
                // In this case we want to send the parse message to the server
                // since pgcat is initiating the prepared statement on this specific server
                match self
                    .register_parse_to_server_cache(true, hash, parse, pool, server)
                    .await
                {
                    Ok(_) => (),
                    Err(err) => match err {
                        Error::PreparedStatementError => {
                            debug!("Removed {key:?} from client cache");
                            self.prepared_statements.remove(&key);
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
    pub(crate) async fn register_parse_to_server_cache(
        &self,
        should_send_parse_to_server: bool,
        hash: &u64,
        parse: &Arc<Parse>,
        pool: &ConnectionPool,
        server: &mut Server,
    ) -> Result<(), Error> {
        // We want to promote this in the pool's LRU
        pool.promote_prepared_statement_hash(hash);

        debug!("Checking for prepared statement {}", parse.name);

        server
            .register_prepared_statement(parse, should_send_parse_to_server)
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
        if !self.prepared_statements_enabled {
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

        // For async clients, create a new Parse with unique name instead of using pool cache
        // This avoids "prepared statement already exists" errors
        let new_parse = if self.async_client {
            // Use rewrite() to rename without cloning - it takes ownership and returns modified Parse
            Arc::new(parse.rewrite())
        } else {
            // Use pool cache for non-async clients
            match pool.register_parse_to_cache(hash, &parse) {
                Some(parse) => parse,
                None => {
                    return Err(Error::ClientError(format!(
                        "Could not store Prepared statement `{client_given_name}`"
                    )))
                }
            }
        };

        debug!(
            "Renamed prepared statement `{}` to `{}` and saved to cache",
            client_given_name, new_parse.name
        );

        // For anonymous prepared statements, use hash as key to avoid collisions
        // Save hash for anonymous prepared statement lookup
        if client_given_name.is_empty() {
            self.last_anonymous_prepared_hash = Some(hash);
        }
        let cache_key = PreparedStatementKey::from_name_or_hash(client_given_name, hash);

        self.prepared_statements
            .insert(cache_key, (new_parse.clone(), hash));

        // Check if server already has this prepared statement
        if server.has_prepared_statement(&new_parse.name) {
            // For async clients, always send Parse to get real ParseComplete from server
            if self.async_client {
                debug!(
                    "Async client: sending Parse `{}` to server even though cached",
                    new_parse.name
                );

                // Add parse message to buffer
                let parse_bytes: BytesMut = new_parse.as_ref().try_into()?;
                self.buffer.put(&parse_bytes[..]);
            } else {
                // We don't want to send the parse message to the server
                // Increment counter - ParseComplete will be inserted before BindComplete in response
                self.pending_parse_complete += 1;
            }
        } else {
            debug!(
                "Prepared statement `{}` not found in server cache",
                new_parse.name
            );

            // Register to server cache (this may send eviction close to server)
            self.register_parse_to_server_cache(false, &hash, &new_parse, pool, server)
                .await?;

            // Add parse message to buffer
            let parse_bytes: BytesMut = new_parse.as_ref().try_into()?;
            self.buffer.put(&parse_bytes[..]);
        }

        Ok(())
    }

    /// Get lookup key for prepared statement (handles anonymous statements)
    async fn get_prepared_statement_lookup_key(
        &mut self,
        client_given_name: &str,
    ) -> Result<PreparedStatementKey, Error> {
        if client_given_name.is_empty() {
            match self.last_anonymous_prepared_hash {
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
        if !self.prepared_statements_enabled {
            debug!("Anonymous bind message");
            self.buffer.put(&message[..]);
            return Ok(());
        }

        let client_given_name = Bind::get_name(&message)?;
        let lookup_key = self
            .get_prepared_statement_lookup_key(&client_given_name)
            .await?;

        match self.prepared_statements.get(&lookup_key) {
            Some((rewritten_parse, _)) => {
                let rewritten_name = rewritten_parse.name.clone();
                let message = Bind::rename(message, &rewritten_name)?;

                debug!(
                    "Rewrote bind `{}` to `{}`",
                    client_given_name, rewritten_name
                );

                // Ensure prepared statement is on server
                // Skip this for async clients - Parse is already in buffer and will be sent with Flush
                if !self.async_client {
                    self.ensure_prepared_statement_is_on_server(lookup_key, pool, server)
                        .await?;
                }

                // Add directly to buffer
                self.buffer.put(&message[..]);

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
        if !self.prepared_statements_enabled {
            debug!("Anonymous describe message");
            self.buffer.put(&message[..]);
            return Ok(());
        }

        let describe: Describe = (&message).try_into()?;
        if describe.target == 'P' {
            debug!("Portal describe message");
            self.buffer.put(&message[..]);
            return Ok(());
        }

        let client_given_name = describe.statement_name.clone();
        let lookup_key = self
            .get_prepared_statement_lookup_key(&client_given_name)
            .await?;

        match self.prepared_statements.get(&lookup_key) {
            Some((rewritten_parse, _)) => {
                // Clone what we need before any mutable borrows
                let rewritten_parse = rewritten_parse.clone();
                let describe = describe.rename(&rewritten_parse.name);

                debug!(
                    "Rewrote describe `{}` to `{}`",
                    client_given_name, describe.statement_name
                );

                // Ensure prepared statement is on server
                // Skip this for async clients - Parse is already in buffer and will be sent with Flush
                if !self.async_client {
                    self.ensure_prepared_statement_is_on_server(lookup_key, pool, server)
                        .await?;
                }

                // If Parse was skipped (pending_parse_complete > 0), we need to insert ParseComplete
                // before ParameterDescription in the response. We use a separate counter for this
                // because we can't send Parse again (statement already exists on server).
                if self.pending_parse_complete > 0 {
                    debug!(
                        "Parse was skipped for `{}`, will insert ParseComplete before ParameterDescription",
                        rewritten_parse.name
                    );
                    self.pending_parse_complete_for_describe += 1;
                    self.pending_parse_complete -= 1;
                }

                // Add directly to buffer
                let describe_bytes: BytesMut = describe.try_into()?;
                self.buffer.put(&describe_bytes[..]);

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

        // Remove from prepared statements cache if it's a named prepared statement
        if self.prepared_statements_enabled && close.is_prepared_statement() && !close.anonymous() {
            let key = PreparedStatementKey::Named(close.name.clone());
            self.prepared_statements.remove(&key);
        }

        Ok(())
    }

    pub(crate) fn reset_buffered_state(&mut self) {
        self.buffer.clear();
        self.pending_close_complete = 0;
        self.pending_parse_complete = 0;
        self.pending_parse_complete_for_describe = 0;
    }
}
