//! Stress generator for the `/metrics` HTTP endpoint.
//!
//! Spins up N tokio tasks that hammer the given URL in a loop using a shared
//! reqwest::Client (so the HTTP connection pool is reused). Used by the
//! `scripts/bench-metrics-impact.sh` bench to measure how aggressive Prometheus
//! scraping affects client-facing latency in pg_doorman.
//!
//! Default knobs are conservative; override via flags or environment:
//!
//! ```text
//! cargo run --release --bin metrics_stress -- \
//!     --url http://127.0.0.1:9127/metrics \
//!     --concurrency 32 \
//!     --duration-secs 30
//! ```

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(about = "HTTP keep-alive stress generator for /metrics", long_about = None)]
struct Args {
    /// Target URL.
    #[arg(long, default_value = "http://127.0.0.1:9127/metrics")]
    url: String,

    /// Number of concurrent tasks hammering the URL.
    #[arg(long, default_value_t = 32)]
    concurrency: usize,

    /// How long to run the stress, in seconds.
    #[arg(long, default_value_t = 30)]
    duration_secs: u64,

    /// Per-request timeout (ms). Caps a single response so a hang on the
    /// server side does not stall a task forever.
    #[arg(long, default_value_t = 5_000)]
    request_timeout_ms: u64,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(args.request_timeout_ms))
        .pool_max_idle_per_host(args.concurrency)
        .build()?;

    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + Duration::from_secs(args.duration_secs);

    let mut handles = Vec::with_capacity(args.concurrency);
    for _ in 0..args.concurrency {
        let client = client.clone();
        let url = args.url.clone();
        let ok = ok.clone();
        let err = err.clone();
        handles.push(tokio::spawn(async move {
            while Instant::now() < deadline {
                match client.get(&url).send().await {
                    Ok(resp) => {
                        // Drain the body so the connection returns to the
                        // keep-alive pool instead of being dropped.
                        let _ = resp.bytes().await;
                        ok.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        err.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    let started = Instant::now();
    for h in handles {
        let _ = h.await;
    }
    let elapsed = started.elapsed();

    let ok = ok.load(Ordering::Relaxed);
    let err = err.load(Ordering::Relaxed);
    let total = ok + err;
    let rps = if elapsed.as_secs_f64() > 0.0 {
        total as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    // Use stderr so the line survives any stdout buffering on a forced kill,
    // and flush explicitly. The bench script captures both streams.
    eprintln!(
        "metrics_stress: url={} concurrency={} duration={:.2}s requests={} ok={} err={} rps={:.0}",
        args.url,
        args.concurrency,
        elapsed.as_secs_f64(),
        total,
        ok,
        err,
        rps
    );
    let _ = std::io::stderr().flush();

    Ok(())
}
