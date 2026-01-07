use bytes::{BufMut, BytesMut};
use log::{error, info};
use std::sync::atomic::Ordering;
use tokio::io::split;
use tokio::net::TcpStream;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::Sender;

use crate::config::get_config;
use crate::errors::Error;
use crate::messages::config_socket::configure_tcp_socket_for_cancel;
use crate::messages::{error_response_terminal, write_all};
use crate::pool::ClientServerMap;
use crate::stats::{CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER};
use crate::utils::rate_limit::RateLimiter;

use super::core::Client;
use super::startup::{get_startup, startup_tls, ClientConnectionType};

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
                "Failed to get peer address: {err:?}"
            )));
        }
    };

    match get_startup::<TcpStream>(&mut stream).await {
        Ok((ClientConnectionType::Tls, _)) => {
            let mut no = BytesMut::new();
            no.put_u8(b'N');
            write_all(&mut stream, no).await?;
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
            return match Client::cancel(read, write, addr, bytes, client_server_map, shutdown).await
            {
                Ok(mut client) => {
                    info!("Client {addr:?} issued a cancel query request");
                    if !client.is_admin() {
                        let _ = drain.send(1).await;
                    }
                    let result = client.handle().await;
                    if !client.is_admin() {
                        let _ = drain.send(-1).await;
                        if result.is_err() {
                            client.disconnect_stats();
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
                let mut yes = BytesMut::new();
                yes.put_u8(b'S');
                write_all(&mut stream, yes).await?;

                if let Some(tls_rate_limiter) = tls_rate_limiter {
                    tls_rate_limiter.wait().await;
                }

                // Negotiate TLS.
                match startup_tls(
                    stream,
                    client_server_map,
                    shutdown,
                    admin_only,
                    tls_acceptor,
                )
                .await
                {
                    Ok(mut client) => {
                        if log_client_connections {
                            info!("Client {addr:?} connected (TLS)");
                        }

                        if !client.is_admin() {
                            let _ = drain.send(1).await;
                        }

                        let result = client.handle().await;

                        if !client.is_admin() {
                            let _ = drain.send(-1).await;

                            if result.is_err() {
                                client.disconnect_stats();
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
                                    info!("Client {addr:?} connected (plain)");
                                }
                                if !client.is_admin() {
                                    let _ = drain.send(1).await;
                                }

                                let result = client.handle().await;

                                if !client.is_admin() {
                                    let _ = drain.send(-1).await;

                                    if result.is_err() {
                                        client.disconnect_stats();
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
                        info!("Client {addr:?} connected (plain)");
                    }
                    if !client.is_admin() {
                        let _ = drain.send(1).await;
                    }

                    let result = client.handle().await;

                    if !client.is_admin() {
                        let _ = drain.send(-1).await;

                        if result.is_err() {
                            client.disconnect_stats();
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
            // Important: without configuring the TCP socket for cancel requests,
            // libpq-based clients (e.g., psycopg2) may emit a noisy stderr warning on cancellation
            // such as:
            // "query cancellation failed: cancellation failed: connection to server ..."
            // We set the appropriate socket options to avoid this spurious message.
            configure_tcp_socket_for_cancel(&stream);
            let (read, write) = split(stream);

            // Continue with cancel query request.
            match Client::cancel(read, write, addr, bytes, client_server_map, shutdown).await {
                Ok(mut client) => {
                    info!("Cancel request received from {addr:?}; forwarding to the backend");

                    if !client.is_admin() {
                        let _ = drain.send(1).await;
                    }

                    let result = client.handle().await;

                    if !client.is_admin() {
                        let _ = drain.send(-1).await;

                        if result.is_err() {
                            client.disconnect_stats();
                        }
                    }
                    result
                }

                Err(err) => Err(err),
            }
        }

        // Something failed, probably the socket.
        Err(err) => {
            error!("{err:?}");
            Err(err)
        }
    }
}
