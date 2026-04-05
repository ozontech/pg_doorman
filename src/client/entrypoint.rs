use log::{error, info, warn};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::sync::atomic::Ordering;
use tokio::io::split;
use tokio::net::{TcpStream, UnixStream};

use crate::config::get_config;
use crate::errors::Error;
use crate::messages::config_socket::configure_tcp_socket_for_cancel;
use crate::messages::{error_response_terminal, write_all_flush};
use crate::pool::ClientServerMap;
use crate::stats::{CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER};
use crate::utils::rate_limit::RateLimiter;

use super::core::Client;
use super::startup::{get_startup, startup_tls, ClientConnectionType};

/// Identity info returned from client_entrypoint for disconnect logging.
pub struct ClientSessionInfo {
    pub username: String,
    pub pool_name: String,
    pub connection_id: u64,
}

pub async fn client_entrypoint_too_many_clients_already(
    mut stream: TcpStream,
    client_server_map: ClientServerMap,
) -> Result<(), Error> {
    let addr = match stream.peer_addr() {
        Ok(addr) => addr,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to get peer address: {err:?}"
            )));
        }
    };

    match get_startup::<TcpStream>(&mut stream).await {
        Ok((ClientConnectionType::Tls, _)) => {
            write_all_flush(&mut stream, b"N").await?;
            // здесь может быть ошибка SSL is not enabled on the server,
            // вместо too many client, но это сделано намерянно, потому что мы
            // не сможем обработать столько клиентов еще и через SSL.
        }
        Ok((ClientConnectionType::Startup, _)) => (
            // pass
        ),
        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            // Important: without configuring the TCP socket for cancel requests,
            // libpq-based clients (e.g., psycopg2) may emit a noisy stderr warning on cancellation
            // such as:
            // "query cancellation failed: cancellation failed: connection to server ..."
            // We set the appropriate socket options to avoid this spurious message.
            configure_tcp_socket_for_cancel(&stream);
            let (read, write) = split(stream);
            // Continue with cancel query request.
            return match Client::cancel(read, write, addr, bytes, client_server_map).await {
                Ok(mut client) => {
                    info!("Cancel request from {addr}");
                    let result = client.handle().await;
                    if !client.is_admin() && result.is_err() {
                        client.disconnect_stats();
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

/// Client entrypoint. Returns session identity on success for disconnect logging.
pub async fn client_entrypoint(
    mut stream: TcpStream,
    client_server_map: ClientServerMap,
    admin_only: bool,
    tls_acceptor: Option<tokio_native_tls::TlsAcceptor>,
    tls_rate_limiter: Option<RateLimiter>,
    connection_id: u64,
) -> Result<Option<ClientSessionInfo>, Error> {
    let config = get_config();
    let log_client_connections = config.general.log_client_connections;
    let tls_mode = config.general.tls_mode.clone();

    // Figure out if the client wants TLS or not.
    let addr = match stream.peer_addr() {
        Ok(addr) => addr,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to get peer address: {err:?}"
            )));
        }
    };

    match get_startup::<TcpStream>(&mut stream).await {
        // Client requested a TLS connection.
        Ok((ClientConnectionType::Tls, _)) => {
            // TLS settings are configured, will setup TLS now.
            if let Some(tls_acceptor) = tls_acceptor {
                TLS_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
                write_all_flush(&mut stream, b"S").await?;

                if let Some(tls_rate_limiter) = tls_rate_limiter {
                    tls_rate_limiter.wait().await;
                }

                // Negotiate TLS.
                match startup_tls(
                    stream,
                    client_server_map,
                    admin_only,
                    tls_acceptor,
                    connection_id,
                )
                .await
                {
                    Ok(mut client) => {
                        if log_client_connections {
                            info!(
                                "[{}@{} #c{}] client connected from {addr} (TLS)",
                                client.username, client.pool_name, client.connection_id
                            );
                        }
                        let session_info = ClientSessionInfo {
                            username: client.username.clone(),
                            pool_name: client.pool_name.clone(),
                            connection_id: client.connection_id,
                        };
                        let result = client.handle().await;
                        if !client.is_admin() && result.is_err() {
                            warn!(
                                "[{}@{} #c{}] client {} disconnected with error: {}",
                                client.username,
                                client.pool_name,
                                client.connection_id,
                                addr,
                                result.as_ref().unwrap_err()
                            );
                            client.disconnect_stats();
                        }
                        result.map(|_| Some(session_info))
                    }
                    Err(err) => Err(err),
                }
            }
            // TLS is not configured, we cannot offer it.
            else {
                // Rejecting client request for TLS.
                PLAIN_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
                write_all_flush(&mut stream, b"N").await?;

                // Attempting regular startup. Client can disconnect now
                // if they choose.
                match get_startup::<TcpStream>(&mut stream).await {
                    // Client accepted unencrypted connection.
                    Ok((ClientConnectionType::Startup, bytes)) => {
                        #[cfg(unix)]
                        let raw_fd = Some(stream.as_raw_fd());
                        let (read, write) = split(stream);

                        // Continue with regular startup.
                        match Client::startup(
                            read,
                            write,
                            addr,
                            bytes,
                            client_server_map,
                            admin_only,
                            false,
                            connection_id,
                            false,
                            #[cfg(unix)]
                            raw_fd,
                            #[cfg(all(unix, feature = "tls-migration"))]
                            None, // no SSL for plain TCP
                        )
                        .await
                        {
                            Ok(mut client) => {
                                if log_client_connections {
                                    info!(
                                        "[{}@{} #c{}] client connected from {addr} (plain)",
                                        client.username, client.pool_name, client.connection_id
                                    );
                                }
                                let session_info = ClientSessionInfo {
                                    username: client.username.clone(),
                                    pool_name: client.pool_name.clone(),
                                    connection_id: client.connection_id,
                                };
                                let result = client.handle().await;
                                if !client.is_admin() && result.is_err() {
                                    client.disconnect_stats();
                                }
                                result.map(|_| Some(session_info))
                            }
                            Err(err) => Err(err),
                        }
                    }

                    // Client probably disconnected rejecting our plain text connection.
                    Ok((ClientConnectionType::Tls, _))
                    | Ok((ClientConnectionType::CancelQuery, _)) => Err(Error::ProtocolSyncError(
                        "Unexpected protocol message during plain-text startup negotiation".into(),
                    )),

                    Err(err) => Err(err),
                }
            }
        }

        // Client wants to use plain connection without encryption.
        Ok((ClientConnectionType::Startup, bytes)) => {
            if tls_mode.is_some() && config.general.only_ssl_connections() {
                error_response_terminal(
                    &mut stream,
                    "Connection without SSL is not allowed by tls_mode.",
                    "28000",
                )
                .await?;
                return Err(Error::ProtocolSyncError("ssl is required".to_string()));
            }
            PLAIN_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            #[cfg(unix)]
            let raw_fd = Some(stream.as_raw_fd());
            let (read, write) = split(stream);

            // Continue with regular startup.
            match Client::startup(
                read,
                write,
                addr,
                bytes,
                client_server_map,
                admin_only,
                false,
                connection_id,
                false,
                #[cfg(unix)]
                raw_fd,
                #[cfg(all(unix, feature = "tls-migration"))]
                None, // no SSL for plain TCP
            )
            .await
            {
                Ok(mut client) => {
                    if log_client_connections {
                        info!(
                            "[{}@{} #c{}] client connected from {addr} (plain)",
                            client.username, client.pool_name, client.connection_id
                        );
                    }
                    let session_info = ClientSessionInfo {
                        username: client.username.clone(),
                        pool_name: client.pool_name.clone(),
                        connection_id: client.connection_id,
                    };
                    let result = client.handle().await;
                    if !client.is_admin() && result.is_err() {
                        client.disconnect_stats();
                    }
                    result.map(|_| Some(session_info))
                }
                Err(err) => Err(err),
            }
        }

        // Client wants to cancel a query.
        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            CANCEL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            // Important: without configuring the TCP socket for cancel requests,
            // libpq-based clients (e.g., psycopg2) may emit a noisy stderr warning on cancellation
            // such as:
            // "query cancellation failed: cancellation failed: connection to server ..."
            // We set the appropriate socket options to avoid this spurious message.
            configure_tcp_socket_for_cancel(&stream);
            let (read, write) = split(stream);

            // Continue with cancel query request.
            match Client::cancel(read, write, addr, bytes, client_server_map).await {
                Ok(mut client) => {
                    info!("Cancel request from {addr}");
                    let result = client.handle().await;
                    if !client.is_admin() && result.is_err() {
                        client.disconnect_stats();
                    }
                    result.map(|_| None)
                }

                Err(err) => Err(err),
            }
        }

        // Something failed, probably the socket.
        Err(err) => {
            error!("#c{connection_id} client {addr} startup failed: {err}");
            Err(err)
        }
    }
}

/// Unix socket client entrypoint. No TLS, no peer_addr — uses fake 127.0.0.1:0.
pub async fn client_entrypoint_unix(
    mut stream: UnixStream,
    client_server_map: ClientServerMap,
    admin_only: bool,
    connection_id: u64,
) -> Result<Option<ClientSessionInfo>, Error> {
    let fake_addr: std::net::SocketAddr = ([127, 0, 0, 1], 0).into();
    let config = get_config();
    let log_client_connections = config.general.log_client_connections;

    match get_startup::<UnixStream>(&mut stream).await {
        Ok((ClientConnectionType::Startup, bytes)) => {
            PLAIN_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let raw_fd = Some(stream.as_raw_fd());
            let (read, write) = split(stream);

            match Client::startup(
                read,
                write,
                fake_addr,
                bytes,
                client_server_map,
                admin_only,
                false,
                connection_id,
                true,
                #[cfg(unix)]
                raw_fd,
                #[cfg(all(unix, feature = "tls-migration"))]
                None, // no SSL on Unix socket
            )
            .await
            {
                Ok(mut client) => {
                    if log_client_connections {
                        info!(
                            "[{}@{} #c{}] client connected via unix socket",
                            client.username, client.pool_name, client.connection_id
                        );
                    }
                    let session_info = ClientSessionInfo {
                        username: client.username.clone(),
                        pool_name: client.pool_name.clone(),
                        connection_id: client.connection_id,
                    };
                    let result = client.handle().await;
                    if !client.is_admin() && result.is_err() {
                        client.disconnect_stats();
                    }
                    result.map(|_| Some(session_info))
                }
                Err(err) => Err(err),
            }
        }

        Ok((ClientConnectionType::Tls, _)) => {
            error_response_terminal(
                &mut stream,
                "TLS is not supported on Unix socket connections",
                "08P01",
            )
            .await?;
            Err(Error::ProtocolSyncError(
                "TLS requested on Unix socket".into(),
            ))
        }

        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            CANCEL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let (read, write) = split(stream);

            match Client::cancel(read, write, fake_addr, bytes, client_server_map).await {
                Ok(mut client) => {
                    info!("Cancel request via unix socket");
                    let result = client.handle().await;
                    if !client.is_admin() && result.is_err() {
                        client.disconnect_stats();
                    }
                    result.map(|_| None)
                }
                Err(err) => Err(err),
            }
        }

        Err(err) => {
            error!("#c{connection_id} unix client startup failed: {err}");
            Err(err)
        }
    }
}
