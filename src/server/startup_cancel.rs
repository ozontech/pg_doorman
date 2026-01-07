use bytes::{BufMut, BytesMut};
use log::warn;

use crate::errors::Error;
use crate::messages::constants::CANCEL_REQUEST_CODE;
use crate::messages::write_all_flush;

use super::stream::{create_tcp_stream_inner, create_unix_stream_inner};

/// Issue a query cancellation request to the server.
/// Uses a separate connection that's not part of the connection pool.
pub(crate) async fn cancel(
    host: &str,
    port: u16,
    process_id: i32,
    secret_key: i32,
) -> Result<(), Error> {
    let mut stream = if host.starts_with('/') {
        create_unix_stream_inner(host, port).await?
    } else {
        create_tcp_stream_inner(host, port, false, false).await?
    };

    warn!("Sending CancelRequest to [{process_id}] {host}:{port}");

    let mut bytes = BytesMut::with_capacity(16);
    bytes.put_i32(16);
    bytes.put_i32(CANCEL_REQUEST_CODE);
    bytes.put_i32(process_id);
    bytes.put_i32(secret_key);

    write_all_flush(&mut stream, &bytes).await
}
