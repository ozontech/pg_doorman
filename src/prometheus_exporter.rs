use crate::pool::StatsPoolIdentifier;
/// Prometheus metrics exporter for pg_doorman
#[cfg(target_os = "linux")]
use crate::stats::get_socket_states_count;
use crate::stats::pool::PoolStats;
use crate::stats::{
    get_server_stats, CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};
use flate2::write::GzEncoder;
use flate2::Compression;
use log::{error, info};
use once_cell::sync::Lazy;
use prometheus::{Encoder, Gauge, GaugeVec, Opts, Registry, TextEncoder};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use tokio::net::TcpSocket;

// Define the metrics we want to expose
static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);
static TOTAL_MEMORY: Lazy<Gauge> = Lazy::new(|| {
    let gauge = Gauge::new(
        "pg_doorman_total_memory",
        "Total memory allocated to the pg_doorman process in bytes. Monitors the memory footprint of the application.",
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_CONNECTIONS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
        "pg_doorman_connection_count",
        "Counter of new connections by type handled by pg_doorman. Types include: 'plain' (unencrypted connections), 'tls' (encrypted connections), 'cancel' (connection cancellation requests), and 'total' (sum of all connections).",
        ), &["type"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

#[cfg(target_os = "linux")]
static SHOW_SOCKETS: Lazy<GaugeVec> = Lazy::new(|| {
    let counter = GaugeVec::new(
        Opts::new(
            "pg_doorman_sockets",
            "Counter of sockets used by pg_doorman by socket type. Types include: 'tcp' (IPv4 TCP sockets), 'tcp6' (IPv6 TCP sockets), 'unix' (Unix domain sockets), and 'unknown' (sockets of unrecognized type). Only available on Linux systems.",
        ),
        &["type"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

static SHOW_POOLS_CLIENT: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_clients",
            "Number of clients in connection pools by status, user, and database. Status values include: 'idle' (connected but not executing queries), 'waiting' (waiting for a server connection), and 'active' (currently executing queries). Helps monitor connection pool utilization and client distribution.",
        ),
        &["status", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_SERVER: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_servers",
            "Number of servers in connection pools by status, user, and database. Status values include: 'active' (actively serving clients) and 'idle' (available for new connections). Helps monitor server availability and load distribution.",
        ),
        &["status", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_BYTES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_bytes",
            "Total bytes transferred through connection pools by direction, user, and database. Direction values include: 'received' (bytes received from clients) and 'sent' (bytes sent to clients). Useful for monitoring network traffic and identifying high-volume connections.",
        ),
        &["direction", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_QUERIES_PERCENTILE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_queries_percentile",
            "Query execution time percentiles by user and database. Percentile values include: '99', '95', '90', and '50' (median). Values are in milliseconds. Helps identify slow queries and performance trends across different users and databases.",
        ),
        &["percentile", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_TRANSACTIONS_PERCENTILE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_transactions_percentile",
            "Transaction execution time percentiles by user and database. Percentile values include: '99', '95', '90', and '50' (median). Values are in milliseconds. Helps monitor transaction performance and identify long-running transactions that might impact database performance.",
        ),
        &["percentile", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_TRANSACTIONS_COUNTER: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_transactions_count",
            "Counter of transactions executed in connection pools by user and database. Helps track transaction volume and identify users or databases with high transaction rates.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_TRANSACTIONS_TOTAL_TIME: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_transactions_total_time",
            "Total time spent executing transactions in connection pools by user and database. Values are in milliseconds. Helps monitor overall transaction performance and identify users or databases with high transaction execution times.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_QUERIES_COUNTER: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_queries_count",
            "Counter of queries executed in connection pools by user and database. Helps track query volume and identify users or databases with high query rates.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_QUERIES_TOTAL_TIME: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_queries_total_time",
            "Total time spent executing queries in connection pools by user and database. Values are in milliseconds. Helps monitor overall query performance and identify users or databases with high query execution times.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_POOLS_WAIT_TIME_AVG: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_avg_wait_time",
            "Average wait time for clients in connection pools by user and database. Values are in milliseconds. Helps monitor client wait times and identify potential bottlenecks.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_SERVERS_PREPARED_HITS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_servers_prepared_hits",
            "Counter of prepared statement hits in databases backends by user and database. Helps track the effectiveness of prepared statements in reducing query parsing overhead.",
        ),
        &["user", "database", "backend_pid"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

static SHOW_SERVERS_PREPARED_MISSES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_servers_prepared_misses",
            "Counter of prepared statement misses in databases backends by user and database. Helps identify queries that could benefit from being prepared to improve performance.",
        ),
        &["user", "database", "backend_pid"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Updates all metrics before they are exposed via the Prometheus endpoint.
fn update_metrics() {
    update_memory_metrics();
    update_connection_metrics();

    #[cfg(target_os = "linux")]
    update_socket_metrics();

    update_pool_metrics();
    update_server_metrics();
}

fn update_memory_metrics() {
    TOTAL_MEMORY.set(get_process_memory_usage() as f64);
}

fn update_connection_metrics() {
    let connection_types = [
        ("plain", &*PLAIN_CONNECTION_COUNTER),
        ("tls", &*TLS_CONNECTION_COUNTER),
        ("cancel", &*CANCEL_CONNECTION_COUNTER),
        ("total", &*TOTAL_CONNECTION_COUNTER),
    ];

    for (conn_type, counter) in &connection_types {
        SHOW_CONNECTIONS
            .with_label_values(&[conn_type])
            .set(counter.load(Ordering::Relaxed) as f64);
    }
}

#[cfg(target_os = "linux")]
fn update_socket_metrics() {
    match get_socket_states_count(std::process::id()) {
        Ok(states) => {
            let socket_states = [
                ("tcp", states.get_tcp()),
                ("tcp6", states.get_tcp6()),
                ("unix", states.get_unix()),
                ("unknown", states.get_unknown()),
            ];

            for (socket_type, count) in socket_states {
                SHOW_SOCKETS
                    .with_label_values(&[socket_type])
                    .set(count as f64);
            }
        }
        Err(e) => {
            SHOW_SOCKETS.reset();
            error!("Failed to get socket states count: {e:?}");
        }
    }
}

fn update_pool_metrics() {
    let lookup = PoolStats::construct_pool_lookup();
    reset_pool_metrics();

    for (identifier, stats) in lookup.iter() {
        update_pool_avg_metrics(identifier, stats);
        update_pool_server_metrics(identifier, stats);
        update_client_state_metrics(identifier, stats);
        update_byte_metrics(identifier, stats);
        update_percentile_metrics(identifier, stats);
    }
}

fn update_server_metrics() {
    SHOW_SERVERS_PREPARED_HITS.reset();
    SHOW_SERVERS_PREPARED_MISSES.reset();
    let stats = get_server_stats();
    for (_, server) in stats {
        // Create owned strings to avoid borrowing issues
        let username = server.username().to_string();
        let pool_name = server.pool_name().to_string();
        let process_id = server.process_id().to_string();

        let server_metrics = [
            (
                &SHOW_SERVERS_PREPARED_HITS,
                server.prepared_hit_count.load(Ordering::Relaxed) as f64,
            ),
            (
                &SHOW_SERVERS_PREPARED_MISSES,
                server.prepared_miss_count.load(Ordering::Relaxed) as f64,
            ),
        ];

        for (metric, value) in &server_metrics {
            metric
                .with_label_values(&[&username, &pool_name, &process_id])
                .set(*value);
        }
    }
}

fn update_pool_avg_metrics(identifier: &StatsPoolIdentifier, stats: &PoolStats) {
    let avg_metrics = [
        (
            &SHOW_POOLS_WAIT_TIME_AVG,
            stats.avg_wait_time as f64 / 1_000f64,
        ),
        (
            &SHOW_POOLS_TRANSACTIONS_COUNTER,
            stats.total_xact_count as f64,
        ),
        (
            &SHOW_POOLS_TRANSACTIONS_TOTAL_TIME,
            stats.total_xact_time_microseconds as f64 / 1_000f64,
        ),
        (&SHOW_POOLS_QUERIES_COUNTER, stats.total_query_count as f64),
        (
            &SHOW_POOLS_QUERIES_TOTAL_TIME,
            stats.total_query_time_microseconds as f64 / 1_000f64,
        ),
    ];

    for (metric, value) in &avg_metrics {
        metric
            .with_label_values(&[&identifier.user, &identifier.db])
            .set(*value);
    }
}

fn update_pool_server_metrics(identifier: &StatsPoolIdentifier, stats: &PoolStats) {
    let server_states = [("active", stats.sv_active), ("idle", stats.sv_idle)];

    for (state, value) in &server_states {
        SHOW_POOLS_SERVER
            .with_label_values(&[state, &identifier.user, &identifier.db])
            .set(*value as f64);
    }
}

fn reset_pool_metrics() {
    SHOW_POOLS_CLIENT.reset();
    SHOW_POOLS_SERVER.reset();
    SHOW_POOLS_BYTES.reset();
    SHOW_POOLS_QUERIES_PERCENTILE.reset();
    SHOW_POOLS_TRANSACTIONS_PERCENTILE.reset();
    SHOW_POOLS_WAIT_TIME_AVG.reset();
    SHOW_POOLS_TRANSACTIONS_COUNTER.reset();
    SHOW_POOLS_TRANSACTIONS_TOTAL_TIME.reset();
    SHOW_POOLS_QUERIES_COUNTER.reset();
    SHOW_POOLS_QUERIES_TOTAL_TIME.reset();
}

fn update_client_state_metrics(identifier: &StatsPoolIdentifier, stats: &PoolStats) {
    let states = [
        ("idle", stats.cl_idle),
        ("waiting", stats.cl_waiting),
        ("active", stats.cl_active),
    ];

    for (state, count) in states {
        SHOW_POOLS_CLIENT
            .with_label_values(&[state, &identifier.user, &identifier.db])
            .set(count as f64);
    }
}

fn update_byte_metrics(identifier: &StatsPoolIdentifier, stats: &PoolStats) {
    SHOW_POOLS_BYTES
        .with_label_values(&["received", &identifier.user, &identifier.db])
        .set(stats.bytes_received as f64);
    SHOW_POOLS_BYTES
        .with_label_values(&["sent", &identifier.user, &identifier.db])
        .set(stats.bytes_sent as f64);
}

fn update_percentile_metrics(identifier: &StatsPoolIdentifier, stats: &PoolStats) {
    const PERCENTILES: &[&str] = &["99", "95", "90", "50"];

    for percentile in PERCENTILES {
        let (query_value, xact_value) = match *percentile {
            "99" => (stats.query_percentile.p99, stats.xact_percentile.p99),
            "95" => (stats.query_percentile.p95, stats.xact_percentile.p95),
            "90" => (stats.query_percentile.p90, stats.xact_percentile.p90),
            "50" => (stats.query_percentile.p50, stats.xact_percentile.p50),
            _ => continue,
        };

        SHOW_POOLS_QUERIES_PERCENTILE
            .with_label_values(&[percentile, &identifier.user, &identifier.db])
            .set(query_value as f64 / 1_000f64);

        SHOW_POOLS_TRANSACTIONS_PERCENTILE
            .with_label_values(&[percentile, &identifier.user, &identifier.db])
            .set(xact_value as f64 / 1_000f64);
    }
}

/// Handles HTTP requests for metrics
async fn handle_metrics_request(stream: tokio::net::TcpStream) {
    // Clone the stream for reading
    let (read_half, write_half) = stream.into_split();
    let mut stream_reader = tokio::io::BufReader::new(read_half);
    let mut connection = tokio::io::BufWriter::new(write_half);
    let mut headers = [0; 1024];

    // Read HTTP request headers
    let n = match tokio::io::AsyncReadExt::read(&mut stream_reader, &mut headers).await {
        Ok(n) => n,
        Err(e) => {
            error!("Failed to read HTTP request: {e}");
            return;
        }
    };

    let headers_str = match std::str::from_utf8(&headers[..n]) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to parse HTTP headers: {e}");
            return;
        }
    };

    // Check if client accepts gzip encoding
    let accepts_gzip =
        headers_str.contains("Accept-Encoding") && headers_str.to_lowercase().contains("gzip");

    // Update metrics before serving
    update_metrics();

    // Encode metrics to the Prometheus text format
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();

    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {e}");
        return;
    }

    let content_type = encoder.format_type();

    // Prepare response body (compressed or not)
    let (response_body, content_encoding) = if accepts_gzip {
        // Compress the buffer with gzip
        let mut compressed = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            if let Err(e) = encoder.write_all(&buffer) {
                error!("Failed to compress metrics data: {e}");
                return;
            }
            if let Err(e) = encoder.finish() {
                error!("Failed to finish gzip compression: {e}");
                return;
            }
        }
        (compressed, "Content-Encoding: gzip\r\n")
    } else {
        (buffer, "")
    };

    // Prepare HTTP response
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n{}Content-Length: {}\r\n\r\n",
        content_type,
        content_encoding,
        response_body.len()
    );

    // Send response
    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut connection, response.as_bytes()).await
    {
        error!("Failed to write HTTP response header: {e}");
        return;
    }

    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut connection, &response_body).await {
        error!("Failed to write metrics data: {e}");
        return;
    }

    if let Err(e) = tokio::io::AsyncWriteExt::flush(&mut connection).await {
        error!("Failed to flush connection: {e}");
    }
}

/// Starts the prometheus exporter
pub async fn start_prometheus_server(host: &str) {
    info!("Starting prometheus exporter on {host}");
    let addr: SocketAddr = match host.parse() {
        Ok(addr) => addr,
        Err(e) => {
            panic!("Failed to parse socket address '{host}': {e}");
        }
    };
    let listen_socket = if addr.is_ipv4() {
        match TcpSocket::new_v4() {
            Ok(socket) => socket,
            Err(e) => {
                panic!("Failed to create IPv4 socket: {e}");
            }
        }
    } else {
        match TcpSocket::new_v6() {
            Ok(socket) => socket,
            Err(e) => {
                panic!("Failed to create IPv6 socket: {e}");
            }
        }
    };
    if let Err(e) = listen_socket.set_reuseaddr(true) {
        panic!("Failed to set SO_REUSEADDR: {e}");
    }

    if let Err(e) = listen_socket.set_reuseport(true) {
        panic!("Failed to set SO_REUSEPORT: {e}");
    }

    if let Err(e) = listen_socket.bind(addr) {
        panic!("Failed to bind to address {addr}: {e}");
    }
    match listen_socket.listen(1024) {
        Ok(listener) => {
            info!("prometheus exporter listening on {addr}");

            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(async move {
                            handle_metrics_request(stream).await;
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {e}");
                    }
                }
            }
        }
        Err(e) => {
            panic!("Failed to bind Prometheus metrics server to {addr}: {e}");
        }
    }
}

/// Gets the current memory usage of the process in bytes
fn get_process_memory_usage() -> u64 {
    #[cfg(target_os = "linux")]
    {
        // On Linux, read from /proc/self/statm
        match std::fs::read_to_string("/proc/self/statm") {
            Ok(statm) => {
                let values: Vec<&str> = statm.split_whitespace().collect();
                if !values.is_empty() {
                    if let Ok(pages) = values[0].parse::<u64>() {
                        // Convert pages to bytes (page size is typically 4KB)
                        return pages * 4096;
                    }
                }
                0
            }
            Err(_) => 0,
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        // On macOS, use ps command
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output();

        match output {
            Ok(output) => {
                let rss = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u64>();
                match rss {
                    Ok(kb) => kb * 1024, // Convert KB to bytes
                    Err(_) => 0,
                }
            }
            Err(_) => 0,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Default implementation for other platforms
        0
    }
}
