//! Handler that builds the /metrics body and writes it onto a TcpStream.
//! The accept loop and HTTP routing live in `crate::web::server`.

use flate2::write::GzEncoder;
use flate2::Compression;
use log::{debug, error, warn};
use prometheus::{Encoder, TextEncoder};
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::tcp::OwnedWriteHalf;

use super::metrics::update_metrics;
use super::REGISTRY;

/// Above this latency a /metrics response is loud enough that we want to see
/// it in operator logs by default. Below it we still record the timing at
/// DEBUG so a profiler-driven investigation can collect the trace without
/// noise.
const SLOW_RESPONSE_THRESHOLD: Duration = Duration::from_millis(100);

/// Builds the Prometheus metrics body and writes a complete HTTP/1.1 response
/// onto the supplied writer. The mux must have already parsed the request
/// (this function performs no reads on the socket).
pub(crate) async fn write_metrics_response(
    writer: &mut BufWriter<OwnedWriteHalf>,
    accepts_gzip: bool,
) {
    let started = Instant::now();
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

    let body_len = response_body.len();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{content_encoding}Content-Length: {body_len}\r\n\r\n"
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

    // Surface how long a single /metrics request took. Useful both for
    // operators watching client p99 regress under a noisy scraper and for
    // regression checks: a sudden jump in this number on previously-quiet
    // deployments points straight back at this code path.
    let elapsed = started.elapsed();
    if elapsed >= SLOW_RESPONSE_THRESHOLD {
        warn!(
            "/metrics request handled in {:.1} ms (bytes={}, gzip={})",
            elapsed.as_secs_f64() * 1000.0,
            body_len,
            accepts_gzip
        );
    } else {
        debug!(
            "/metrics request handled in {:.1} ms (bytes={}, gzip={})",
            elapsed.as_secs_f64() * 1000.0,
            body_len,
            accepts_gzip
        );
    }
}
