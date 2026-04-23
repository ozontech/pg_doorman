use bytes::{BufMut, BytesMut};
use log::warn;

use crate::config::tls::{ServerTlsConfig, ServerTlsMode};
use crate::errors::Error;
use crate::messages::constants::CANCEL_REQUEST_CODE;
use crate::messages::write_all_flush;

use super::stream::{create_tcp_stream_inner, create_unix_stream_inner};

/// Issue a query cancellation request to the server.
/// Uses a separate connection that's not part of the connection pool.
/// When the original connection used TLS, the cancel connection also uses TLS.
pub(crate) async fn cancel(
    host: &str,
    port: u16,
    process_id: i32,
    secret_key: i32,
    server_tls: &ServerTlsConfig,
    connected_with_tls: bool,
) -> Result<(), Error> {
    let disable_config = ServerTlsConfig {
        mode: ServerTlsMode::Disable,
        connector: None,
    };
    let cancel_tls = if connected_with_tls {
        server_tls
    } else {
        &disable_config
    };

    let mut stream = if host.starts_with('/') {
        create_unix_stream_inner(host, port).await?
    } else {
        create_tcp_stream_inner(host, port, cancel_tls).await?
    };

    warn!("cancel request forwarded to {host}:{port} pid={process_id}");

    let mut bytes = BytesMut::with_capacity(16);
    bytes.put_i32(16);
    bytes.put_i32(CANCEL_REQUEST_CODE);
    bytes.put_i32(process_id);
    bytes.put_i32(secret_key);

    write_all_flush(&mut stream, &bytes).await
}
