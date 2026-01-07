use bytes::{Buf, BufMut, BytesMut};
use log::error;
use std::sync::atomic::Ordering;
use tokio::io::{split, AsyncReadExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::broadcast::Receiver;

use crate::errors::Error;
use crate::messages::constants::*;
use crate::messages::write_all_flush;
use crate::pool::ClientServerMap;
use crate::stats::CANCEL_CONNECTION_COUNTER;

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
            "Unexpected startup code: {code}"
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
                shutdown,
                admin_only,
                true,
            )
            .await
        }

        Ok((ClientConnectionType::CancelQuery, bytes)) => {
            CANCEL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
            let (read, write) = split(stream);
            // Continue with cancel query request.
            Client::cancel(read, write, addr, bytes, client_server_map, shutdown).await
        }

        Ok((ClientConnectionType::Tls, _)) => {
            Err(Error::ProtocolSyncError("Bad postgres client (tls)".into()))
        }

        Err(err) => Err(err),
    }
}
