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
pub use metrics::{
    observe_anonymous_eviction, observe_backend_create_phase, observe_pool_query_microseconds,
    observe_pool_transaction_microseconds, observe_pool_wait_microseconds, observe_streaming_bytes,
    observe_streaming_event, record_interner_gc, record_listener_rejection, record_synthetic_miss,
    refresh_static_info_metrics,
};

// Define the metrics we want to expose
pub(crate) static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

/// `build_info`-style gauge that always reports `1` and carries the
/// pg_doorman version in a label. Pinned to one series per process so
/// dashboards can join on `version` without affecting cardinality, and
/// alerts can fire on a missing series after a deploy. Refreshed on
/// startup and on every config reload (the value never changes mid-run,
/// but RELOAD calls the same refresh path so a future override has one
/// place to land).
pub(crate) static BUILD_INFO: Lazy<IntGaugeVec> = Lazy::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "pg_doorman_build_info",
            "Static information about the running pg_doorman binary, exposed as a gauge fixed at 1. The version label carries the crate version (Cargo.toml) so dashboards can show 'which build is in production' without parsing logs.",
        ),
        &["version"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// One series per `(user, database, pool_mode)` triple defined in the
/// active configuration, value always `1`. Refreshed on every config
/// reload so an operator can spot pools that disappeared (series
/// missing) or appeared (new series) without diffing the TOML.
pub(crate) static USERS_CONFIGURED: Lazy<IntGaugeVec> = Lazy::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "pg_doorman_users_configured",
            "Configured (user, database, pool_mode) triples — one series per triple, value 1. Disappears on RELOAD when the corresponding pool is removed; appears when added.",
        ),
        &["user", "database", "pool_mode"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Current effective log filter, reported as one series per active
/// level/module-filter string with value `1`. The set of series
/// changes when an operator runs `SET log_level = ...` via the admin
/// console — a temporary `debug` enabled mid-incident shows up
/// immediately and disappears when reverted.
pub(crate) static LOG_LEVEL_INFO: Lazy<IntGaugeVec> = Lazy::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "pg_doorman_log_level",
            "Current effective log filter as reported by `SHOW LOG_LEVEL` / admin console. One series with value 1, label `level` carries the filter string (e.g. 'info', 'warn,pg_doorman::pool=debug').",
        ),
        &["level"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static TOTAL_MEMORY: Lazy<Gauge> = Lazy::new(|| {
    let gauge = Gauge::new(
        "pg_doorman_total_memory",
        "Total memory allocated to the pg_doorman process in bytes. Monitors the memory footprint of the application.",
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// DEPRECATED: monotonic value exposed as a Gauge — `rate()` works in
/// practice but Prometheus reset detection breaks on restart because the
/// gauge does not declare itself as monotonic. Prefer
/// `pg_doorman_connections_total`. Scheduled for removal in 3.10.
pub(crate) static SHOW_CONNECTIONS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
        "pg_doorman_connection_count",
        "DEPRECATED, removed in 3.10: cumulative count of accepted connections by type ('plain'/'tls'/'cancel'/'total'). Exposed as a gauge but the underlying counter is monotonic — Prometheus reset detection cannot tell a process restart apart from a counter wrap. Use pg_doorman_connections_total instead.",
        ), &["type"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Per-type cumulative counter of accepted client connections.
/// Replaces the gauge form `pg_doorman_connection_count`, which the
/// scrape protocol could not flag as monotonic — so a process restart
/// looked indistinguishable from a counter wrap-around. Updated by the
/// scrape path from the same atomic counters that already feed the
/// gauge, via per-scrape delta emission.
pub(crate) static SHOW_CONNECTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_connections_total",
            "Cumulative count of accepted client connections by type: 'plain' (unencrypted), 'tls' (encrypted), 'cancel' (cancel-query startup), 'total' (sum of all). Counter form replaces pg_doorman_connection_count.",
        ),
        &["type"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
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

pub(crate) static SHOW_POOLS_PAUSED: Lazy<IntGaugeVec> = Lazy::new(|| {
    let gauge = IntGaugeVec::new(
        Opts::new(
            "pg_doorman_pools_paused",
            "Whether the pool is currently paused (1) or running (0). Reflects the PAUSE/RESUME admin command per pool. A pool stuck at 1 after incident triage drops all client traffic until manually resumed.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_POOLS_MAXWAIT_MICROSECONDS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_maxwait_microseconds",
            "Largest single client checkout wait, taken as max(client.max_wait_time) across the alive clients of each pool. Each client tracks its own lifetime maximum (fetch_max), so a client that hit a slow checkout once keeps the pool gauge at that value until it disconnects — interpret spikes as 'someone in this pool ever waited this long', not 'someone is waiting now'.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Per-pool error counter split by SQLSTATE class. The `sqlstate` label is
/// constrained to a fixed whitelist — `08` (connection_exception), `53`
/// (insufficient_resources), `57` (operator_intervention), the exact codes
/// `25P02` (in_failed_sql_transaction) and `26000` (invalid_sql_statement_name),
/// and `other` for everything else — so the cardinality stays at 6 ×
/// pool count regardless of what the backend returns. The full 5-character
/// breakdown is still available through `/api/pools` and the Web UI;
/// Prometheus only carries the class-level rollup.
pub(crate) static SHOW_POOLS_ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_pools_errors_total",
            "Cumulative count of backend errors observed per pool, bucketed \
             by SQLSTATE class. The sqlstate label is one of: '08' \
             (connection_exception), '53' (insufficient_resources), '57' \
             (operator_intervention), '25P02' (in_failed_sql_transaction), \
             '26000' (invalid_sql_statement_name), or 'other'. The full \
             5-character breakdown is available through /api/pools and \
             the Web UI; Prometheus only carries the class-level rollup.",
        ),
        &["user", "database", "sqlstate"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// DEPRECATED: monotonic byte counter shipped as a Gauge. Prefer
/// `pg_doorman_pools_bytes_total`. Scheduled for removal in 3.10.
pub(crate) static SHOW_POOLS_BYTES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_bytes",
            "DEPRECATED, removed in 3.10: cumulative bytes transferred per direction/user/database, exposed as a gauge. Use pg_doorman_pools_bytes_total instead — Prometheus then handles reset detection correctly across restarts.",
        ),
        &["direction", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Per-pool, per-direction byte counter. Direction is 'received'
/// (client → backend) or 'sent' (backend → client). Replaces the
/// gauge form `pg_doorman_pools_bytes`. Updated from the same atomic
/// counters that feed the existing gauge.
pub(crate) static SHOW_POOLS_BYTES_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_pools_bytes_total",
            "Cumulative bytes transferred per pool and direction. Direction is 'received' (data from client) or 'sent' (data to client). Counter form replaces pg_doorman_pools_bytes.",
        ),
        &["direction", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
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

/// Wall-clock duration of each phase of backend connection setup, split
/// by phase. Phases are disjoint and additive:
/// - `tcp_connect` — raw socket connect (TcpStream::connect or
///   UnixStream::connect plus socket configuration).
/// - `tls` — full TLS bring-up: SSL request roundtrip plus the TLS
///   handshake. Skipped for plain TCP and Unix sockets.
/// - `auth` — from `StartupMessage` send to `AuthenticationOK` receive
///   (the credential exchange itself, including SCRAM rounds).
/// - `startup` — from `AuthenticationOK` to `ReadyForQuery` (parameter
///   status messages and backend key delivery).
///
/// Aggregated across all pools so the only dimension is the four-value
/// `phase` label regardless of fleet size — the question this answers
/// is "where in backend setup does pg_doorman spend its time", not
/// "which pool is slow". Per-pool TLS handshake time is still
/// available on `pg_doorman_server_tls_handshake_duration_seconds`.
///
/// Failure paths leave the affected phase silent: a TLS handshake
/// error does not produce a `tls` sample, and an early backend
/// `ErrorResponse` skips `auth` and `startup`. The phase you don't
/// see is the phase that failed before completing.
pub(crate) static BACKEND_CREATE_DURATION_SECONDS: Lazy<HistogramVec> = Lazy::new(|| {
    let histogram = HistogramVec::new(
        prometheus::HistogramOpts::new(
            "pg_doorman_backend_create_duration_seconds",
            "Wall-clock duration of each backend connection setup phase, \
             split by phase: 'tcp_connect' (raw socket), 'tls' (TLS \
             setup, only for TCP+TLS pools), 'auth' (StartupMessage to \
             AuthenticationOK), 'startup' (AuthenticationOK to \
             ReadyForQuery). Aggregated across pools.",
        )
        .buckets(vec![0.001, 0.01, 0.1, 1.0, 10.0]),
        &["phase"],
    )
    .unwrap();
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

/// Counter for client connections rejected before authentication completes,
/// split by reason. The label set is fixed:
/// - `hba` — HBA configuration explicitly denied the client
/// - `tls_required` — client tried plain text while `only_ssl_connections` is on
/// - `tls_handshake_fail` — TLS negotiation failed (bad cert, version mismatch, ...)
/// - `protocol_error` — unexpected sequence of startup messages
/// - `invalid_startup` — malformed startup packet or socket error before parameters
/// - `too_many_clients` — listener at `max_clients` capacity
///
/// A sustained non-zero `hba` or `tls_handshake_fail` rate is the bruteforce
/// signal pg_doorman previously only logged.
pub(crate) static LISTENER_REJECTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_listener_rejections_total",
            "Cumulative count of client connections rejected before \
             authentication, by reason. Reasons: 'hba' (HBA denied), \
             'tls_required' (plain text rejected by only_ssl_connections), \
             'tls_handshake_fail' (TLS negotiation failed), \
             'protocol_error' (unexpected startup message sequence), \
             'invalid_startup' (malformed startup or socket error), \
             'too_many_clients' (listener at capacity).",
        ),
        &["reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counts backend startup attempts pg_doorman aborted because PostgreSQL
/// returned an `ErrorResponse` that names a key the pool actually sent in
/// `StartupMessage`. Labels:
///
/// * `pool` — `<user>@<database>` identifier.
/// * `sqlstate` — PG SQLSTATE on the rejection (`22023`, `42704`,
///   `42501`, `55P02`, or any other code under the startup-parameter
///   family — pg_doorman does not pre-filter by SQLSTATE).
///
/// SQLSTATE class `57P*` (server unavailable) is excluded: those
/// `ErrorResponse`s are surfaced as `ServerUnavailableError` to drive the
/// Patroni-assisted fallback path before the counter branch is reached.
///
/// Identification of the failing key is best-effort: pg_doorman first
/// parses the canonical English `parameter "<name>"` phrase, then falls
/// back to scanning the M-field for any sent key wrapped in double
/// quotes (PG keeps the quote markers across all `lc_messages` locales).
/// If both heuristics fail — typically because the PG error is unrelated
/// to the sent map at all — the counter does NOT increment. Operator
/// reading a non-zero rate can be confident the issue is on a key they
/// configured; the per-line warn log carries the parameter name and
/// username for triage.
///
/// The parameter name and username are intentionally NOT in the label
/// set so a dynamic `auth_query` pool that mints per-tenant roles cannot
/// blow up Prometheus series count by reading user input into labels.
/// Counts cases where pg_doorman dropped operator-supplied
/// `startup_parameters` *before* the StartupMessage went on the wire —
/// the failure mode the per-pool `*_errors_total` counter cannot see
/// because PG never had a chance to reject. Labels:
///
/// * `pool` — `<user>@<database>` identifier.
/// * `reason` — bounded enum:
///   * `cascade_budget_exceeded` — the merged general+pool+auth_query
///     map exceeded the operator budget (`MAX_OPERATOR_BUDGET`, 9 488
///     bytes). Every operator-supplied key was dropped for that spawn
///     and the backend got PG defaults instead.
///   * `packet_cap_exceeded` — the full StartupMessage including user,
///     application_name and database would exceed PG's
///     `MAX_STARTUP_PACKET_LENGTH` (10 000 bytes). Same drop-all
///     behaviour.
///   * `auth_query_oversize` — the auth_query `startup_parameters`
///     text column for some username exceeded the operator budget at
///     parse time, so the per-user overlay is ignored.
///   * `auth_query_overlay_oversize` — the merged baseline+overlay was
///     over budget, but the baseline alone fits. Keeps general/pool
///     guardrails (statement_timeout, lock_timeout, …) for that
///     user instead of stripping the operator cascade outright.
///   * `auth_query_invalid_entry` — an individual entry in the
///     auth_query JSON failed validation (reserved key, bad GUC name,
///     null byte, non-string value). Incremented per offending entry.
///   * `dedicated_mode` — a per-user auth_query entry carried
///     startup_parameters, but the pool runs in dedicated auth_query
///     mode (one shared backend across users) so the per-user overlay
///     was dropped. Incremented per dropped entry.
///
/// All cases also emit a `warn!` log line for human triage; the
/// counter exists so dashboards and alerts can spot the silent drop
/// without log scraping.
pub(crate) static STARTUP_PARAMETERS_DROPPED_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_startup_parameters_dropped_total",
            "Cumulative count of operator-supplied startup_parameters \
             entries pg_doorman dropped before sending StartupMessage. \
             Labels: pool, reason (cascade_budget_exceeded, \
             packet_cap_exceeded, auth_query_oversize, \
             auth_query_overlay_oversize, auth_query_invalid_entry, \
             dedicated_mode). Distinct from \
             pg_doorman_backend_startup_parameter_errors_total which \
             counts PG-side rejections after StartupMessage.",
        ),
        &["pool", "reason"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub(crate) static BACKEND_STARTUP_PARAMETER_ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_backend_startup_parameter_errors_total",
            "Cumulative count of backend startup attempts pg_doorman \
             aborted because PostgreSQL ErrorResponse identified a key \
             this pool sent in StartupMessage (operator-supplied \
             startup_parameters cascade). Labels: pool, sqlstate. \
             SQLSTATE class 57P (server unavailable) is excluded — \
             those rejections take the Patroni-assisted fallback path \
             instead. The failing parameter name and username are in \
             the corresponding warn log line; kept out of the label set \
             so dynamic auth_query roles cannot inflate Prometheus \
             series count.",
        ),
        &["pool", "sqlstate"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter for protocol-level large-message streaming events. pg_doorman
/// drops to byte-stream forwarding when a server message of type DataRow
/// ('D') or CopyData ('d') exceeds max_message_size — see
/// src/server/protocol_io.rs handle_large_data_row / handle_large_copy_data.
/// This is invalid for healthy OLTP traffic; sustained non-zero rate
/// signals oversized BYTEA/JSONB payloads, COPY rows with pathological
/// content, or a misbehaving ORM. Result distinguishes a clean forward
/// (ok) from a partially streamed payload that ended in a connection
/// reset (error).
pub(crate) static STREAMING_EVENTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_streaming_events_total",
            "Cumulative count of large-message streaming events by user, \
             database, message kind (data_row|copy_data) and result \
             (ok|error). Each event corresponds to one message larger than \
             max_message_size that pg_doorman forwarded byte-for-byte from \
             the backend to the client.",
        ),
        &["user", "database", "kind", "result"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter for bytes pushed through the streaming path described above.
/// Includes the message header (5 bytes) plus the payload. Updated even
/// for failed events — the counter records what reached the client wire,
/// not only fully delivered messages.
pub(crate) static STREAMING_BYTES_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_streaming_bytes_total",
            "Bytes forwarded through the byte-stream path (header + payload), \
             split by user, database and message kind (data_row|copy_data). \
             Incremented before the event result is known, so bytes that \
             flowed during a failed stream are counted as well.",
        ),
        &["user", "database", "kind"],
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

/// DEPRECATED: pre-aggregated percentile gauges that cannot be summed
/// across pods. Prefer `pg_doorman_pools_query_duration_seconds_bucket`
/// and `histogram_quantile()`. Scheduled for removal in 3.10.
pub(crate) static SHOW_POOLS_QUERIES_PERCENTILE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_queries_percentile",
            "DEPRECATED, removed in 3.10: pre-aggregated query latency percentiles (50/90/95/99) per user and database, in milliseconds. Use pg_doorman_pools_query_duration_seconds (histogram) with histogram_quantile() instead — that one composes correctly across replicas.",
        ),
        &["percentile", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// DEPRECATED: see `SHOW_POOLS_QUERIES_PERCENTILE`. Prefer
/// `pg_doorman_pools_transaction_duration_seconds_bucket`.
pub(crate) static SHOW_POOLS_TRANSACTIONS_PERCENTILE: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_transactions_percentile",
            "DEPRECATED, removed in 3.10: pre-aggregated transaction latency percentiles (50/90/95/99) per user and database, in milliseconds. Use pg_doorman_pools_transaction_duration_seconds (histogram) with histogram_quantile() instead.",
        ),
        &["percentile", "user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// DEPRECATED: monotonic transaction count exposed as a Gauge. Prefer
/// `pg_doorman_pools_transactions_total`. Scheduled for removal in 3.10.
pub(crate) static SHOW_POOLS_TRANSACTIONS_COUNTER: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_transactions_count",
            "DEPRECATED, removed in 3.10: cumulative transaction count per user/database, exposed as a gauge. Use pg_doorman_pools_transactions_total instead.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Cumulative transaction count per pool. Replaces the gauge form
/// `pg_doorman_pools_transactions_count`.
pub(crate) static SHOW_POOLS_TRANSACTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_pools_transactions_total",
            "Cumulative transaction count per pool, by user and database. Counter form replaces pg_doorman_pools_transactions_count.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
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

/// DEPRECATED: monotonic query count exposed as a Gauge. Prefer
/// `pg_doorman_pools_queries_total`. Scheduled for removal in 3.10.
pub(crate) static SHOW_POOLS_QUERIES_COUNTER: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_queries_count",
            "DEPRECATED, removed in 3.10: cumulative query count per user/database, exposed as a gauge. Use pg_doorman_pools_queries_total instead.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Cumulative query count per pool. Replaces the gauge form
/// `pg_doorman_pools_queries_count`.
pub(crate) static SHOW_POOLS_QUERIES_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_pools_queries_total",
            "Cumulative query count per pool, by user and database. Counter form replaces pg_doorman_pools_queries_count.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
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

/// DEPRECATED: running mean that drowns spikes. Prefer
/// `pg_doorman_pools_wait_duration_seconds_bucket` with
/// `histogram_quantile()` for tail latency. Scheduled for removal in 3.10.
pub(crate) static SHOW_POOLS_WAIT_TIME_AVG: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_pools_avg_wait_time",
            "DEPRECATED, removed in 3.10: running mean of client checkout wait per user and database, in milliseconds. The running mean washes out tail latency spikes that operators care about; use pg_doorman_pools_wait_duration_seconds (histogram) with histogram_quantile() instead.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Server-side query latency histogram per pool. Buckets cover OLTP and
/// the long-tail OLAP-style queries (0.5 ms → 5 s). Replaces the
/// pre-aggregated `pg_doorman_pools_queries_percentile` gauges, which
/// could not be summed across replicas — `histogram_quantile()` here
/// gives correct quantiles even when several pg_doorman pods scrape
/// into the same Prometheus.
pub(crate) static SHOW_POOLS_QUERY_DURATION_SECONDS: Lazy<HistogramVec> = Lazy::new(|| {
    let histogram = HistogramVec::new(
        prometheus::HistogramOpts::new(
            "pg_doorman_pools_query_duration_seconds",
            "Server-side query latency per pool (StartupMessage-to-CommandComplete time \
             of every individual query that pg_doorman forwards). Use histogram_quantile() \
             over the _bucket series for percentiles; rate(_count) for QPS.",
        )
        .buckets(vec![0.0005, 0.005, 0.05, 0.5, 5.0]),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

/// Transaction latency histogram per pool. Captures the full
/// transaction span (BEGIN/start to COMMIT/end), so values are
/// typically larger than per-query latency by the number of statements
/// in the transaction plus inter-statement gaps. Replaces
/// `pg_doorman_pools_transactions_percentile`.
pub(crate) static SHOW_POOLS_TRANSACTION_DURATION_SECONDS: Lazy<HistogramVec> = Lazy::new(|| {
    let histogram = HistogramVec::new(
        prometheus::HistogramOpts::new(
            "pg_doorman_pools_transaction_duration_seconds",
            "Transaction latency per pool — full span from transaction start to its \
             COMMIT or ROLLBACK reply, including inter-statement application gaps. \
             Use histogram_quantile() over the _bucket series for percentiles; \
             rate(_count) for TPS.",
        )
        .buckets(vec![0.001, 0.01, 0.1, 1.0, 10.0]),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

/// Client checkout-wait latency histogram per pool. Records the full
/// time `Pool::timeout_get` spent before handing a backend to the
/// client (semaphore wait, anticipation, coordinator path, burst gate,
/// `server_pool.create()`). Replaces `pg_doorman_pools_avg_wait_time`,
/// whose running mean drowns spikes operators care about.
pub(crate) static SHOW_POOLS_WAIT_DURATION_SECONDS: Lazy<HistogramVec> = Lazy::new(|| {
    let histogram = HistogramVec::new(
        prometheus::HistogramOpts::new(
            "pg_doorman_pools_wait_duration_seconds",
            "Client checkout wait latency per pool — the time a client spent in \
             pg_doorman's queue before receiving a backend connection. Sustained p99 \
             above query duration means the pool, not PostgreSQL, is the bottleneck. \
             Use histogram_quantile() for percentiles.",
        )
        .buckets(vec![0.0001, 0.001, 0.01, 0.1, 1.0]),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(histogram.clone())).unwrap();
    histogram
});

/// Aggregated prepared-statement hits across all backends of a pool.
/// `backend_pid` is intentionally absent: each backend's PID survives only
/// `server_lifetime` (20 minutes by default, often shorter under churn),
/// so labelling per PID would leak hundreds of stale series per pool per
/// day even though every useful query (`sum by (user, database) (...)`,
/// `rate(...)`) immediately collapses them anyway.
pub(crate) static SHOW_SERVERS_PREPARED_HITS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_servers_prepared_hits",
            "Cumulative prepared-statement cache hits across all backends of each pool, by user and database. Compare with pg_doorman_servers_prepared_misses to derive hit ratio.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

/// Aggregated prepared-statement misses across all backends of a pool.
/// See `SHOW_SERVERS_PREPARED_HITS` for why `backend_pid` is not a label.
pub(crate) static SHOW_SERVERS_PREPARED_MISSES: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_servers_prepared_misses",
            "Cumulative prepared-statement cache misses across all backends of each pool, by user and database. A sustained non-zero rate signals queries that could benefit from being prepared, or from a larger server_prepared_statement_cache_size.",
        ),
        &["user", "database"],
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

/// Counter form of `auth_query_cache` for the cumulative `type`s
/// (hits/misses/refetches/rate_limited). The `entries` value is a
/// snapshot, not cumulative, so it stays on the gauge form.
pub(crate) static AUTH_QUERY_CACHE_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_auth_query_cache_total",
            "Cumulative auth query cache events by type ('hits'/'misses'/'refetches'/'rate_limited') and database.",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter form of `auth_query_auth` outcomes.
pub(crate) static AUTH_QUERY_AUTH_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_auth_query_auth_total",
            "Cumulative auth query authentication outcomes by result ('success'/'failure') and database.",
        ),
        &["result", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter form of `auth_query_executor`. Both `queries` and `errors`
/// are cumulative.
pub(crate) static AUTH_QUERY_EXECUTOR_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_auth_query_executor_total",
            "Cumulative auth query executor events by type ('queries'/'errors') and database.",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter form of `auth_query_dynamic_pools` for the cumulative
/// `type`s (created/destroyed). The `current` value is a snapshot,
/// not cumulative, so it stays on the gauge form.
pub(crate) static AUTH_QUERY_DYNAMIC_POOLS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_auth_query_dynamic_pools_total",
            "Cumulative auth query dynamic pool lifecycle events by type ('created'/'destroyed') and database.",
        ),
        &["type", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter form of `servers_prepared_hits`. The gauge form aggregates
/// per-pool sums across live backends and naturally drops when a
/// backend is recycled; this counter mirrors the same per-pool sum
/// through the delta tracker so a `rate()` over the counter is stable
/// across the `server_lifetime` rotation churn.
pub(crate) static SHOW_SERVERS_PREPARED_HITS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_servers_prepared_hits_total",
            "Cumulative prepared-statement cache hits across all backends of each pool, by user and database.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

/// Counter form of `servers_prepared_misses`. See
/// `SHOW_SERVERS_PREPARED_HITS_TOTAL` for the rationale.
pub(crate) static SHOW_SERVERS_PREPARED_MISSES_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_servers_prepared_misses_total",
            "Cumulative prepared-statement cache misses across all backends of each pool, by user and database.",
        ),
        &["user", "database"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
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
        "1 when the web UI has SSO configured and the public key loaded successfully, 0 otherwise. Pairs with `pg_doorman_web_sso_config_error` to detect a misconfigured SSO setup.",
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static WEB_SSO_CONFIG_ERROR: Lazy<prometheus::IntGauge> = Lazy::new(|| {
    let gauge = prometheus::IntGauge::new(
        "pg_doorman_web_sso_config_error",
        "1 when [web].sso_enabled = true but the runtime failed to load (missing key file, empty audience, unparsable PEM, etc.), 0 otherwise. The exact reason is returned through /api/auth/config.sso_config_error.",
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
            "Web UI requests by status class and resolved role. `status_class` is 1xx/2xx/3xx/4xx/5xx (or `other` for non-standard codes). Pair with auth attempts to spot e.g. spikes in 4xx for the sso role (broken proxy) without wading through logs.",
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
