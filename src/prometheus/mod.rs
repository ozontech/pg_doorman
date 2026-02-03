//! Prometheus metrics exporter for pg_doorman.
//!
//! This module provides a Prometheus-compatible metrics endpoint that exposes
//! various statistics about the connection pooler's operation.

use once_cell::sync::Lazy;
use prometheus::{Gauge, GaugeVec, Opts, Registry};

// Sub-modules
mod metrics;
mod server;
mod system;

#[cfg(test)]
mod tests;

// Re-exports
pub use server::start_prometheus_server;

// Define the metrics we want to expose
pub(crate) static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

pub(crate) static TOTAL_MEMORY: Lazy<Gauge> = Lazy::new(|| {
    let gauge = Gauge::new(
        "pg_doorman_total_memory",
        "Total memory allocated to the pg_doorman process in bytes. Monitors the memory footprint of the application.",
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_CONNECTIONS: Lazy<GaugeVec> = Lazy::new(|| {
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
pub(crate) static SHOW_SOCKETS: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_CLIENT: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_SERVER: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_BYTES: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOL_CACHE_ENTRIES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pool_prepared_cache_entries",
            "Number of entries in the pool-level prepared statement cache by user and database.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_POOL_CACHE_BYTES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pool_prepared_cache_bytes",
            "Approximate memory usage of the pool-level prepared statement cache in bytes by user and database."
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_CLIENT_CACHE_ENTRIES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_clients_prepared_cache_entries",
            "Total number of entries in all clients' prepared statement caches by user and database."
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_CLIENT_CACHE_BYTES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_clients_prepared_cache_bytes",
            "Total approximate memory usage of all clients' prepared statement caches in bytes by user and database."
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_ASYNC_CLIENTS_COUNT: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_async_clients_count",
            "Number of async clients (using Flush instead of Sync) by user and database.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_POOLS_QUERIES_PERCENTILE: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_TRANSACTIONS_PERCENTILE: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_TRANSACTIONS_COUNTER: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_TRANSACTIONS_TOTAL_TIME: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_QUERIES_COUNTER: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_QUERIES_TOTAL_TIME: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_POOLS_WAIT_TIME_AVG: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_SERVERS_PREPARED_HITS: Lazy<GaugeVec> = Lazy::new(|| {
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

pub(crate) static SHOW_SERVERS_PREPARED_MISSES: Lazy<GaugeVec> = Lazy::new(|| {
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
