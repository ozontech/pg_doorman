//! Prometheus metrics exporter for pg_doorman.
//!
//! This module provides a Prometheus-compatible metrics endpoint that exposes
//! various statistics about the connection pooler's operation.

use once_cell::sync::Lazy;
use prometheus::{
    Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec,
    IntGaugeVec, Opts, Registry,
};

// Sub-modules
mod handler;
#[allow(clippy::module_inception)]
mod metrics;
pub(crate) mod system;

#[cfg(test)]
mod tests;

// Re-exports
pub(crate) use handler::write_metrics_response;
pub use metrics::{observe_anonymous_eviction, record_interner_gc, record_synthetic_miss};

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

pub(crate) static SHOW_POOL_SIZE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pool_size",
            "Configured maximum pool size per user and database. Useful for calculating remaining pool capacity together with pg_doorman_pools_servers.",
        ),
        &["user", "database"],
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

pub(crate) static SHOW_POOLS_OLDEST_ACTIVE_AGE_MS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_oldest_active_age_ms",
            "Maximum age in milliseconds among ACTIVE servers in each pool. Zero when no server is currently ACTIVE. Sustained non-zero values indicate stuck checkouts.",
        ),
        &["user", "database"],
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

pub(crate) static SHOW_CLIENT_PREPARED_NAMED_ENTRIES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_clients_prepared_named_entries",
            "Total Named entries across all clients' prepared statement caches by user and database.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_CLIENT_PREPARED_ANONYMOUS_ENTRIES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_clients_prepared_anonymous_entries",
            "Total Anonymous entries across all clients' prepared statement caches by user and database.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_CLIENT_PREPARED_ANONYMOUS_EVICTIONS_TOTAL: Lazy<IntCounterVec> =
    Lazy::new(|| {
        let counter = IntCounterVec::new(
            Opts::new(
                "pg_doorman_clients_prepared_anonymous_evictions_total",
                "Cumulative count of Anonymous LRU evictions on the per-client cache by user and \
                 database. A sustained non-zero rate signals that \
                 client_anonymous_prepared_cache_size is too small for the workload.",
            ),
            &["user", "database"],
        )
        .unwrap();
        REGISTRY.register(Box::new(counter.clone())).unwrap();
        counter
    });

/// Number of entries in the global query interner, split by kind (named or
/// anonymous). Refreshed once per GC sweep.
pub(crate) static QUERY_INTERNER_ENTRIES: Lazy<IntGaugeVec> = Lazy::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "pg_doorman_query_interner_entries",
            "Number of entries in the global query interner, split by kind \
             (named or anonymous). Refreshed once per GC sweep.",
        ),
        &["kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Total bytes of interned query text, split by kind. The named half is
/// bounded only by passive Arc::strong_count GC; the anonymous half is
/// bounded by query_interner_anon_idle_ttl_seconds.
pub(crate) static QUERY_INTERNER_BYTES: Lazy<IntGaugeVec> = Lazy::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "pg_doorman_query_interner_bytes",
            "Total length (bytes) of interned query text, split by kind \
             (named or anonymous). Refreshed once per GC sweep.",
        ),
        &["kind"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Cumulative count of interner evictions, split by kind and reason.
/// reason='gc_passive' for named entries removed because nothing outside
/// the interner held the Arc<str>; reason='ttl_expired' for anonymous
/// entries removed after exceeding the idle TTL.
pub(crate) static QUERY_INTERNER_EVICTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_query_interner_evictions_total",
            "Cumulative interner evictions, by kind (named|anonymous) and \
             reason (gc_passive|ttl_expired).",
        ),
        &["kind", "reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter for cases where pg_doorman returns SQLSTATE 26000 because an
/// anonymous prepared statement has been evicted from the interner before
/// the next Bind. A persistently non-zero rate signals either too short a
/// query_interner_anon_idle_ttl_seconds or a client pattern that depends
/// on cross-batch unnamed prepared statements.
pub(crate) static QUERY_INTERNER_SYNTHETIC_MISSES_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let counter = IntCounter::new(
        "pg_doorman_query_interner_synthetic_misses_total",
        "Times pg_doorman returned 26000 because an anonymous interner entry \
         expired or was evicted before the next Bind referencing it. \
         Sustained non-zero rate signals TTL too short or a driver \
         depending on cross-batch unnamed prepared statements.",
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Wall-clock time spent in a single GC sweep cycle (named + anonymous
/// combined). Custom buckets target sweep durations from 100 µs to 1 s
/// because shard-scan time scales with interner size.
pub(crate) static QUERY_INTERNER_GC_DURATION_SECONDS: Lazy<Histogram> = Lazy::new(|| {
    let opts = HistogramOpts::new(
        "pg_doorman_query_interner_gc_duration_seconds",
        "Wall-clock time spent in a single GC sweep cycle (named + anonymous combined).",
    )
    .buckets(vec![
        0.0001, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
    ]);
    let histogram = Histogram::with_opts(opts).unwrap();
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
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

pub(crate) static AUTH_QUERY_CACHE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_auth_query_cache",
            "Auth query cache metrics by type and database.",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static AUTH_QUERY_AUTH: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_auth_query_auth",
            "Auth query authentication outcomes by result and database.",
        ),
        &["result", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static AUTH_QUERY_EXECUTOR: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_auth_query_executor",
            "Auth query executor metrics by type and database.",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static AUTH_QUERY_DYNAMIC_POOLS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_auth_query_dynamic_pools",
            "Auth query dynamic pool metrics by type and database.",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static COORDINATOR: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pool_coordinator",
            "Pool coordinator current state by type and database. Types: connections (current total), \
             reserve_in_use (current reserve), max_connections (configured limit), \
             reserve_pool_size (configured reserve).",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static COORDINATOR_TOTALS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_pool_coordinator_total",
            "Pool coordinator cumulative counters by type and database. Types: evictions, \
             reserve_acquisitions, exhaustions.",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static POOL_SCALING_GAUGE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pool_scaling",
            "Per-pool gauges for the anticipation + bounded burst path. Types: \
             inflight_creates (server creates currently being established).",
        ),
        &["type", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static POOL_SCALING_TOTALS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_pool_scaling_total",
            "Per-pool cumulative counters for the anticipation + bounded burst path. Types: \
             creates_started (took a burst slot), \
             burst_gate_waits (had to wait on a Notify), \
             burst_gate_budget_exhausted (adaptive timeout fired, stopped waiting for handoff), \
             anticipation_wakes_notify (anticipation woke on idle return), \
             anticipation_wakes_timeout (anticipation budget elapsed without return), \
             create_fallback (anticipation did not avoid an allocation), \
             replenish_deferred (background replenish skipped due to gate full).",
        ),
        &["type", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static SHOW_SERVER_TLS_CONNECTIONS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_server_tls_connections",
            "Current number of backend connections using TLS encryption, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_SERVER_TLS_HANDSHAKE_DURATION: Lazy<HistogramVec> = Lazy::new(|| {
    let histogram = HistogramVec::new(
        prometheus::HistogramOpts::new(
            "pg_doorman_server_tls_handshake_duration_seconds",
            "Duration of TLS handshakes to backend PostgreSQL servers, by pool.",
        )
        .buckets(vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
        ]),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

pub(crate) static SHOW_SERVER_TLS_HANDSHAKE_ERRORS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_server_tls_handshake_errors_total",
            "Total number of failed TLS negotiations to backend servers, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static PATRONI_API_REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_patroni_api_requests_total",
            "Total number of Patroni /cluster requests, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static FALLBACK_CONNECTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_fallback_connections_total",
            "Total number of fallback connections established, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static PATRONI_API_ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_patroni_api_errors_total",
            "Total number of failed Patroni /cluster requests, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static FALLBACK_ACTIVE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_fallback_active",
            "1 when the local backend is in cooldown and the pool is using a fallback host, 0 otherwise. Per pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static FALLBACK_HOST: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_fallback_host",
            "Currently active fallback host (1 = active), by pool/host/port.",
        ),
        &["pool", "host", "port"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static FALLBACK_CACHE_HITS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_fallback_cache_hits_total",
            "Total number of times the cached fallback host was reused without re-querying Patroni, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static FALLBACK_CANDIDATE_FAILURES_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_fallback_candidate_failures_total",
            "Total number of fallback candidates rejected, by pool and failure reason \
             (connect_error, startup_error, server_unavailable, timeout, other).",
        ),
        &["pool", "reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static PATRONI_API_DURATION: Lazy<HistogramVec> = Lazy::new(|| {
    let histogram = HistogramVec::new(
        prometheus::HistogramOpts::new(
            "pg_doorman_patroni_api_duration_seconds",
            "Duration of Patroni /cluster requests, by pool.",
        )
        .buckets(vec![0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

// ----------------------------------------------------------------------------
// Web UI / SSO observability
// ----------------------------------------------------------------------------

pub(crate) static WEB_SSO_ENABLED: Lazy<prometheus::IntGauge> = Lazy::new(|| {
    let gauge = prometheus::IntGauge::new(
        "pg_doorman_web_sso_enabled",
        "1 when the web UI has SSO configured and the public key loaded successfully, 0 otherwise. Pairs with `pg_doorman_web_sso_config_error_total` to detect a misconfigured rollout.",
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static WEB_SSO_CONFIG_ERROR: Lazy<prometheus::IntGauge> = Lazy::new(|| {
    let gauge = prometheus::IntGauge::new(
        "pg_doorman_web_sso_config_error",
        "1 when [web].sso_enabled = true but the runtime failed to load (missing key file, empty audience, unparsable PEM, etc.), 0 otherwise. The exact reason is surfaced through /api/auth/config.sso_config_error.",
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static WEB_AUTH_ATTEMPTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_web_auth_attempts_total",
            "Web UI authentication attempts by resolved role and source. `role` is one of admin/sso/anonymous/rejected; `source` is basic/sso/none. Useful for tracking 401/403 spikes and the share of SSO-vs-Basic logins.",
        ),
        &["role", "source"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static WEB_REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_web_requests_total",
            "Web UI requests by status class and resolved role. `status_class` is 2xx/3xx/4xx/5xx. Pair with auth attempts to spot e.g. spikes in 4xx for the sso role (broken proxy) without wading through logs.",
        ),
        &["status_class", "role"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static WEB_SSO_VALIDATION_ERRORS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_web_sso_validation_errors_total",
            "JWT validation failures by reason: signature, expired, audience, no_username, allowlist. A sustained signature spike means the SSO proxy rotated keys without updating sso_public_key_file; allowlist spikes mean someone outside the allowlist is trying to log in.",
        ),
        &["reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});
