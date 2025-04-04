use crate::errors::{ClientIdentifier, Error};
/// Handle clients by pretending to be a PostgreSQL server.
use bytes::{Buf, BufMut, BytesMut};
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::ops::DerefMut;
use std::str;
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicUsize, Arc};
use std::time::Instant;
use tokio::io::{split, AsyncReadExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::Sender;

use crate::admin::{generate_server_parameters_for_admin, handle_admin};
use crate::config::{addr_in_hba, get_config, PoolMode};
use crate::constants::*;
use crate::jwt_auth::get_user_name_from_jwt;
use crate::messages::*;
use crate::pool::{get_pool, ClientServerMap, ConnectionPool, CANCELED_PIDS};
use crate::rate_limit::RateLimiter;
use crate::scram_server::{
    parse_client_final_message, parse_client_first_message, parse_server_secret,
    prepare_server_final_message, prepare_server_first_response,
};
use crate::server::{Server, ServerParameters};
use crate::stats::{
    ClientStats, ServerStats, CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER,
    TLS_CONNECTION_COUNTER,
};

/// Incrementally count prepared statements
/// to avoid random conflicts in places where the random number generator is weak.
pub static PREPARED_STATEMENT_COUNTER: Lazy<Arc<AtomicUsize>> =
    Lazy::new(|| Arc::new(AtomicUsize::new(0)));
pub static CLIENT_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));
// Ignore deallocate queries from pgx.
static QUERY_DEALLOCATE: &[u8] = "deallocate \"".as_bytes();

/// Type of connection received from client.
enum ClientConnectionType {
    Startup,
    Tls,
    CancelQuery,
}

/// The client state. One of these is created per client.
pub struct Client<S, T> {
    /// The reads are buffered (8K by default).
    read: BufReader<S>,

    /// We buffer the writes ourselves because we know the protocol
    /// better than a stock buffer.
    write: T,

    /// Internal buffer, where we place messages until we have to flush
    /// them to the backend.
    buffer: BytesMut,

    /// Used to buffer response messages to the client
    response_message_queue_buffer: BytesMut,

    /// Address
    addr: std::net::SocketAddr,

    /// The client was started with the sole reason to cancel another running query.
    cancel_mode: bool,

    /// In transaction mode, the connection is released after each transaction.
    /// Session mode has slightly higher throughput per client, but lower capacity.
    transaction_mode: bool,

    /// For query cancellation, the client is given a random process ID and secret on startup.
    process_id: i32,
    secret_key: i32,

    /// Clients are mapped to servers while they use them. This allows a client
    /// to connect and cancel a query.
    client_server_map: ClientServerMap,

    /// Client parameters, e.g. user, client_encoding, etc.
    #[allow(dead_code)]
    parameters: HashMap<String, String>,

    /// Statistics related to this client
    stats: Arc<ClientStats>,

    /// Clients want to talk to admin database.
    admin: bool,

    /// Last server process stats we talked to.
    last_server_stats: Option<Arc<ServerStats>>,

    /// Connected to server
    connected_to_server: bool,

    /// Name of the server pool for this client (This comes from the database name in the connection string)
    pool_name: String,

    /// Postgres user for this client (This comes from the user in the connection string)
    username: String,

    /// Server startup and session parameters that we're going to track
    server_parameters: ServerParameters,

    /// Used to notify clients about an impending shutdown
    shutdown: Receiver<()>,

    /// Whether prepared statements are enabled for this client
    prepared_statements_enabled: bool,

    /// Mapping of client named prepared statement to rewritten parse messages
    prepared_statements: HashMap<String, (Arc<Parse>, u64)>,

    max_memory_usage: u64,

    /// Buffered extended protocol data
    extended_protocol_data_buffer: VecDeque<ExtendedProtocolData>,

    client_last_messages_in_tx: BytesMut,

    pooler_check_query_request_vec: Vec<u8>,

    created_at: Instant,
    virtual_pool_count: u16,
}

pub async fn client_entrypoint_too_many_clients_already(
    mut stream: TcpStream,
    client_server_map: ClientServerMap,
    shutdown: Receiver<()>,
    drain: Sender<i32>,
) -> Result<(), Error> {
    let addr = match stream.peer_addr() {
        Ok(addr) => addr,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to get peer address: {:?}",
                err
            )));
        }
    };

    match get_startup::<TcpStream>(&mut stream).await {
        Ok((ClientConnectionType::Tls, _)) => {
            let mut no = BytesMut::new();
            no.put_u8(b'N');
            write_all(&mut stream, no).await?
            // здесь может быть ошибка SSL is not enabled on the server,
            // вместо too many client, но это сделано намерянно, потому что мы
            // не сможем обработать столько клиентов еще и через SSL.
        }
        Ok((ClientConnectionType::Startup, _)) => (
            // pass
            ),
        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            let (read, write) = split(stream);
            // Continue with cancel query request.
            return match Client::cancel(read, write, addr, bytes, client_server_map, shutdown).await
            {
                Ok(mut client) => {
                    info!("Client {:?} issued a cancel query request", addr);
                    if !client.is_admin() {
                        let _ = drain.send(1).await;
                    }
                    let result = client.handle().await;
                    if !client.is_admin() {
                        let _ = drain.send(-1).await;
                        if result.is_err() {
                            client.stats.disconnect();
                        }
                    }
                    result
                }
                Err(err) => Err(err),
            };
        }
        Err(err) => return Err(err),
    }
    error_response_terminal(&mut stream, "sorry, too many clients already", "53300").await?;
    Ok(())
}

/// Client entrypoint.
#[allow(clippy::too_many_arguments)]
pub async fn client_entrypoint(
    mut stream: TcpStream,
    client_server_map: ClientServerMap,
    shutdown: Receiver<()>,
    drain: Sender<i32>,
    admin_only: bool,
    tls_acceptor: Option<tokio_native_tls::TlsAcceptor>,
    tls_rate_limiter: Option<RateLimiter>,
) -> Result<(), Error> {
    let log_client_connections = get_config().general.log_client_connections;

    // Figure out if the client wants TLS or not.
    let addr = match stream.peer_addr() {
        Ok(addr) => addr,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to get peer address: {:?}",
                err
            )));
        }
    };

    match get_startup::<TcpStream>(&mut stream).await {
        // Client requested a TLS connection.
        Ok((ClientConnectionType::Tls, _)) => {
            // TLS settings are configured, will setup TLS now.
            if tls_acceptor.is_some() {
                TLS_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
                let mut yes = BytesMut::new();
                yes.put_u8(b'S');
                write_all(&mut stream, yes).await?;

                if tls_rate_limiter.is_some() {
                    tls_rate_limiter.unwrap().wait().await;
                }

                // Negotiate TLS.
                match startup_tls(
                    stream,
                    client_server_map,
                    shutdown,
                    admin_only,
                    tls_acceptor.unwrap(),
                )
                .await
                {
                    Ok(mut client) => {
                        if log_client_connections {
                            info!("Client {:?} connected (TLS)", addr);
                        }

                        if !client.is_admin() {
                            let _ = drain.send(1).await;
                        }

                        let result = client.handle().await;

                        if !client.is_admin() {
                            let _ = drain.send(-1).await;

                            if result.is_err() {
                                client.stats.disconnect();
                            }
                        }

                        result
                    }
                    Err(err) => Err(err),
                }
            }
            // TLS is not configured, we cannot offer it.
            else {
                // Rejecting client request for TLS.
                PLAIN_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
                let mut no = BytesMut::new();
                no.put_u8(b'N');
                write_all(&mut stream, no).await?;

                // Attempting regular startup. Client can disconnect now
                // if they choose.
                match get_startup::<TcpStream>(&mut stream).await {
                    // Client accepted unencrypted connection.
                    Ok((ClientConnectionType::Startup, bytes)) => {
                        let (read, write) = split(stream);

                        // Continue with regular startup.
                        match Client::startup(
                            read,
                            write,
                            addr,
                            bytes,
                            client_server_map,
                            shutdown,
                            admin_only,
                            false,
                        )
                        .await
                        {
                            Ok(mut client) => {
                                if log_client_connections {
                                    info!("Client {:?} connected (plain)", addr);
                                }
                                if !client.is_admin() {
                                    let _ = drain.send(1).await;
                                }

                                let result = client.handle().await;

                                if !client.is_admin() {
                                    let _ = drain.send(-1).await;

                                    if result.is_err() {
                                        client.stats.disconnect();
                                    }
                                }

                                result
                            }
                            Err(err) => Err(err),
                        }
                    }

                    // Client probably disconnected rejecting our plain text connection.
                    Ok((ClientConnectionType::Tls, _))
                    | Ok((ClientConnectionType::CancelQuery, _)) => Err(Error::ProtocolSyncError(
                        "Bad postgres client (plain)".into(),
                    )),

                    Err(err) => Err(err),
                }
            }
        }

        // Client wants to use plain connection without encryption.
        Ok((ClientConnectionType::Startup, bytes)) => {
            PLAIN_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let (read, write) = split(stream);

            // Continue with regular startup.
            match Client::startup(
                read,
                write,
                addr,
                bytes,
                client_server_map,
                shutdown,
                admin_only,
                false,
            )
            .await
            {
                Ok(mut client) => {
                    if log_client_connections {
                        info!("Client {:?} connected (plain)", addr);
                    }
                    if !client.is_admin() {
                        let _ = drain.send(1).await;
                    }

                    let result = client.handle().await;

                    if !client.is_admin() {
                        let _ = drain.send(-1).await;

                        if result.is_err() {
                            client.stats.disconnect();
                        }
                    }

                    result
                }
                Err(err) => Err(err),
            }
        }

        // Client wants to cancel a query.
        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            CANCEL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let (read, write) = split(stream);

            // Continue with cancel query request.
            match Client::cancel(read, write, addr, bytes, client_server_map, shutdown).await {
                Ok(mut client) => {
                    info!("Client {:?} issued a cancel query request", addr);

                    if !client.is_admin() {
                        let _ = drain.send(1).await;
                    }

                    let result = client.handle().await;

                    if !client.is_admin() {
                        let _ = drain.send(-1).await;

                        if result.is_err() {
                            client.stats.disconnect();
                        }
                    }
                    result
                }

                Err(err) => Err(err),
            }
        }

        // Something failed, probably the socket.
        Err(err) => Err(err),
    }
}

/// Handle the first message the client sends.
async fn get_startup<S>(stream: &mut S) -> Result<(ClientConnectionType, BytesMut), Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin + tokio::io::AsyncWrite,
{
    // Get startup message length.
    let len = match stream.read_i32().await {
        Ok(len) => len,
        Err(_) => return Err(Error::ClientBadStartup),
    };

    // Get the rest of the message.
    let mut startup = vec![0u8; len as usize - 4];
    match stream.read_exact(&mut startup).await {
        Ok(_) => (),
        Err(_) => return Err(Error::ClientBadStartup),
    };

    let mut bytes = BytesMut::from(&startup[..]);
    let code = bytes.get_i32();

    match code {
        // Client is requesting SSL (TLS).
        SSL_REQUEST_CODE => Ok((ClientConnectionType::Tls, bytes)),

        // Client wants to use plain text, requesting regular startup.
        PROTOCOL_VERSION_NUMBER => Ok((ClientConnectionType::Startup, bytes)),

        // Client is requesting to cancel a running query (plain text connection).
        CANCEL_REQUEST_CODE => Ok((ClientConnectionType::CancelQuery, bytes)),

        REQUEST_GSSENCMODE_CODE => {
            // Rejecting client request for GSSENCMODE.
            let mut no = BytesMut::new();
            no.put_u8(b'G');
            write_all_flush(stream, &no).await?;
            Err(Error::AuthError("GSSENCMODE is unsupported".to_string()))
        }

        // Something else, probably something is wrong, and it's not our fault,
        // e.g. badly implemented Postgres client.
        _ => Err(Error::ProtocolSyncError(format!(
            "Unexpected startup code: {}",
            code
        ))),
    }
}

/// Handle TLS connection negotiation.
pub async fn startup_tls(
    stream: TcpStream,
    client_server_map: ClientServerMap,
    shutdown: Receiver<()>,
    admin_only: bool,
    tls_acceptor: tokio_native_tls::TlsAcceptor,
) -> Result<
    Client<
        ReadHalf<tokio_native_tls::TlsStream<TcpStream>>,
        WriteHalf<tokio_native_tls::TlsStream<TcpStream>>,
    >,
    Error,
> {
    // Negotiate TLS.
    let addr = match stream.peer_addr() {
        Ok(addr) => addr,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to get peer address: {:?}",
                err
            )));
        }
    };

    let mut stream = match tls_acceptor.accept(stream).await {
        Ok(stream) => stream,

        // TLS negotiation failed.
        Err(err) => {
            error!("TLS negotiation failed: {:?}", err);
            return Err(Error::TlsError);
        }
    };

    // TLS negotiation successful.
    // Continue with regular startup using encrypted connection.
    match get_startup::<tokio_native_tls::TlsStream<TcpStream>>(&mut stream).await {
        // Got good startup message, proceeding like normal except we
        // are encrypted now.
        Ok((ClientConnectionType::Startup, bytes)) => {
            let (read, write) = split(stream);

            Client::startup(
                read,
                write,
                addr,
                bytes,
                client_server_map,
                shutdown,
                admin_only,
                true,
            )
            .await
        }

        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            CANCEL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let (read, write) = split(stream);
            Client::cancel(read, write, addr, bytes, client_server_map, shutdown).await
        }

        // Bad Postgres client.
        Ok((ClientConnectionType::Tls, _)) => {
            Err(Error::ProtocolSyncError("Bad postgres client (tls)".into()))
        }

        Err(err) => Err(err),
    }
}

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    /// Handle Postgres client startup after TLS negotiation is complete
    /// or over plain text.
    #[allow(clippy::too_many_arguments)]
    pub async fn startup(
        mut read: S,
        mut write: T,
        addr: std::net::SocketAddr,
        bytes: BytesMut, // The rest of the startup message.
        client_server_map: ClientServerMap,
        shutdown: Receiver<()>,
        admin_only: bool,
        use_tls: bool,
    ) -> Result<Client<S, T>, Error> {
        let parameters = parse_startup(bytes.clone())?;

        // This parameter is mandatory by the protocol.
        let username = match parameters.get("user") {
            Some(user) => user,
            None => {
                return Err(Error::ClientError(
                    "Missing user parameter on client startup".into(),
                ))
            }
        };

        let pool_name = parameters.get("database").unwrap_or(username);

        let application_name = match parameters.get("application_name") {
            Some(application_name) => application_name,
            None => "pg_doorman",
        };

        let client_identifier = ClientIdentifier::new(
            application_name,
            username,
            pool_name,
            addr.to_string().as_str(),
        );

        let admin = ["pgdoorman", "pgbouncer"]
            .iter()
            .filter(|db| *db == pool_name)
            .count()
            == 1;

        // Kick any client that's not admin while we're in admin-only mode.
        if !admin && admin_only {
            error_response_terminal(
                &mut write,
                "is admin only mode: pooler is shut down now",
                "58006",
            )
            .await?;
            return Err(Error::ShuttingDown);
        }

        if !addr_in_hba(addr.ip()) {
            error_response_terminal(&mut write, "hba forbidden for this ip address", "28000")
                .await?;
            return Err(Error::HbaForbiddenError(format!(
                "hba forbidden client: {} from address: {:?}",
                client_identifier,
                addr.ip()
            )));
        }

        // Generate random backend ID and secret key
        let process_id: i32 = rand::random();
        let secret_key: i32 = rand::random();

        let mut prepared_statements_enabled = false;

        // Authenticate admin user.
        let (transaction_mode, mut server_parameters) = if admin {
            // Authenticate admin user with md5.
            let salt = md5_challenge(&mut write).await?;
            let password_response = read_password(&mut read).await?;
            let config = get_config();

            // Compare server and client hashes.
            let password_hash = md5_hash_password(
                &config.general.admin_username,
                &config.general.admin_password,
                &salt,
            );

            if password_hash != password_response {
                let error = Error::ClientGeneralError("Invalid password".into(), client_identifier);

                warn!("{}", error);
                wrong_password(&mut write, username).await?;

                return Err(error);
            }

            (false, generate_server_parameters_for_admin())
        }
        // Authenticate normal user.
        else {
            let virtual_pool_id = 0;
            let pool = match get_pool(pool_name, username, virtual_pool_id) {
                Some(pool) => pool,
                None => {
                    error_response(
                        &mut write,
                        &format!(
                            "No pool configured for database: {}, user: {}, virtual pool id: {}",
                            pool_name, username, virtual_pool_id
                        ),
                        "3D000",
                    )
                    .await?;

                    return Err(Error::ClientGeneralError(
                        "Invalid pool name".into(),
                        client_identifier,
                    ));
                }
            };
            let pool_password = pool.settings.user.password.clone();
            if pool_password.starts_with(SCRAM_SHA_256) {
                // scram auth.
                scram_start_challenge(&mut write).await?;
                let server_secret = match parse_server_secret(pool_password.as_str()) {
                    Ok(server_secret) => server_secret,
                    Err(err) => {
                        warn!("parse server secret for client {}: {}", username, err);
                        wrong_password(&mut write, username).await?;
                        return Err(err);
                    }
                };
                let first_message = read_password(&mut read).await?;
                let client_first_message =
                    match parse_client_first_message(String::from_utf8_lossy(&first_message)) {
                        Ok(client_first_message) => client_first_message,
                        Err(err) => {
                            warn!("parse first client message error: {}", err);
                            wrong_password(&mut write, username).await?;
                            return Err(err);
                        }
                    };
                let server_first_response = prepare_server_first_response(
                    client_first_message.nonce.as_str(),
                    client_first_message.client_first_bare.as_str(),
                    server_secret.salt_base64.as_str(),
                    server_secret.iteration,
                );
                scram_server_response(
                    &mut write,
                    SASL_CONTINUE,
                    server_first_response.server_first_bare.as_str(),
                )
                .await?;
                let final_message = read_password(&mut read).await?;
                let client_final_message =
                    match parse_client_final_message(String::from_utf8_lossy(&final_message)) {
                        Ok(client_final_message) => client_final_message,
                        Err(err) => {
                            warn!(
                                "parse final scram client {} message error: {}",
                                username, err
                            );
                            wrong_password(&mut write, username).await?;
                            return Err(err);
                        }
                    };
                let server_final_message = match prepare_server_final_message(
                    client_first_message,
                    client_final_message,
                    server_first_response,
                    server_secret.server_key,
                    server_secret.stored_key,
                ) {
                    Ok(server_final_message) => server_final_message,
                    Err(err) => {
                        warn!(
                            "parse final scram server message for {} error: {}",
                            username, err
                        );
                        wrong_password(&mut write, username).await?;
                        return Err(err);
                    }
                };
                scram_server_response(&mut write, SASL_FINAL, server_final_message.as_str())
                    .await?;
            } else if pool_password.starts_with(MD5_PASSWORD_PREFIX) {
                // md5 auth.
                let salt = md5_challenge(&mut write).await?;
                let password_response = read_password(&mut read).await?;
                let except_md5_hash =
                    md5_hash_second_pass(pool_password.strip_prefix("md5").unwrap(), &salt);
                if except_md5_hash != password_response {
                    error!("md5 auth error for: {}", pool.address);
                    wrong_password(&mut write, username).await?;
                    return Err(Error::AuthError(username.into()));
                }
            } else if pool_password.starts_with(JWT_PUB_KEY_PASSWORD_PREFIX) {
                // jwt.
                plain_password_challenge(&mut write).await?;
                let jwt_token_response = read_password(&mut read).await?;
                let jwt_token_with_nul = match str::from_utf8(&jwt_token_response) {
                    Ok(token) => token,
                    Err(_) => return Err(Error::AuthError(username.into())),
                };
                let jwt_token = match CStr::from_bytes_until_nul(jwt_token_with_nul.as_ref()) {
                    Ok(token) => token.to_str().unwrap().to_string(),
                    Err(_) => return Err(Error::AuthError(username.into())),
                };
                let jwt_user_name = match get_user_name_from_jwt(
                    pool_password
                        .strip_prefix(JWT_PUB_KEY_PASSWORD_PREFIX)
                        .unwrap()
                        .to_string(),
                    jwt_token,
                )
                .await
                {
                    Ok(u) => u,
                    Err(err) => {
                        wrong_password(&mut write, username).await?;
                        error!("unpack jwt for user {}: {:?}", username, err);
                        return Err(Error::AuthError(username.into()));
                    }
                };
                if !jwt_user_name.eq(username) {
                    wrong_password(&mut write, username).await?;
                    return Err(Error::AuthError(username.into()));
                }
            } else {
                warn!("unsupported password type");
                wrong_password(&mut write, username).await?;
                return Err(Error::AuthError(username.into()));
            }

            let transaction_mode = pool.settings.pool_mode == PoolMode::Transaction;
            prepared_statements_enabled =
                transaction_mode && pool.prepared_statement_cache.is_some();
            (transaction_mode, pool.server_parameters())
        };

        // Update the parameters to merge what the application sent and what's originally on the server
        server_parameters.set_from_hashmap(&parameters, false);

        auth_ok(&mut write).await?;
        write_all(&mut write, (&server_parameters).into()).await?;
        backend_key_data(&mut write, process_id, secret_key).await?;
        send_ready_for_query(&mut write).await?;

        let stats = Arc::new(ClientStats::new(
            process_id,
            application_name,
            username,
            pool_name,
            addr.to_string().as_str(),
            tokio::time::Instant::now(),
            use_tls,
        ));

        let config = get_config();
        Ok(Client {
            read: BufReader::new(read),
            write,
            addr,
            buffer: BytesMut::with_capacity(8196),
            response_message_queue_buffer: BytesMut::with_capacity(8196),
            cancel_mode: false,
            transaction_mode,
            process_id,
            secret_key,
            client_server_map,
            parameters: parameters.clone(),
            stats,
            admin,
            last_server_stats: None,
            connected_to_server: false,
            pool_name: pool_name.clone(),
            username: username.clone(),
            server_parameters,
            shutdown,
            prepared_statements_enabled,
            prepared_statements: HashMap::new(),
            virtual_pool_count: config.general.virtual_pool_count,
            client_last_messages_in_tx: BytesMut::with_capacity(8196),
            extended_protocol_data_buffer: VecDeque::new(),
            created_at: Instant::now(),
            max_memory_usage: config.general.max_memory_usage,
            pooler_check_query_request_vec: config
                .general
                .clone()
                .poller_check_query_request_bytes_vec(),
        })
    }

    /// Handle cancel request.
    pub async fn cancel(
        read: S,
        write: T,
        addr: std::net::SocketAddr,
        mut bytes: BytesMut, // The rest of the startup message.
        client_server_map: ClientServerMap,
        shutdown: Receiver<()>,
    ) -> Result<Client<S, T>, Error> {
        let process_id = bytes.get_i32();
        let secret_key = bytes.get_i32();
        Ok(Client {
            read: BufReader::new(read),
            write,
            addr,
            buffer: BytesMut::with_capacity(8196),
            response_message_queue_buffer: BytesMut::with_capacity(8196),
            cancel_mode: true,
            transaction_mode: false,
            process_id,
            secret_key,
            client_server_map,
            parameters: HashMap::new(),
            stats: Arc::new(ClientStats::default()),
            admin: false,
            last_server_stats: None,
            pool_name: String::from("undefined"),
            username: String::from("undefined"),
            server_parameters: ServerParameters::new(),
            shutdown,
            prepared_statements_enabled: false,
            prepared_statements: HashMap::new(),
            extended_protocol_data_buffer: VecDeque::new(),
            connected_to_server: false,
            client_last_messages_in_tx: BytesMut::with_capacity(8196),
            virtual_pool_count: get_config().general.virtual_pool_count,
            created_at: Instant::now(),
            max_memory_usage: 128 * 1024 * 1024,
            pooler_check_query_request_vec: Vec::new(),
        })
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
                            cancel_guard.push(*process_id);
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
        loop {
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
            tokio::select! {
                _ = self.shutdown.recv() => {
                    if !self.admin {
                        warn!("Drop client {:?} because shutdown and client completed transaction", self.addr);
                        error_response_terminal(&mut self.write, "pooler is shut down now", "58006").await?;
                        self.stats.disconnect();
                        return Ok(());
                    }
                },
                _ = tokio::task::yield_now() => {}
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
                    if self.pooler_check_query_request_vec.eq(&message.to_vec()) {
                        // This is the first message in the transaction, since we are responding with 'IZ',
                        // then we can not expect a server connection and immediately send answer and exit transaction loop.
                        write_all_flush(&mut self.write, &check_query_response()).await?;
                        continue;
                    }
                    if message.len() < 40 && message.len() > QUERY_DEALLOCATE.len() + 5 {
                        // Do not pass simple query with deallocate, as it will run on an unknown server.
                        let query = message[5..QUERY_DEALLOCATE.len() + 5].to_vec();
                        if QUERY_DEALLOCATE.eq(&query) {
                            write_all_flush(&mut self.write, &deallocate_response()).await?;
                            continue;
                        }
                    }
                }
                // Buffer extended protocol messages even if we do not have
                // a server connection yet. Hopefully, when we get the S message
                // we'll be able to allocate a connection. Also, clients do not expect
                // the server to respond to these messages so even if we were not able to
                // allocate a connection, we wouldn't be able to send back an error message
                // to the client so we buffer them and defer the decision to error out or not
                // to when we get the S message
                // Parse
                'P' => {
                    self.buffer_parse(message, current_pool)?;
                    continue;
                }

                // Bind
                'B' => {
                    self.buffer_bind(message).await?;
                    continue;
                }

                // Describe
                'D' => {
                    self.buffer_describe(message).await?;
                    continue;
                }

                'E' => {
                    self.extended_protocol_data_buffer
                        .push_back(ExtendedProtocolData::create_new_execute(message));
                    continue;
                }

                // Close (F)
                'C' => {
                    let close: Close = (&message).try_into()?;
                    self.extended_protocol_data_buffer
                        .push_back(ExtendedProtocolData::create_new_close(message, close));
                    continue;
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
                                    guard.retain(|&id| id != conn.get_process_id());
                                    conn.mark_bad("because was canceled", true);
                                    continue; // try to find another server.
                                }
                            }
                            break conn;
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
                                format!("could not get connection from the pool - {}", err)
                                    .as_str(),
                                "53300",
                            )
                            .await?;

                            error!(
                                "Could not get connection from pool: \
                        {{ \
                            pool_name: {:?}, \
                            username: {:?}, \
                            error: \"{:?}\" \
                        }}",
                                self.pool_name, self.username, err
                            );
                            continue;
                        }
                    };
                };
                let server = conn.deref_mut();
                // это отложенная очистка перед доступом к новому серверу.
                server.checkin_cleanup().await?;
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
                server.set_flush_wait_code(' ');

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

                            if !server.in_transaction() {
                                // Report transaction executed statistics.
                                self.stats.transaction();
                                server
                                    .stats
                                    .transaction(self.server_parameters.get_application_name());

                                // Release server back to the pool if we are in transaction mode.
                                // If we are in session mode, we keep the server until the client disconnects.
                                if self.transaction_mode && !server.in_copy_mode() {
                                    self.stats.idle_read();
                                    break;
                                }
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
                            self.buffer_parse(message, current_pool)?;
                        }

                        // Bind
                        'B' => {
                            self.buffer_bind(message).await?;
                        }

                        // Describe
                        // Command a client can issue to describe a previously prepared named statement.
                        'D' => {
                            self.buffer_describe(message).await?;
                        }

                        // Execute
                        // Execute a prepared statement prepared in `P` and bound in `B`.
                        'E' => {
                            self.extended_protocol_data_buffer
                                .push_back(ExtendedProtocolData::create_new_execute(message));
                        }

                        // Close
                        // Close the prepared statement.
                        'C' => {
                            let close: Close = (&message).try_into()?;

                            self.extended_protocol_data_buffer
                                .push_back(ExtendedProtocolData::create_new_close(message, close));
                        }

                        // Sync
                        // Frontend (client) is asking for the query result now.
                        'S' | 'H' => {
                            // Prepared statements can arrive like this
                            // 1. Without named describe
                            //      Client: Parse, with name, query and params
                            //              Sync
                            //      Server: ParseComplete
                            //              ReadyForQuery
                            // 3. Without named describe
                            //      Client: Parse, with name, query and params
                            //              Describe, with no name
                            //              Sync
                            //      Server: ParseComplete
                            //              ParameterDescription
                            //              RowDescription
                            //              ReadyForQuery
                            // 2. With named describe
                            //      Client: Parse, with name, query and params
                            //              Describe, with name
                            //              Sync
                            //      Server: ParseComplete
                            //              ParameterDescription
                            //              RowDescription
                            //              ReadyForQuery
                            // Iterate over our extended protocol data that we've buffered
                            let mut async_wait_code = ' ';
                            while let Some(protocol_data) =
                                self.extended_protocol_data_buffer.pop_front()
                            {
                                match protocol_data {
                                    ExtendedProtocolData::Parse { data, metadata } => {
                                        async_wait_code = '1';
                                        debug!("Have parse in extended buffer");
                                        let (parse, hash) = match metadata {
                                            Some(metadata) => metadata,
                                            None => {
                                                let first_char_in_name = *data.get(5).unwrap_or(&0);
                                                if first_char_in_name != 0 {
                                                    // This is a named prepared statement while prepared statements are disabled
                                                    // Server connection state will need to be cleared at checkin
                                                    server.mark_dirty();
                                                }
                                                // Not a prepared statement
                                                self.buffer.put(&data[..]);
                                                continue;
                                            }
                                        };

                                        // This is a prepared statement we already have on the checked out server
                                        if server.has_prepared_statement(&parse.name) {
                                            // We don't want to send the parse message to the server
                                            // Instead queue up a parse complete message to send to the client
                                            self.response_message_queue_buffer
                                                .put(parse_complete());
                                        } else {
                                            debug!(
                                                "Prepared statement `{}` not found in server cache",
                                                parse.name
                                            );

                                            // TODO: Consider adding the close logic that this function can send for eviction to the client buffer instead
                                            // In this case we don't want to send the parse message to the server since the client is sending it
                                            self.register_parse_to_server_cache(
                                                false,
                                                &hash,
                                                &parse,
                                                current_pool,
                                                server,
                                            )
                                            .await?;

                                            // Add parse message to buffer
                                            self.buffer.put(&data[..]);
                                        }
                                    }
                                    ExtendedProtocolData::Bind { data, metadata } => {
                                        async_wait_code = '2';
                                        // This is using a prepared statement
                                        if let Some(client_given_name) = metadata {
                                            self.ensure_prepared_statement_is_on_server(
                                                client_given_name,
                                                current_pool,
                                                server,
                                            )
                                            .await?;
                                        }

                                        self.buffer.put(&data[..]);
                                    }
                                    ExtendedProtocolData::Describe { data, metadata } => {
                                        async_wait_code = 'T';
                                        // This is using a prepared statement
                                        if let Some(client_given_name) = metadata {
                                            self.ensure_prepared_statement_is_on_server(
                                                client_given_name,
                                                current_pool,
                                                server,
                                            )
                                            .await?;
                                        }

                                        self.buffer.put(&data[..]);
                                    }
                                    ExtendedProtocolData::Execute { data } => {
                                        async_wait_code = 'C';
                                        self.buffer.put(&data[..])
                                    }
                                    ExtendedProtocolData::Close { data, close } => {
                                        // We don't send the close message to the server if prepared statements are enabled,
                                        // and it's a close with a prepared statement name provided
                                        if self.prepared_statements_enabled
                                            && close.is_prepared_statement()
                                            && !close.anonymous()
                                        {
                                            self.prepared_statements.remove(&close.name);
                                            // Queue up a close complete message to send to the client
                                            self.response_message_queue_buffer
                                                .put(close_complete());
                                        } else {
                                            self.buffer.put(&data[..]);
                                        }
                                    }
                                }
                            }

                            // Add the sync message
                            self.buffer.put(&message[..]);

                            if code == 'H' {
                                server.set_flush_wait_code(async_wait_code);
                                debug!("Client requested flush, going async");
                            } else {
                                server.set_flush_wait_code(' ')
                            }

                            self.send_and_receive_loop(None, server).await?;
                            self.stats.query();
                            server.stats.query(
                                query_start_at.elapsed().as_micros() as u64,
                                self.server_parameters.get_application_name(),
                            );

                            self.buffer.clear();

                            if !server.in_transaction() {
                                self.stats.transaction();
                                server
                                    .stats
                                    .transaction(self.server_parameters.get_application_name());

                                // Release server back to the pool if we are in transaction mode.
                                // If we are in session mode, we keep the server until the client disconnects.
                                if self.transaction_mode && !server.in_copy_mode() {
                                    if !self.response_message_queue_buffer.is_empty() {
                                        self.client_last_messages_in_tx
                                            .put(&self.response_message_queue_buffer[..]);
                                        self.client_last_messages_in_tx = set_messages_right_place(
                                            self.client_last_messages_in_tx.to_vec(),
                                        )?;
                                        self.response_message_queue_buffer.clear();
                                    }
                                    break;
                                }
                            }

                            // Send all queued messages to the client
                            // NOTE: it's possible we don't perfectly send things back in the same order as postgres would,
                            //       however clients should be able to handle this
                            if !self.response_message_queue_buffer.is_empty() {
                                if let Err(err) = write_all_flush(
                                    &mut self.write,
                                    &self.response_message_queue_buffer,
                                )
                                .await
                                {
                                    // We might be in some kind of error/in between protocol state
                                    server.mark_bad(
                                        format!("write to client {}: {:?}", self.addr, err)
                                            .as_str(),
                                        false,
                                    );
                                    return Err(err);
                                }

                                self.response_message_queue_buffer.clear();
                            }
                        }

                        // CopyData
                        'd' => {
                            self.buffer.put(&message[..]);

                            // Want to limit buffer size
                            if self.buffer.len() > 8196 {
                                // Forward the data to the server,
                                server.send_and_flush(&self.buffer).await?;
                                self.buffer.clear();
                            }
                        }

                        // CopyDone or CopyFail
                        // Copy is done, successfully or not.
                        'c' | 'f' => {
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
                                        false,
                                    );
                                    return Err(err);
                                }
                            };

                            if !server.in_transaction() {
                                self.stats.transaction();
                                server
                                    .stats
                                    .transaction(self.server_parameters.get_application_name());

                                // Release server back to the pool if we are in transaction mode.
                                // If we are in session mode, we keep the server until the client disconnects.
                                if self.transaction_mode {
                                    break;
                                }
                            }
                        }

                        // Some unexpected message. We either did not implement the protocol correctly
                        // or this is not a Postgres client we're talking to.
                        _ => {
                            error!("Unexpected code: {}", code);
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
                self.stats.idle_write(); // go to idle_read if success.
                write_all_flush(&mut self.write, &self.client_last_messages_in_tx).await?;
                self.client_last_messages_in_tx.clear();
            }
            self.connected_to_server = false;
            // change pool.
            if tx_counter % 10 == 0 && self.transaction_mode {
                pool = Some(self.get_pool(client_counter).await?);
            }
            tx_counter += 1;

            self.stats.idle_read();
            // capacity растет - вырастает rss у процесса.
            if self.client_last_messages_in_tx.capacity() > 4 * 8 * 1024 {
                self.client_last_messages_in_tx = BytesMut::with_capacity(8 * 1024);
            }
            if self.buffer.capacity() > 4 * 8 * 1024 {
                self.buffer = BytesMut::with_capacity(8 * 1024);
            }
            if self.response_message_queue_buffer.capacity() > 4 * 8 * 1024 {
                self.response_message_queue_buffer = BytesMut::with_capacity(8 * 1024);
            }
        }
    }
    /// Makes sure the checked out server has the prepared statement and sends it to the server if it doesn't
    async fn ensure_prepared_statement_is_on_server(
        &mut self,
        client_name: String,
        pool: &ConnectionPool,
        server: &mut Server,
    ) -> Result<(), Error> {
        match self.prepared_statements.get(&client_name) {
            Some((parse, hash)) => {
                debug!("Prepared statement `{}` found in cache", client_name);
                // In this case we want to send the parse message to the server
                // since pgcat is initiating the prepared statement on this specific server
                match self
                    .register_parse_to_server_cache(true, hash, parse, pool, server)
                    .await
                {
                    Ok(_) => (),
                    Err(err) => match err {
                        Error::PreparedStatementError => {
                            debug!("Removed {} from client cache", client_name);
                            self.prepared_statements.remove(&client_name);
                        }

                        _ => {
                            return Err(err);
                        }
                    },
                }
            }

            None => {
                return Err(Error::ClientError(format!(
                    "prepared statement `{}` not found",
                    client_name
                )))
            }
        };

        Ok(())
    }

    /// Register the parse to the server cache and send it to the server if requested (ie. requested by pgcat)
    ///
    /// Also updates the pool LRU that this parse was used recently
    async fn register_parse_to_server_cache(
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

    /// Register and rewrite the parse statement to the clients statement cache
    /// and also the pool's statement cache. Add it to extended protocol data.
    fn buffer_parse(&mut self, message: BytesMut, pool: &ConnectionPool) -> Result<(), Error> {
        // Avoid parsing if prepared statements not enabled
        if !self.prepared_statements_enabled {
            debug!("Anonymous parse message");
            self.extended_protocol_data_buffer
                .push_back(ExtendedProtocolData::create_new_parse(message, None));
            return Ok(());
        }

        let client_given_name = Parse::get_name(&message)?;
        let parse: Parse = (&message).try_into()?;

        // Compute the hash of the parse statement
        let hash = parse.get_hash();

        // Add the statement to the cache or check if we already have it
        let new_parse = match pool.register_parse_to_cache(hash, &parse) {
            Some(parse) => parse,
            None => {
                return Err(Error::ClientError(format!(
                    "Could not store Prepared statement `{}`",
                    client_given_name
                )))
            }
        };

        debug!(
            "Renamed prepared statement `{}` to `{}` and saved to cache",
            client_given_name, new_parse.name
        );

        self.prepared_statements
            .insert(client_given_name, (new_parse.clone(), hash));

        self.extended_protocol_data_buffer
            .push_back(ExtendedProtocolData::create_new_parse(
                new_parse.as_ref().try_into()?,
                Some((new_parse.clone(), hash)),
            ));

        Ok(())
    }

    /// Rewrite the Bind (F) message to use the prepared statement name
    /// saved in the client cache.
    async fn buffer_bind(&mut self, message: BytesMut) -> Result<(), Error> {
        // Avoid parsing if prepared statements not enabled
        if !self.prepared_statements_enabled {
            debug!("Anonymous bind message");
            self.extended_protocol_data_buffer
                .push_back(ExtendedProtocolData::create_new_bind(message, None));
            return Ok(());
        }

        let client_given_name = Bind::get_name(&message)?;

        match self.prepared_statements.get(&client_given_name) {
            Some((rewritten_parse, _)) => {
                let message = Bind::rename(message, &rewritten_parse.name)?;

                debug!(
                    "Rewrote bind `{}` to `{}`",
                    client_given_name, rewritten_parse.name
                );

                self.extended_protocol_data_buffer.push_back(
                    ExtendedProtocolData::create_new_bind(message, Some(client_given_name)),
                );

                Ok(())
            }
            None => {
                debug!(
                    "Got bind for unknown prepared statement {:?}",
                    client_given_name
                );

                error_response(
                    &mut self.write,
                    &format!(
                        "prepared statement \"{}\" does not exist",
                        client_given_name
                    ),
                    "58000",
                )
                .await?;

                Err(Error::ClientError(format!(
                    "Prepared statement `{}` doesn't exist",
                    client_given_name
                )))
            }
        }
    }

    /// Rewrite the Describe (F) message to use the prepared statement name
    /// saved in the client cache.
    async fn buffer_describe(&mut self, message: BytesMut) -> Result<(), Error> {
        // Avoid parsing if prepared statements not enabled
        if !self.prepared_statements_enabled {
            debug!("Anonymous describe message");
            self.extended_protocol_data_buffer
                .push_back(ExtendedProtocolData::create_new_describe(message, None));

            return Ok(());
        }

        let describe: Describe = (&message).try_into()?;
        if describe.target == 'P' {
            debug!("Portal describe message");
            self.extended_protocol_data_buffer
                .push_back(ExtendedProtocolData::create_new_describe(message, None));

            return Ok(());
        }

        let client_given_name = describe.statement_name.clone();

        match self.prepared_statements.get(&client_given_name) {
            Some((rewritten_parse, _)) => {
                let describe = describe.rename(&rewritten_parse.name);

                debug!(
                    "Rewrote describe `{}` to `{}`",
                    client_given_name, describe.statement_name
                );

                self.extended_protocol_data_buffer.push_back(
                    ExtendedProtocolData::create_new_describe(
                        describe.try_into()?,
                        Some(client_given_name),
                    ),
                );

                Ok(())
            }

            None => {
                debug!("Got describe for unknown prepared statement {:?}", describe);

                error_response(
                    &mut self.write,
                    &format!(
                        "prepared statement \"{}\" does not exist",
                        client_given_name
                    ),
                    "58000",
                )
                .await?;

                Err(Error::ClientError(format!(
                    "Prepared statement `{}` doesn't exist",
                    client_given_name
                )))
            }
        }
    }

    fn reset_buffered_state(&mut self) {
        self.buffer.clear();
        self.extended_protocol_data_buffer.clear();
        self.response_message_queue_buffer.clear();
    }

    fn get_virtual_pool_id(&mut self, client_counter: usize) -> u16 {
        let counter = client_counter as u64 + (self.created_at.elapsed().as_secs());
        (counter % self.virtual_pool_count as u64) as u16
    }

    /// Retrieve connection pool, if it exists.
    /// Return an error to the client otherwise.
    async fn get_pool(&mut self, client_counter: usize) -> Result<ConnectionPool, Error> {
        let virtual_pool_id = self.get_virtual_pool_id(client_counter);
        match get_pool(&self.pool_name, &self.username, virtual_pool_id) {
            Some(pool) => Ok(pool),
            None => {
                error_response(
                    &mut self.write,
                    &format!(
                        "No pool configured for database: {}, user: {}",
                        self.pool_name, self.username
                    ),
                    "3D000",
                )
                .await?;

                Err(Error::ClientError(format!(
                    "Invalid pool name {{ username: {}, pool_name: {}, application_name: {}, virtual pool id: {} }}",
                    self.pool_name,
                    self.username,
                    self.server_parameters.get_application_name(),
                    virtual_pool_id
                )))
            }
        }
    }

    /// Release the server from the client: it can't cancel its queries anymore.
    pub fn release(&self) {
        let mut guard = self.client_server_map.lock();
        guard.remove(&(self.process_id, self.secret_key));
    }

    async fn send_and_receive_loop(
        &mut self,
        message: Option<&BytesMut>,
        server: &mut Server,
    ) -> Result<(), Error> {
        let message = message.unwrap_or(&self.buffer);
        server.send_and_flush(message).await?;
        // Read all data the server has to offer, which can be multiple messages
        // buffered in 8196 bytes chunks.
        loop {
            self.stats.active_idle();
            let mut response = match server
                .recv(&mut self.write, Some(&mut self.server_parameters))
                .await
            {
                Ok(msg) => msg,
                Err(err) => {
                    server.wait_available().await;
                    server.mark_bad(
                        format!("loop with client {}: {:?}", self.addr, err).as_str(),
                        true,
                    );
                    return Err(err);
                }
            };
            // Fast release server back to the pool (only in transaction pool mode).
            if !server.is_data_available()
                && !server.in_transaction()
                && !server.in_copy_mode()
                && self.transaction_mode
                && !server.is_async()
            {
                self.client_last_messages_in_tx.put(&response[..]);
                break;
            }

            if !self.response_message_queue_buffer.is_empty() {
                response.put(&self.response_message_queue_buffer[..]);
                response = set_messages_right_place(response.to_vec())?;
                self.response_message_queue_buffer.clear();
            }

            self.stats.active_write();
            match write_all_flush(&mut self.write, &response).await {
                Ok(_) => self.stats.active_idle(),
                Err(err_write) => {
                    server.wait_available().await;
                    server.mark_bad(
                        format!("flush to client {} {:?}", self.addr, err_write).as_str(),
                        true,
                    );
                    return Err(err_write);
                }
            };

            if !server.is_data_available() {
                break;
            }
        }

        Ok(())
    }
    async fn process_error(&mut self, err: Error) -> Result<(), Error> {
        match err {
            Error::MaxMessageSize => {
                error_response(
                    &mut self.write,
                    format!("could not read full message - {}", err).as_str(),
                    "53200",
                )
                .await?;
                Err(err)
            }
            Error::CurrentMemoryUsage => {
                error_response(
                    &mut self.write,
                    format!("could not read message, temporary out of memory - {}", err).as_str(),
                    "53200",
                )
                .await?;
                Err(err)
            }
            _ => Err(err),
        }
    }
}

impl<S, T> Drop for Client<S, T> {
    fn drop(&mut self) {
        let mut guard = self.client_server_map.lock();
        guard.remove(&(self.process_id, self.secret_key));

        // Dirty shutdown
        // TODO: refactor, this is not the best way to handle state management.

        if self.connected_to_server && self.last_server_stats.is_some() {
            self.last_server_stats.as_ref().unwrap().idle(0);
        }
    }
}
