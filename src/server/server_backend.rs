// Implementation of the PostgreSQL server (database) protocol.

// Standard library imports
use std::collections::{HashMap, VecDeque};
use std::mem;
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
use crate::auth::jwt::{new_claims, sign_with_jwt_priv_key};
use crate::auth::scram_client::ScramSha256;
use crate::config::{get_config, Address, User};
use crate::errors::{Error, ServerIdentifier};
use crate::messages::constants::*;
use crate::messages::PgErrorMsg;
use crate::messages::{
    md5_hash_password, read_message_data, simple_query, startup, sync, write_all_flush,
    BytesMutReader, Close, Parse,
};
use crate::pool::{ClientServerMap, CANCELED_PIDS};
use crate::stats::ServerStats;

use super::cleanup::CleanupState;
use super::parameters::ServerParameters;
use super::stream::{create_tcp_stream_inner, create_unix_stream_inner, StreamInner};
use super::{prepared_statements, protocol_io, startup_cancel};

/// Server state.
#[derive(Debug)]
pub struct Server {
    /// Server host, e.g. localhost,
    /// port, e.g. 5432, and role, e.g. primary or replica.
    pub(crate) address: Address,

    /// Server connection.
    pub(crate) stream: BufStream<StreamInner>,

    /// Our server response buffer. We buffer data before we give it to the client.
    pub(crate) buffer: BytesMut,

    /// Server information the server sent us over on startup.
    pub(crate) server_parameters: ServerParameters,

    /// Backend id and secret key used for query cancellation.
    process_id: i32,
    secret_key: i32,

    /// Is the server inside a transaction or idle.
    pub(crate) in_transaction: bool,

    /// Is the server inside a transaction and aborted.
    pub(crate) is_aborted: bool,

    /// Is there more data for the client to read.
    pub(crate) data_available: bool,

    /// Is the server in copy-in or copy-out modes
    pub(crate) in_copy_mode: bool,

    /// Is the server in async mode (Flush instead of Sync)
    async_mode: bool,

    /// Is the server broken? We'll remote it from the pool if so.
    pub(crate) bad: bool,

    /// If server connection requires reset statements before checkin
    pub(crate) cleanup_state: CleanupState,

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
    pub(crate) use_savepoint: bool,

    /// Log client parameter status changes
    pub(crate) log_client_parameter_status_changes: bool,

    /// Prepared statements
    pub(crate) prepared_statement_cache: Option<LruCache<String, ()>>,

    /// Prepared statement being currently registered on the server.
    pub(crate) registering_prepared_statement: VecDeque<String>,

    /// Max message size
    pub(crate) max_message_size: i32,
}

impl std::fmt::Display for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "[{}]-vp-{}-{}@{}:{}/{}/{}",
            self.process_id,
            self.address.virtual_pool_id,
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

    #[inline(always)]
    pub fn get_process_id(&self) -> i32 {
        self.process_id
    }

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
        protocol_io::send_and_flush_timeout(self, messages, duration).await
    }
    pub async fn send_and_flush(&mut self, messages: &BytesMut) -> Result<(), Error> {
        protocol_io::send_and_flush(self, messages).await
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
        prepared_statements::add_to_cache(&mut self.prepared_statement_cache, &self.stats, name)
    }

    fn remove_prepared_statement_from_cache(&mut self, name: &str) {
        prepared_statements::remove_from_cache(
            &mut self.prepared_statement_cache,
            &self.stats,
            name,
        );
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
                    // Determine which kind of authentication is required, if any.
                    let auth_code = match stream.read_i32().await {
                        Ok(auth_code) => auth_code,
                        Err(_) => {
                            return Err(Error::ServerStartupError(
                                "Failed to read authentication code from server".into(),
                                server_identifier.clone(),
                            ));
                        }
                    };
                    match auth_code {
                        AUTHENTICATION_SUCCESSFUL => (),
                        /* SASL begin */
                        SASL => match scram_client_auth {
                            None => {
                                return Err(Error::ServerAuthError(
                                    "server wants sasl auth, but it is not configured".into(),
                                    server_identifier.clone(),
                                ));
                            }
                            Some(_) => {
                                let sasl_len = (len - 8) as usize;
                                let mut sasl_auth = vec![0u8; sasl_len];

                                match stream.read_exact(&mut sasl_auth).await {
                                    Ok(_) => (),
                                    Err(_) => return Err(Error::ServerStartupError(
                                        "Failed to read SASL authentication message from server"
                                            .into(),
                                        server_identifier.clone(),
                                    )),
                                };

                                let sasl_type = String::from_utf8_lossy(&sasl_auth[..sasl_len - 2]);

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
                                        server_identifier.clone(),
                                    ));
                                }
                            }
                        },
                        SASL_CONTINUE => {
                            let mut sasl_data = vec![0u8; (len - 8) as usize];

                            match stream.read_exact(&mut sasl_data).await {
                                Ok(_) => (),
                                Err(_) => {
                                    return Err(Error::ServerStartupError(
                                        "Failed to read SASL continuation message from server"
                                            .into(),
                                        server_identifier.clone(),
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
                                        server_identifier.clone(),
                                    ))
                                }
                            };

                            scram_client_auth
                                .as_mut()
                                .unwrap()
                                .finish(&BytesMut::from(&sasl_final[..]))?;
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
                                    server_identifier.clone(),
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
                                let token = sign_with_jwt_priv_key(
                                    claims,
                                    server_password
                                        .strip_prefix(JWT_PRIV_KEY_PASSWORD_PREFIX)
                                        .unwrap()
                                        .to_string(),
                                )
                                .await
                                .map_err(|err| {
                                    Error::ServerAuthError(
                                        err.to_string(),
                                        server_identifier.clone(),
                                    )
                                })?;
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
                                            server_identifier.clone(),
                                        ));
                                    }
                                }
                            } else {
                                return Err(Error::ServerAuthError(
                                    "plain password is not supported".into(),
                                    server_identifier.clone(),
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
                                    server_identifier.clone(),
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
                                stream.read_buf(&mut salt).await.map_err(|err| {
                                    Error::ServerAuthError(
                                        format!("md5 authentication on the server: {err:?}"),
                                        server_identifier.clone(),
                                    )
                                })?;
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
                                            server_identifier.clone(),
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
                                server_identifier.clone(),
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
                                server_identifier.clone(),
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
                                    server_identifier.clone(),
                                ));
                            }
                            let mut error = vec![0u8; len as usize - 2 * mem::size_of::<u32>()];
                            stream.read_exact(&mut error).await.map_err(|err| {
                                Error::ServerStartupError(
                                    format!("while create new connection to postgresql received error, but can't read it: {err:?}"),
                                    server_identifier.clone(),
                                )
                            })?;

                            return match PgErrorMsg::parse(&error) {
                                Ok(f) => {
                                    error!(
                                        "Get server error - {} {}: {}",
                                        f.severity, f.code, f.message
                                    );
                                    Err(Error::ServerStartupError(
                                        f.message,
                                        server_identifier.clone(),
                                    ))
                                }
                                Err(err) => {
                                    error!("Get unparsed server error: {err:?}");
                                    Err(Error::ServerStartupError(
                                         format!("while create new connection to postgresql received error, but can't read it: {err:?}"),
                                         server_identifier.clone(),
                                     ))
                                }
                            };
                        }
                    };

                    return Err(Error::ServerError);
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
