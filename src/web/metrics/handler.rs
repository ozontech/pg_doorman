//! Handler that builds the /metrics body and writes it onto a TcpStream.
//! The accept loop and HTTP routing live in `crate::web::server`.

use flate2::write::GzEncoder;
use flate2::Compression;
use log::{error, info, warn};
use prometheus::{Encoder, TextEncoder};
use std::io::Write;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::tcp::OwnedWriteHalf;

use super::metrics::update_metrics;
use super::REGISTRY;

/// Above this latency a /metrics response is loud enough that we want to see
/// it in operator logs by default. Below it the timing is still logged, but
/// at INFO so it's part of normal operations rather than an alarm.
const SLOW_RESPONSE_THRESHOLD: Duration = Duration::from_millis(100);

/// Minimum gap between consecutive slow-response WARN lines. Without it a
/// misconfigured scraper (e.g. accidental `scrape_interval: 1s`) under a
/// genuine slow path turns into 1 warn/s, drowning out real signal in the
/// log pipeline. Operators learn nothing from the 2nd, 10th, 100th copy of
/// the same line, so we throttle to one per N seconds.
const SLOW_RESPONSE_LOG_INTERVAL_SECS: i64 = 30;

/// Unix-epoch second of the last slow-response WARN emitted from this path.
/// Shared across all in-flight `/metrics` requests so the gate is global.
static SLOW_RESPONSE_LAST_WARN: AtomicI64 = AtomicI64::new(0);

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

    // Log every /metrics request at INFO so operators see in the normal log
    // stream how often Prometheus is scraping and how long each call takes
    // (the typical question after a p99 regression is "did scrape get
    // slower"). Above SLOW_RESPONSE_THRESHOLD the same event is also raised
    // to WARN, rate-limited to one warn per
    // `SLOW_RESPONSE_LOG_INTERVAL_SECS` so a misbehaving scraper does not
    // turn it into a per-request flood.
    let elapsed = started.elapsed();
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    info!("/metrics request handled in {elapsed_ms:.1} ms (bytes={body_len}, gzip={accepts_gzip})");
    if elapsed >= SLOW_RESPONSE_THRESHOLD {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let last = SLOW_RESPONSE_LAST_WARN.load(Ordering::Relaxed);
        if now_secs - last >= SLOW_RESPONSE_LOG_INTERVAL_SECS
            && SLOW_RESPONSE_LAST_WARN
                .compare_exchange(last, now_secs, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            warn!(
                "/metrics request slow: {elapsed_ms:.1} ms \
                 (bytes={body_len}, gzip={accepts_gzip}, rate-limited 1/{SLOW_RESPONSE_LOG_INTERVAL_SECS}s)"
            );
        }
    }
}
