//! Handler that builds the /metrics body and writes it onto a TcpStream.
//! The accept loop and HTTP routing live in `crate::web::server`.

use flate2::write::GzEncoder;
use flate2::Compression;
use log::error;
use prometheus::{Encoder, TextEncoder};
use std::io::Write;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::tcp::OwnedWriteHalf;

use super::metrics::update_metrics;
use super::REGISTRY;

/// Builds the Prometheus metrics body and writes a complete HTTP/1.1 response
/// onto the supplied writer. The mux must have already parsed the request
/// (this function performs no reads on the socket).
pub(crate) async fn write_metrics_response(
    writer: &mut BufWriter<OwnedWriteHalf>,
    accepts_gzip: bool,
) {
    update_metrics();

    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();

    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {e}");
        return;
    }

    let content_type = encoder.format_type();

    let (response_body, content_encoding) = if accepts_gzip {
        let mut compressed = Vec::new();
        {
            let mut gz = GzEncoder::new(&mut compressed, Compression::default());
            if let Err(e) = gz.write_all(&buffer) {
                error!("Failed to compress metrics data: {e}");
                return;
            }
            if let Err(e) = gz.finish() {
                error!("Failed to finish gzip compression: {e}");
                return;
            }
        }
        (compressed, "Content-Encoding: gzip\r\n")
    } else {
        (buffer, "")
    };

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n{}Content-Length: {}\r\n\r\n",
        content_type,
        content_encoding,
        response_body.len()
    );

    if let Err(e) = writer.write_all(response.as_bytes()).await {
        error!("Failed to write HTTP response header: {e}");
        return;
    }
    if let Err(e) = writer.write_all(&response_body).await {
        error!("Failed to write metrics data: {e}");
        return;
    }
    if let Err(e) = writer.flush().await {
        error!("Failed to flush connection: {e}");
    }
}
