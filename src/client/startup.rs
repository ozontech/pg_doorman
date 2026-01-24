use ahash::AHashMap;
use bytes::{Buf, BufMut, BytesMut};
use log::error;
use std::ffi::CStr;
use std::str;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{split, AsyncReadExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;

use crate::auth::authenticate;
use crate::auth::hba::CheckResult;
use crate::auth::talos::{extract_talos_token, talos_role_to_string};
use crate::config::{check_hba, get_config};
use crate::errors::{ClientIdentifier, Error};
use crate::messages::constants::*;
use crate::messages::{
    error_response_terminal, parse_startup, plain_password_challenge, read_password,
    ready_for_query, write_all_flush,
};
use crate::pool::ClientServerMap;
use crate::server::ServerParameters;
use crate::stats::{ClientStats, CANCEL_CONNECTION_COUNTER};

use super::buffer_pool::PooledBuffer;
use super::core::Client;

/// Type of connection received from client.
pub(crate) enum ClientConnectionType {
    Startup,
    Tls,
    CancelQuery,
}

/// Handle the first message the client sends.
pub(crate) async fn get_startup<S>(
    stream: &mut S,
) -> Result<(ClientConnectionType, BytesMut), Error>
where
    S: tokio::io::AsyncRead + std::marker::Unpin + tokio::io::AsyncWrite,
{
    // Get startup message length.
    let len = match stream.read_i32().await {
        Ok(len) => len,
        Err(_) => return Err(Error::ClientBadStartup),
    };

    // Validate message length: minimum is 8 bytes (4 for length field + 4 for protocol code).
    // Also reject negative or excessively large lengths to prevent overflow/DoS.
    if !(8..=8 * 1024).contains(&len) {
        return Err(Error::ClientBadStartup);
    }

    // Get the rest of the message.
    let mut startup = vec![0u8; (len - 4) as usize];
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
            "Unexpected startup code: {code}"
        ))),
    }
}

/// Handle TLS connection negotiation.
pub async fn startup_tls(
    stream: TcpStream,
    client_server_map: ClientServerMap,
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
                "Failed to get peer address: {err:?}"
            )));
        }
    };

    let mut stream = match tls_acceptor.accept(stream).await {
        Ok(stream) => stream,

        // TLS negotiation failed.
        Err(err) => {
            error!("TLS negotiation failed: {err:?}");
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
                admin_only,
                true,
            )
            .await
        }

        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            CANCEL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let (read, write) = split(stream);
            // Continue with cancel query request.
            Client::cancel(read, write, addr, bytes, client_server_map).await
        }

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
    /// Handle Postgres client startup after TLS negotiation is complete
    /// or over plain text.
    #[allow(clippy::too_many_arguments)]
    pub async fn startup(
        mut read: S,
        mut write: T,
        addr: std::net::SocketAddr,
        bytes: BytesMut, // The rest of the startup message.
        client_server_map: ClientServerMap,
        admin_only: bool,
        use_tls: bool,
    ) -> Result<Client<S, T>, Error> {
        let parameters = parse_startup(bytes)?;

        // This parameter is mandatory by the protocol.
        let username_from_parameters = match parameters.get("user") {
            Some(user) => user,
            None => {
                return Err(Error::ClientError(
                    "Missing 'user' parameter in connection string. Please specify a username in your connection string.".into(),
                ))
            }
        };

        let pool_name = parameters
            .get("database")
            .unwrap_or(username_from_parameters)
            .to_string();

        let application_name = match parameters.get("application_name") {
            Some(application_name) => application_name,
            None => "pg_doorman",
        };

        let mut client_identifier = ClientIdentifier::new(
            application_name,
            username_from_parameters,
            &pool_name,
            addr.to_string().as_str(),
        );
        client_identifier.hba_md5 = check_hba(
            addr.ip(),
            use_tls,
            "md5",
            username_from_parameters,
            &pool_name,
        );
        client_identifier.hba_scram = check_hba(
            addr.ip(),
            use_tls,
            "scram-sha-256",
            username_from_parameters,
            &pool_name,
        );
        {
            // If md5 or scram is allowed, we can try to authenticate with Talos.
            let hba_ok = client_identifier.hba_md5 == CheckResult::Allow
                || client_identifier.hba_scram == CheckResult::Allow;
            if username_from_parameters == TALOS_USERNAME && hba_ok {
                plain_password_challenge(&mut write).await?;
                let talos_token_response = read_password(&mut read).await?;
                let talos_token_with_nul = match str::from_utf8(&talos_token_response) {
                    Ok(token) => token,
                    Err(_) => {
                        error_response_terminal(
                            &mut write,
                            "Invalid Talos token format. Token must be valid UTF-8 text.",
                            "3D000",
                        )
                        .await?;
                        return Err(Error::AuthError(format!(
                            "Failed to parse Talos token as UTF-8 for user: {TALOS_USERNAME}"
                        )));
                    }
                };
                let talos_token = match CStr::from_bytes_until_nul(talos_token_with_nul.as_ref()) {
                    Ok(token) => token.to_str().unwrap().to_string(),
                    Err(_) => {
                        error_response_terminal(
                            &mut write,
                            "Invalid Talos token format. Token must be a null-terminated string.",
                            "3D000",
                        )
                        .await?;
                        return Err(Error::AuthError(format!(
                            "Failed to convert Talos token to string for user: {TALOS_USERNAME}. Token must be null-terminated."
                        )));
                    }
                };
                let talos_databases = get_config().talos.databases;
                let token = match extract_talos_token(talos_token, talos_databases).await {
                    Ok(token) => token,
                    Err(err) => {
                        error_response_terminal(
                            &mut write,
                            format!("Invalid Talos token: {err:?}").as_str(),
                            "3D000",
                        )
                        .await?;
                        return Err(Error::AuthError(format!("Invalid Talos token: {err:?}")));
                    }
                };
                client_identifier.application_name = token.client_id;
                client_identifier.username = talos_role_to_string(token.role);
                client_identifier.is_talos = true;
            }
        }

        let admin = ["pgdoorman", "pgbouncer"]
            .iter()
            .filter(|db| **db == pool_name)
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

        // Final HBA decision: if neither md5 nor scram is explicitly allowed or trusted,
        // the connection is not permitted by HBA. `Deny` indicates explicit `reject` rule,
        // while `NotMatched` means no rule matched.
        let hba_ok_final = matches!(
            client_identifier.hba_scram,
            CheckResult::Allow | CheckResult::Trust
        ) || matches!(
            client_identifier.hba_md5,
            CheckResult::Allow | CheckResult::Trust
        );
        if !hba_ok_final {
            error_response_terminal(
                &mut write,
                format!("Connection from IP address {} to {}@{} (TLS: {}) is not permitted by HBA configuration. Please contact your database administrator.",
                        addr.ip(), username_from_parameters, pool_name, use_tls).as_str(),
                "28000"
            )
                .await?;
            return Err(Error::HbaForbiddenError(format!(
                "Connection not permitted by HBA configuration for client: {} from address: {:?}",
                client_identifier,
                addr.ip()
            )));
        }

        // Generate random backend ID and secret key
        let process_id: i32 = rand::random();
        let secret_key: i32 = rand::random();

        // Authenticate user
        let (transaction_mode, mut server_parameters, prepared_statements_enabled) = authenticate(
            &mut read,
            &mut write,
            admin,
            &client_identifier,
            &pool_name,
            username_from_parameters,
        )
        .await?;

        // Update the parameters to merge what the application sent and what's originally on the server
        server_parameters.set_from_hashmap(&parameters, false);
        let mut buf = BytesMut::new();
        {
            let mut auth_ok = BytesMut::with_capacity(9);
            auth_ok.put_u8(b'R');
            auth_ok.put_i32(8);
            auth_ok.put_i32(0);
            buf.put(auth_ok);
            let server_params_buf: BytesMut = (&server_parameters).into();
            buf.put(server_params_buf);
            let mut key_data = BytesMut::from(&b"K"[..]);
            key_data.put_i32(12);
            key_data.put_i32(process_id);
            key_data.put_i32(secret_key);
            buf.put(key_data);
            buf.put(ready_for_query(false));
        }
        write_all_flush(&mut write, &buf).await?;

        let stats = Arc::new(ClientStats::new(
            process_id,
            client_identifier.application_name.as_str(),
            client_identifier.username.as_str(),
            &pool_name,
            addr.to_string().as_str(),
            crate::utils::clock::recent(),
            use_tls,
        ));

        let config = get_config();
        Ok(Client {
            read: BufReader::new(read),
            write,
            addr,
            buffer: PooledBuffer::new(),
            pending_close_complete: 0,
            skipped_parses: Vec::new(),
            cancel_mode: false,
            transaction_mode,
            process_id,
            secret_key,
            client_server_map,
            stats,
            admin,
            last_server_stats: None,
            connected_to_server: false,
            pool_name,
            username: std::mem::take(&mut client_identifier.username),
            server_parameters,
            prepared_statements_enabled,
            async_client: false,
            prepared_statements: AHashMap::new(),
            last_anonymous_prepared_hash: None,
            client_last_messages_in_tx: PooledBuffer::new(),
            max_memory_usage: config.general.max_memory_usage.as_bytes(),
            pooler_check_query_request_vec: config.general.poller_check_query_request_bytes_vec(),
        })
    }

    /// Handle cancel request.
    pub async fn cancel(
        read: S,
        write: T,
        addr: std::net::SocketAddr,
        mut bytes: BytesMut, // The rest of the startup message.
        client_server_map: ClientServerMap,
    ) -> Result<Client<S, T>, Error> {
        let process_id = bytes.get_i32();
        let secret_key = bytes.get_i32();
        Ok(Client {
            read: BufReader::new(read),
            write,
            addr,
            buffer: PooledBuffer::new(),
            pending_close_complete: 0,
            skipped_parses: Vec::new(),
            cancel_mode: true,
            transaction_mode: false,
            process_id,
            secret_key,
            client_server_map,
            stats: Arc::new(ClientStats::default()),
            admin: false,
            last_server_stats: None,
            pool_name: String::from("undefined"),
            username: String::from("undefined"),
            server_parameters: ServerParameters::new(),
            prepared_statements_enabled: false,
            async_client: false,
            prepared_statements: AHashMap::new(),
            last_anonymous_prepared_hash: None,
            connected_to_server: false,
            client_last_messages_in_tx: PooledBuffer::new(),
            max_memory_usage: 128 * 1024 * 1024,
            pooler_check_query_request_vec: Vec::new(),
        })
    }
}
