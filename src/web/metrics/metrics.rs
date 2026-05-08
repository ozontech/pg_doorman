//! Metrics update functions for Prometheus exporter.

#[cfg(target_os = "linux")]
use log::error;
use once_cell::sync::Lazy;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::pool::{PoolIdentifier, AUTH_QUERY_STATE, COORDINATORS, DYNAMIC_POOLS};
#[cfg(target_os = "linux")]
use crate::stats::get_socket_states_count;
use crate::stats::pool::PoolStats;
use crate::stats::{
    CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};

use super::system::get_process_memory_usage;
#[cfg(target_os = "linux")]
use super::SHOW_SOCKETS;
use super::{
    AUTH_QUERY_AUTH, AUTH_QUERY_CACHE, AUTH_QUERY_DYNAMIC_POOLS, AUTH_QUERY_EXECUTOR, COORDINATOR,
    COORDINATOR_TOTALS, POOL_SCALING_GAUGE, POOL_SCALING_TOTALS, SHOW_ASYNC_CLIENTS_COUNT,
    SHOW_CLIENT_CACHE_BYTES, SHOW_CLIENT_CACHE_ENTRIES, SHOW_CLIENT_PREPARED_ANONYMOUS_ENTRIES,
    SHOW_CLIENT_PREPARED_ANONYMOUS_EVICTIONS_TOTAL, SHOW_CLIENT_PREPARED_NAMED_ENTRIES,
    SHOW_CONNECTIONS, SHOW_CONNECTIONS_TOTAL, SHOW_POOLS_BYTES, SHOW_POOLS_BYTES_TOTAL,
    SHOW_POOLS_CLIENT, SHOW_POOLS_ERRORS_TOTAL, SHOW_POOLS_MAXWAIT_MICROSECONDS,
    SHOW_POOLS_OLDEST_ACTIVE_AGE_MS, SHOW_POOLS_PAUSED, SHOW_POOLS_QUERIES_COUNTER,
    SHOW_POOLS_QUERIES_PERCENTILE, SHOW_POOLS_QUERIES_TOTAL, SHOW_POOLS_QUERIES_TOTAL_TIME,
    SHOW_POOLS_SERVER, SHOW_POOLS_TRANSACTIONS_COUNTER, SHOW_POOLS_TRANSACTIONS_PERCENTILE,
    SHOW_POOLS_TRANSACTIONS_TOTAL, SHOW_POOLS_TRANSACTIONS_TOTAL_TIME, SHOW_POOLS_WAIT_TIME_AVG,
    SHOW_POOL_CACHE_BYTES, SHOW_POOL_CACHE_ENTRIES, SHOW_POOL_SIZE, SHOW_SERVERS_PREPARED_HITS,
    SHOW_SERVERS_PREPARED_MISSES, SHOW_SERVER_TLS_CONNECTIONS, TOTAL_MEMORY,
};

/// Updates all metrics before they are exposed via the Prometheus endpoint.
pub fn update_metrics() {
    update_memory_metrics();
    update_connection_metrics();

    #[cfg(target_os = "linux")]
    update_socket_metrics();

    update_pool_metrics();
    update_pool_errors_metrics();
    update_server_metrics();
    update_auth_query_metrics();
    update_coordinator_metrics();
    update_pool_scaling_metrics();
}

fn update_memory_metrics() {
    TOTAL_MEMORY.set(get_process_memory_usage() as f64);
}

fn update_connection_metrics() {
    let connection_types = [
        ("plain", &PLAIN_CONNECTION_COUNTER),
        ("tls", &TLS_CONNECTION_COUNTER),
        ("cancel", &CANCEL_CONNECTION_COUNTER),
        ("total", &TOTAL_CONNECTION_COUNTER),
    ];

    for (conn_type, counter) in &connection_types {
        let value = counter.load(Ordering::Relaxed);
        SHOW_CONNECTIONS
            .with_label_values(&[conn_type])
            .set(value as f64);
        emit_process_counter_delta(
            &SHOW_CONNECTIONS_TOTAL.with_label_values(&[conn_type]),
            value as u64,
        );
    }
}

/// Bumps a Prometheus counter from a process-lifetime monotonic source
/// (a `static AtomicUsize` that survives RELOAD). Reads
/// `counter.get()` as the previous mark, so MUST NOT be used for any
/// source that can shrink mid-process — pool stats are wiped on
/// `Pool::from_config`. See `CounterDeltaTracker` for those.
#[inline]
fn emit_process_counter_delta(counter: &prometheus::IntCounter, value: u64) {
    let prev = counter.get();
    if value > prev {
        counter.inc_by(value - prev);
    }
}

/// Per-source delta tracker for counters whose source can be replaced
/// during the process lifetime: every `AddressStats::default()` minted
/// by `ConnectionPool::from_config` reopens its `total_*` counters at
/// zero. Holding our own previous-value map per metric label lets the
/// exporter detect that reset (`current < previous`) and emit the
/// current cumulative as the post-reset delta exactly once instead
/// of either freezing — the failure mode if we treat the Prometheus
/// counter as the previous mark, since the counter never goes back —
/// or fabricating duplicates each scrape.
struct CounterDeltaTracker<K> {
    prev: std::sync::Mutex<std::collections::HashMap<K, u64>>,
}

impl<K> CounterDeltaTracker<K>
where
    K: std::cmp::Eq + std::hash::Hash + Clone,
{
    fn new() -> Self {
        Self {
            prev: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Records `current` as the latest source value for `key` and
    /// increments `counter` by the appropriate delta. On source reset
    /// (the new `current` is below the previous mark) the post-reset
    /// `current` is emitted once and the tracker realigns.
    fn observe(&self, counter: &prometheus::IntCounter, key: K, current: u64) {
        let mut prev = match self.prev.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let last = prev.get(&key).copied().unwrap_or(0);
        let delta = if current >= last {
            current - last
        } else {
            current
        };
        if delta > 0 {
            counter.inc_by(delta);
        }
        prev.insert(key, current);
    }

    /// Drops entries whose keys are not in `current_keys`, returning
    /// them so the caller can issue `remove_label_values` on the
    /// owning `IntCounterVec`.
    fn drain_stale(&self, current_keys: &std::collections::HashSet<K>) -> Vec<K> {
        let mut prev = match self.prev.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let stale: Vec<K> = prev
            .keys()
            .filter(|k| !current_keys.contains(*k))
            .cloned()
            .collect();
        for key in &stale {
            prev.remove(key);
        }
        stale
    }
}

type PoolKey = (String, String);
type PoolBytesKey = (String, String, String);

static POOL_QUERIES_PREV: Lazy<CounterDeltaTracker<PoolKey>> = Lazy::new(CounterDeltaTracker::new);
static POOL_TRANSACTIONS_PREV: Lazy<CounterDeltaTracker<PoolKey>> =
    Lazy::new(CounterDeltaTracker::new);
static POOL_BYTES_PREV: Lazy<CounterDeltaTracker<PoolBytesKey>> =
    Lazy::new(CounterDeltaTracker::new);

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
    // Reuse the shared 250 ms snapshot — codex Arch P2#6 / Perf P2#6.
    // /metrics scrapes typically arrive every 15-30 s, but during an
    // incident the SPA can be polling /api/* on the same listener at
    // 1.5 s. Without the cache both paths each cloned CLIENT_STATS and
    // SERVER_STATS under their own read lock; with the cache they
    // share one snapshot whenever they fall inside the TTL.
    let snap = crate::web::routes::collect::snapshot();
    reset_pool_metrics();

    for (identifier, stats) in snap.pool_lookup.iter() {
        update_pool_avg_metrics(identifier, stats);
        update_pool_server_metrics(identifier, stats);
        update_client_state_metrics(identifier, stats);
        update_byte_metrics(identifier, stats);
        update_percentile_metrics(identifier, stats);
        update_pool_cache_metrics(identifier, stats);
        update_pool_size_metrics(identifier, stats);
        update_pool_state_metrics(identifier, stats);
    }

    // Drop label sets for the resettable `_total` counters whose
    // pools disappeared on RELOAD. The counter values themselves
    // never go backwards in Prometheus; this only removes the time
    // series so a later pool with the same `(user, database)` does
    // not inherit a non-zero `previous` mark from the tracker.
    let current_pool_keys: std::collections::HashSet<PoolKey> = snap
        .pool_lookup
        .keys()
        .map(|id| (id.user.clone(), id.db.clone()))
        .collect();

    for stale in POOL_QUERIES_PREV.drain_stale(&current_pool_keys) {
        let _ = SHOW_POOLS_QUERIES_TOTAL.remove_label_values(&[&stale.0, &stale.1]);
    }
    for stale in POOL_TRANSACTIONS_PREV.drain_stale(&current_pool_keys) {
        let _ = SHOW_POOLS_TRANSACTIONS_TOTAL.remove_label_values(&[&stale.0, &stale.1]);
    }

    let mut current_bytes_keys: std::collections::HashSet<PoolBytesKey> =
        std::collections::HashSet::with_capacity(snap.pool_lookup.len() * 2);
    for id in snap.pool_lookup.keys() {
        current_bytes_keys.insert(("received".to_string(), id.user.clone(), id.db.clone()));
        current_bytes_keys.insert(("sent".to_string(), id.user.clone(), id.db.clone()));
    }
    for stale in POOL_BYTES_PREV.drain_stale(&current_bytes_keys) {
        let _ = SHOW_POOLS_BYTES_TOTAL.remove_label_values(&[&stale.0, &stale.1, &stale.2]);
    }
}

fn update_pool_state_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    let user = identifier.user.as_str();
    let database = identifier.db.as_str();
    SHOW_POOLS_PAUSED
        .with_label_values(&[user, database])
        .set(if stats.paused { 1 } else { 0 });
    SHOW_POOLS_MAXWAIT_MICROSECONDS
        .with_label_values(&[user, database])
        .set(stats.maxwait as f64);
}

fn update_pool_cache_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    let user = identifier.user.as_str();
    let database = identifier.db.as_str();

    // Pool-level prepared statement cache metrics
    SHOW_POOL_CACHE_ENTRIES
        .with_label_values(&[user, database])
        .set(stats.prepared_statements_count as f64);
    SHOW_POOL_CACHE_BYTES
        .with_label_values(&[user, database])
        .set(stats.prepared_statements_bytes as f64);

    // Client-level prepared statement cache metrics (aggregated)
    SHOW_CLIENT_CACHE_ENTRIES
        .with_label_values(&[user, database])
        .set(stats.client_prepared_count as f64);
    SHOW_CLIENT_CACHE_BYTES
        .with_label_values(&[user, database])
        .set(stats.client_prepared_bytes as f64);
    SHOW_CLIENT_PREPARED_NAMED_ENTRIES
        .with_label_values(&[user, database])
        .set(stats.client_named_count as f64);
    SHOW_CLIENT_PREPARED_ANONYMOUS_ENTRIES
        .with_label_values(&[user, database])
        .set(stats.client_anonymous_count as f64);

    // Anonymous LRU evictions are no longer aggregated here. The IntCounter
    // is bumped at the eviction site via `observe_anonymous_eviction`, which
    // survives client disconnect — the previous polling-delta approach lost
    // history when an alive client's contribution dropped from the
    // PoolStats sum.

    SHOW_ASYNC_CLIENTS_COUNT
        .with_label_values(&[user, database])
        .set(stats.async_clients_count as f64);
}

/// Increment the cumulative Anonymous LRU eviction counter for a single
/// (user, database) pair.
///
/// Called from the eviction site (`process_parse_immediate`) so the counter
/// is monotonic across client disconnects — the IntCounterVec entry lives in
/// the global Prometheus registry, not in any per-client state.
#[inline]
pub fn observe_anonymous_eviction(user: &str, database: &str) {
    SHOW_CLIENT_PREPARED_ANONYMOUS_EVICTIONS_TOTAL
        .with_label_values(&[user, database])
        .inc();
}

fn update_server_metrics() {
    SHOW_SERVERS_PREPARED_HITS.reset();
    SHOW_SERVERS_PREPARED_MISSES.reset();
    SHOW_SERVER_TLS_CONNECTIONS.reset();
    // Same snapshot the rest of the scrape used; falls back to a
    // direct global read if the cache is somehow empty (e.g. the
    // first scrape racing with TTL expiry).
    let snap = crate::web::routes::collect::snapshot();

    // Aggregate hits and misses per (user, database) across all backends.
    // The PID-level breakdown lived here historically but exploded the
    // cardinality once `server_lifetime` expired the first generation
    // of backends — every reconnect minted a fresh PID label that
    // Prometheus then carried for the staleness window.
    let mut totals: std::collections::HashMap<(String, String), (f64, f64)> =
        std::collections::HashMap::new();

    for server in snap.server_states.values() {
        let username = server.username();
        let pool_name = server.pool_name();

        let entry = totals
            .entry((username.to_string(), pool_name.to_string()))
            .or_insert((0.0, 0.0));
        entry.0 += server.prepared_hit_count.load(Ordering::Relaxed) as f64;
        entry.1 += server.prepared_miss_count.load(Ordering::Relaxed) as f64;

        // Count TLS-encrypted backend connections per pool.
        if server.tls() {
            SHOW_SERVER_TLS_CONNECTIONS
                .with_label_values(&[pool_name])
                .inc();
        }
    }

    for ((user, database), (hits, misses)) in totals {
        SHOW_SERVERS_PREPARED_HITS
            .with_label_values(&[user.as_str(), database.as_str()])
            .set(hits);
        SHOW_SERVERS_PREPARED_MISSES
            .with_label_values(&[user.as_str(), database.as_str()])
            .set(misses);
    }
}

fn update_pool_avg_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
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

    let user = identifier.user.as_str();
    let database = identifier.db.as_str();
    for (metric, value) in &avg_metrics {
        metric.with_label_values(&[user, database]).set(*value);
    }

    // Counter-form mirrors of the two monotonic gauges above. The
    // wait-time average and the *_total_time gauges are not
    // counter-shaped (rolling averages and summed durations are still
    // surfaced through histograms), so only query/transaction counts
    // get a parallel `_total`. Pool stats live across at most one
    // RELOAD: `Pool::from_config` mints a fresh `AddressStats` whose
    // counters start at zero. The dedicated trackers detect that and
    // emit the post-reset delta instead of either freezing the metric
    // or fabricating duplicates each scrape.
    let pool_key = (user.to_string(), database.to_string());
    POOL_QUERIES_PREV.observe(
        &SHOW_POOLS_QUERIES_TOTAL.with_label_values(&[user, database]),
        pool_key.clone(),
        stats.total_query_count,
    );
    POOL_TRANSACTIONS_PREV.observe(
        &SHOW_POOLS_TRANSACTIONS_TOTAL.with_label_values(&[user, database]),
        pool_key,
        stats.total_xact_count,
    );
}

fn update_pool_server_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    let server_states = [("active", stats.sv_active), ("idle", stats.sv_idle)];

    for (state, value) in server_states {
        let labels: [&str; 3] = [state, identifier.user.as_str(), identifier.db.as_str()];
        SHOW_POOLS_SERVER
            .with_label_values(&labels)
            .set(value as f64);
    }

    SHOW_POOLS_OLDEST_ACTIVE_AGE_MS
        .with_label_values(&[identifier.user.as_str(), identifier.db.as_str()])
        .set(stats.oldest_active_age_ms as f64);
}

fn reset_pool_metrics() {
    SHOW_POOLS_CLIENT.reset();
    SHOW_POOLS_SERVER.reset();
    SHOW_POOLS_OLDEST_ACTIVE_AGE_MS.reset();
    SHOW_POOLS_BYTES.reset();
    SHOW_POOLS_QUERIES_PERCENTILE.reset();
    SHOW_POOLS_TRANSACTIONS_PERCENTILE.reset();
    SHOW_POOLS_WAIT_TIME_AVG.reset();
    SHOW_POOLS_TRANSACTIONS_COUNTER.reset();
    SHOW_POOLS_TRANSACTIONS_TOTAL_TIME.reset();
    SHOW_POOLS_QUERIES_COUNTER.reset();
    SHOW_POOLS_QUERIES_TOTAL_TIME.reset();
    SHOW_POOL_SIZE.reset();
    SHOW_POOLS_PAUSED.reset();
    SHOW_POOLS_MAXWAIT_MICROSECONDS.reset();
}

fn update_pool_size_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    SHOW_POOL_SIZE
        .with_label_values(&[identifier.user.as_str(), identifier.db.as_str()])
        .set(stats.pool_size as f64);
}

fn update_client_state_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    let states = [
        ("idle", stats.cl_idle),
        ("waiting", stats.cl_waiting),
        ("active", stats.cl_active),
    ];

    for (state, count) in states {
        let labels: [&str; 3] = [state, identifier.user.as_str(), identifier.db.as_str()];
        SHOW_POOLS_CLIENT
            .with_label_values(&labels)
            .set(count as f64);
    }
}

fn update_byte_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    let user = identifier.user.as_str();
    let database = identifier.db.as_str();

    SHOW_POOLS_BYTES
        .with_label_values(&["received", user, database])
        .set(stats.bytes_received as f64);
    SHOW_POOLS_BYTES
        .with_label_values(&["sent", user, database])
        .set(stats.bytes_sent as f64);

    POOL_BYTES_PREV.observe(
        &SHOW_POOLS_BYTES_TOTAL.with_label_values(&["received", user, database]),
        (
            "received".to_string(),
            user.to_string(),
            database.to_string(),
        ),
        stats.bytes_received,
    );
    POOL_BYTES_PREV.observe(
        &SHOW_POOLS_BYTES_TOTAL.with_label_values(&["sent", user, database]),
        ("sent".to_string(), user.to_string(), database.to_string()),
        stats.bytes_sent,
    );
}

fn update_percentile_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    const PERCENTILES: &[&str] = &["99", "95", "90", "50"];

    for percentile in PERCENTILES {
        let (query_value, xact_value) = match *percentile {
            "99" => (stats.query_percentile.p99, stats.xact_percentile.p99),
            "95" => (stats.query_percentile.p95, stats.xact_percentile.p95),
            "90" => (stats.query_percentile.p90, stats.xact_percentile.p90),
            "50" => (stats.query_percentile.p50, stats.xact_percentile.p50),
            _ => continue,
        };

        let labels_q: [&str; 3] = [percentile, identifier.user.as_str(), identifier.db.as_str()];
        SHOW_POOLS_QUERIES_PERCENTILE
            .with_label_values(&labels_q)
            .set(query_value as f64 / 1_000f64);

        let labels_x: [&str; 3] = [percentile, identifier.user.as_str(), identifier.db.as_str()];
        SHOW_POOLS_TRANSACTIONS_PERCENTILE
            .with_label_values(&labels_x)
            .set(xact_value as f64 / 1_000f64);
    }
}

fn update_auth_query_metrics() {
    AUTH_QUERY_CACHE.reset();
    AUTH_QUERY_AUTH.reset();
    AUTH_QUERY_EXECUTOR.reset();
    AUTH_QUERY_DYNAMIC_POOLS.reset();

    let states = AUTH_QUERY_STATE.load();
    if states.is_empty() {
        return;
    }

    let dynamic = DYNAMIC_POOLS.load();

    for (pool_name, state) in states.iter() {
        let db = pool_name.as_str();
        let s = state.stats.snapshot();

        // Cache metrics
        AUTH_QUERY_CACHE
            .with_label_values(&["entries", db])
            .set(state.cache_len() as f64);
        AUTH_QUERY_CACHE
            .with_label_values(&["hits", db])
            .set(s.cache_hits as f64);
        AUTH_QUERY_CACHE
            .with_label_values(&["misses", db])
            .set(s.cache_misses as f64);
        AUTH_QUERY_CACHE
            .with_label_values(&["refetches", db])
            .set(s.cache_refetches as f64);
        AUTH_QUERY_CACHE
            .with_label_values(&["rate_limited", db])
            .set(s.cache_rate_limited as f64);

        // Auth outcomes
        AUTH_QUERY_AUTH
            .with_label_values(&["success", db])
            .set(s.auth_success as f64);
        AUTH_QUERY_AUTH
            .with_label_values(&["failure", db])
            .set(s.auth_failure as f64);

        // Executor metrics
        AUTH_QUERY_EXECUTOR
            .with_label_values(&["queries", db])
            .set(s.executor_queries as f64);
        AUTH_QUERY_EXECUTOR
            .with_label_values(&["errors", db])
            .set(s.executor_errors as f64);

        // Dynamic pool metrics
        let dyn_current = dynamic.iter().filter(|id| id.db == *pool_name).count();
        AUTH_QUERY_DYNAMIC_POOLS
            .with_label_values(&["current", db])
            .set(dyn_current as f64);
        AUTH_QUERY_DYNAMIC_POOLS
            .with_label_values(&["created", db])
            .set(s.dynamic_pools_created as f64);
        AUTH_QUERY_DYNAMIC_POOLS
            .with_label_values(&["destroyed", db])
            .set(s.dynamic_pools_destroyed as f64);
    }
}

/// Previous coordinator state — tracks Arc pointers to detect replacements
/// and database set to clean up stale label combinations on RELOAD.
static COORDINATOR_PREV: Lazy<std::sync::Mutex<std::collections::HashMap<String, usize>>> =
    Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn reset_coordinator_counters(db: &str) {
    for t in [
        "connections",
        "reserve_in_use",
        "max_connections",
        "reserve_pool_size",
    ] {
        let _ = COORDINATOR.remove_label_values(&[t, db]);
    }
    for t in ["evictions", "reserve_acquisitions", "exhaustions"] {
        let _ = COORDINATOR_TOTALS.remove_label_values(&[t, db]);
    }
}

fn update_coordinator_metrics() {
    let coordinators = COORDINATORS.load();

    let current: std::collections::HashMap<String, usize> = coordinators
        .iter()
        .map(|(db, arc)| (db.clone(), Arc::as_ptr(arc) as usize))
        .collect();

    if let Ok(mut prev) = COORDINATOR_PREV.lock() {
        // Remove counters for databases no longer coordinated
        for old_db in prev.keys() {
            if !current.contains_key(old_db) {
                reset_coordinator_counters(old_db);
            }
        }
        // Reset counters for databases where coordinator was replaced (new Arc)
        for (db, new_ptr) in &current {
            if let Some(old_ptr) = prev.get(db) {
                if old_ptr != new_ptr {
                    reset_coordinator_counters(db);
                }
            }
        }
        *prev = current;
    }

    for (db, coordinator) in coordinators.iter() {
        let stats = coordinator.stats();
        let config = coordinator.config();

        COORDINATOR
            .with_label_values(&["connections", db])
            .set(stats.total_connections as f64);
        COORDINATOR
            .with_label_values(&["reserve_in_use", db])
            .set(stats.reserve_in_use as f64);
        COORDINATOR
            .with_label_values(&["max_connections", db])
            .set(config.max_db_connections as f64);
        COORDINATOR
            .with_label_values(&["reserve_pool_size", db])
            .set(config.reserve_pool_size as f64);

        let evictions_counter = COORDINATOR_TOTALS.with_label_values(&["evictions", db]);
        let delta = stats
            .evictions_total
            .saturating_sub(evictions_counter.get());
        if delta > 0 {
            evictions_counter.inc_by(delta);
        }

        let reserve_counter = COORDINATOR_TOTALS.with_label_values(&["reserve_acquisitions", db]);
        let delta = stats
            .reserve_acquisitions_total
            .saturating_sub(reserve_counter.get());
        if delta > 0 {
            reserve_counter.inc_by(delta);
        }

        let exhaustions_counter = COORDINATOR_TOTALS.with_label_values(&["exhaustions", db]);
        let delta = stats
            .exhaustions_total
            .saturating_sub(exhaustions_counter.get());
        if delta > 0 {
            exhaustions_counter.inc_by(delta);
        }
    }
}

/// (type, user, db) → last observed counter value. Used by the scaling
/// totals exporter so it can emit `inc_by(delta)` rather than overwrite a
/// monotonic counter, and so it can drop stale label combinations on pool
/// removal.
type PoolScalingPrev = std::collections::HashMap<(String, String, String), u64>;

static POOL_SCALING_PREV: Lazy<std::sync::Mutex<PoolScalingPrev>> =
    Lazy::new(|| std::sync::Mutex::new(PoolScalingPrev::new()));

const POOL_SCALING_TOTAL_TYPES: &[&str] = &[
    "creates_started",
    "burst_gate_waits",
    "burst_gate_budget_exhausted",
    "anticipation_wakes_notify",
    "anticipation_wakes_timeout",
    "create_fallback",
    "replenish_deferred",
];

fn reset_pool_scaling_metrics(user: &str, database: &str) {
    let _ = POOL_SCALING_GAUGE.remove_label_values(&["inflight_creates", user, database]);
    for t in POOL_SCALING_TOTAL_TYPES {
        let _ = POOL_SCALING_TOTALS.remove_label_values(&[*t, user, database]);
    }
}

fn update_pool_scaling_metrics() {
    use crate::pool::get_all_pools;

    // One mutex acquisition for both delta emission and stale-label cleanup.
    // The lock spans the iteration but each per-pool body is a few atomic
    // loads plus HashMap lookups, so the critical section stays microsecond-scale.
    let Ok(mut prev) = POOL_SCALING_PREV.lock() else {
        return;
    };

    let mut current: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();

    for (identifier, pool) in get_all_pools().iter() {
        let user = identifier.user.as_str();
        let database = identifier.db.as_str();
        current.insert((user.to_string(), database.to_string()));

        let snapshot = pool.database.scaling_stats();

        POOL_SCALING_GAUGE
            .with_label_values(&["inflight_creates", user, database])
            .set(snapshot.inflight_creates as f64);

        // Translate snapshot fields to (type_label, value) pairs and emit
        // them as monotonic counter deltas. Pools have unique (user, db) keys
        // so prev tracking is per (type, user, db).
        let totals: [(&str, u64); 7] = [
            ("creates_started", snapshot.creates_started),
            ("burst_gate_waits", snapshot.burst_gate_waits),
            (
                "burst_gate_budget_exhausted",
                snapshot.burst_gate_budget_exhausted,
            ),
            (
                "anticipation_wakes_notify",
                snapshot.anticipation_wakes_notify,
            ),
            (
                "anticipation_wakes_timeout",
                snapshot.anticipation_wakes_timeout,
            ),
            ("create_fallback", snapshot.create_fallback),
            ("replenish_deferred", snapshot.replenish_deferred),
        ];

        for (label, value) in totals {
            let key = (label.to_string(), user.to_string(), database.to_string());
            let prev_value = prev.get(&key).copied().unwrap_or(0);
            let delta = value.saturating_sub(prev_value);
            if delta > 0 {
                POOL_SCALING_TOTALS
                    .with_label_values(&[label, user, database])
                    .inc_by(delta);
            }
            prev.insert(key, value);
        }
    }

    // Drop stale labels for pools that have disappeared since the last
    // scrape. Order matters: collect the (user, db) pairs to reset BEFORE
    // removing them from `prev`, otherwise we lose the information needed
    // to clear the corresponding Prometheus labels and they linger forever.
    let stale_pairs: std::collections::HashSet<(String, String)> = prev
        .keys()
        .filter(|(_, user, db)| !current.contains(&(user.clone(), db.clone())))
        .map(|(_, user, db)| (user.clone(), db.clone()))
        .collect();

    for (user, db) in &stale_pairs {
        reset_pool_scaling_metrics(user, db);
    }

    prev.retain(|(_, user, db), _| !stale_pairs.contains(&(user.clone(), db.clone())));
}

/// Maps a 5-character canonical SQLSTATE code to its bounded label
/// class. The output is a `'static` string so it can flow into
/// `IntCounterVec::with_label_values` without an allocation. Classes
/// follow Postgres SQLSTATE chapters: `08` is connection_exception,
/// `53` is insufficient_resources, `57` is operator_intervention.
/// `25P02` and `26000` keep their full code because each one points at
/// a distinct misuse pattern — a transaction-scope leak under session
/// pooling and an evicted anonymous prepared statement, respectively.
/// Anything else collapses into `other` so the label space stays
/// constant regardless of what the backend returns.
fn classify_sqlstate(code: &str) -> &'static str {
    match code {
        "25P02" => "25P02",
        "26000" => "26000",
        c if c.starts_with("08") => "08",
        c if c.starts_with("53") => "53",
        c if c.starts_with("57") => "57",
        _ => "other",
    }
}

/// (user, database, sqlstate-class) key for the per-pool error
/// counter delta tracker. The class is `&'static str` because
/// `classify_sqlstate` returns one of six interned literals.
type PoolErrorsKey = (String, String, &'static str);

static POOL_ERRORS_PREV: Lazy<CounterDeltaTracker<PoolErrorsKey>> =
    Lazy::new(CounterDeltaTracker::new);

fn update_pool_errors_metrics() {
    use crate::pool::get_all_pools;

    // Aggregate per-pool snapshots into bounded sqlstate classes.
    // `errors_by_sqlstate_snapshot` returns canonical 5-character codes
    // (the address-side increment validates them); classify_sqlstate
    // collapses each one into the {08, 53, 57, 25P02, 26000, other}
    // bucket the counter exposes.
    let mut current_sums: std::collections::HashMap<PoolErrorsKey, u64> =
        std::collections::HashMap::new();

    for (identifier, pool) in get_all_pools().iter() {
        let user = identifier.user.as_str();
        let database = identifier.db.as_str();

        let snap = pool.address().stats.errors_by_sqlstate_snapshot();
        for (code, count) in snap {
            let class = classify_sqlstate(&code);
            let key = (user.to_string(), database.to_string(), class);
            *current_sums.entry(key).or_insert(0) += count;
        }
    }

    // Drive the tracker — it knows the previous source value per
    // (user, database, class) tuple and detects address replacement
    // (current < previous) by emitting `current` once. Crucially it
    // then updates its own previous-mark, so the next scrape with the
    // same `current` produces a zero delta. The earlier implementation
    // used `IntCounter::get()` as the previous mark; that wedged each
    // scrape into another fabricated increment because the Prometheus
    // counter never goes backwards.
    for (key, &current_sum) in &current_sums {
        let counter =
            SHOW_POOLS_ERRORS_TOTAL.with_label_values(&[key.0.as_str(), key.1.as_str(), key.2]);
        POOL_ERRORS_PREV.observe(&counter, key.clone(), current_sum);
    }

    // Drop labels for triples that disappeared since the last scrape —
    // typically a pool removed by RELOAD. `remove_label_values` is
    // best-effort; ignoring its return is intentional.
    let current_keys: std::collections::HashSet<PoolErrorsKey> =
        current_sums.keys().cloned().collect();
    for stale in POOL_ERRORS_PREV.drain_stale(&current_keys) {
        let _ = SHOW_POOLS_ERRORS_TOTAL.remove_label_values(&[
            stale.0.as_str(),
            stale.1.as_str(),
            stale.2,
        ]);
    }
}

/// Called by the GC tokio task on every sweep tick. Updates the interner
/// gauges (entries, bytes per kind), increments eviction counters, and
/// observes the sweep duration in the histogram. The byte totals come
/// straight from `GcStats` so we don't traverse the DashMaps a second
/// time after the sweep already walked them.
pub fn record_interner_gc(
    named: crate::server::GcStats,
    anon: crate::server::GcStats,
    elapsed_seconds: f64,
) {
    super::QUERY_INTERNER_EVICTIONS_TOTAL
        .with_label_values(&["named", "gc_passive"])
        .inc_by(named.evicted);
    super::QUERY_INTERNER_EVICTIONS_TOTAL
        .with_label_values(&["anonymous", "ttl_expired"])
        .inc_by(anon.evicted);

    super::QUERY_INTERNER_ENTRIES
        .with_label_values(&["named"])
        .set(crate::server::named_len() as i64);
    super::QUERY_INTERNER_ENTRIES
        .with_label_values(&["anonymous"])
        .set(crate::server::anon_len() as i64);
    super::QUERY_INTERNER_BYTES
        .with_label_values(&["named"])
        .set(named.bytes as i64);
    super::QUERY_INTERNER_BYTES
        .with_label_values(&["anonymous"])
        .set(anon.bytes as i64);

    super::QUERY_INTERNER_GC_DURATION_SECONDS.observe(elapsed_seconds);
}

/// Increments the synthetic-miss counter — called from the protocol
/// path when pg_doorman returns SQLSTATE 26000 to a client because the
/// anonymous prepared statement it referred to is no longer in any of
/// the caches (interner, pool cache, client cache).
pub fn record_synthetic_miss() {
    super::QUERY_INTERNER_SYNTHETIC_MISSES_TOTAL.inc();
}

/// Records one large-message streaming event. Called from
/// `handle_large_data_row` and `handle_large_copy_data` after the
/// outcome is known. `kind` is "data_row" or "copy_data"; `result` is
/// "ok" or "error".
#[inline]
pub fn observe_streaming_event(user: &str, database: &str, kind: &str, result: &str) {
    super::STREAMING_EVENTS_TOTAL
        .with_label_values(&[user, database, kind, result])
        .inc();
}

/// Records bytes forwarded for one streaming event. Called once per
/// event, before the result is known, so the bytes flowed during a
/// failed stream are still counted.
#[inline]
pub fn observe_streaming_bytes(user: &str, database: &str, kind: &str, bytes: u64) {
    super::STREAMING_BYTES_TOTAL
        .with_label_values(&[user, database, kind])
        .inc_by(bytes);
}

/// Records one client rejection at the listener / pre-auth stage.
/// `reason` must be one of the labels documented on
/// `LISTENER_REJECTIONS_TOTAL`; passing any other value still works but
/// inflates the cardinality the metric was designed to bound.
#[inline]
pub fn record_listener_rejection(reason: &'static str) {
    super::LISTENER_REJECTIONS_TOTAL
        .with_label_values(&[reason])
        .inc();
}

/// Observes wall-clock duration of one backend connection setup phase.
/// `phase` must be one of `tcp_connect`, `tls`, `auth`, `startup` —
/// passing any other value still works but breaks the cardinality
/// contract documented on `BACKEND_CREATE_DURATION_SECONDS`.
#[inline]
pub fn observe_backend_create_phase(phase: &'static str, seconds: f64) {
    super::BACKEND_CREATE_DURATION_SECONDS
        .with_label_values(&[phase])
        .observe(seconds);
}

/// Observes one query duration in the per-pool query histogram. Caller
/// passes microseconds because every existing call site already has
/// that unit; the conversion to seconds happens once here, behind the
/// inline call, so the hot path stays one division and one
/// `with_label_values` lookup.
#[inline]
pub fn observe_pool_query_microseconds(user: &str, database: &str, microseconds: u64) {
    super::SHOW_POOLS_QUERY_DURATION_SECONDS
        .with_label_values(&[user, database])
        .observe(microseconds as f64 / 1_000_000.0);
}

/// Observes one transaction duration in the per-pool transaction
/// histogram. Mirrors `AddressStats::xact_time_add` and silently drops
/// zero-microsecond inputs: the `idle(0)` and `add_xact_time_and_idle(0)`
/// call sites fire on backend creation and on `Drop for Client` without
/// representing a real transaction, and recording them would pull the
/// histogram's lowest bucket toward background noise.
#[inline]
pub fn observe_pool_transaction_microseconds(user: &str, database: &str, microseconds: u64) {
    if microseconds == 0 {
        return;
    }
    super::SHOW_POOLS_TRANSACTION_DURATION_SECONDS
        .with_label_values(&[user, database])
        .observe(microseconds as f64 / 1_000_000.0);
}

/// Observes one client checkout wait in the per-pool wait histogram.
/// Same unit-conversion contract as `observe_pool_query_microseconds`.
#[inline]
pub fn observe_pool_wait_microseconds(user: &str, database: &str, microseconds: u64) {
    super::SHOW_POOLS_WAIT_DURATION_SECONDS
        .with_label_values(&[user, database])
        .observe(microseconds as f64 / 1_000_000.0);
}

/// Refreshes the trio of static info gauges: `build_info` (constant
/// version label), `users_configured` (one series per (user, database,
/// pool_mode) triple from the active config), and `log_level` (current
/// effective filter from `app::log_level::get_log_level`). Called on
/// startup and on every config reload — `BUILD_INFO` is idempotent,
/// the other two are reset + repopulated so disappeared pools and
/// rolled-back log overrides drop their series straight away.
pub fn refresh_static_info_metrics() {
    super::BUILD_INFO
        .with_label_values(&[env!("CARGO_PKG_VERSION")])
        .set(1);

    super::USERS_CONFIGURED.reset();
    let config = crate::config::get_config();
    for (database, pool) in &config.pools {
        let pool_mode = pool.pool_mode.to_string();
        for user in &pool.users {
            super::USERS_CONFIGURED
                .with_label_values(&[
                    user.username.as_str(),
                    database.as_str(),
                    pool_mode.as_str(),
                ])
                .set(1);
        }
    }

    super::LOG_LEVEL_INFO.reset();
    let level = crate::app::log_level::get_log_level();
    super::LOG_LEVEL_INFO
        .with_label_values(&[level.as_str()])
        .set(1);
}

#[cfg(test)]
mod tests {
    use super::classify_sqlstate;

    #[test]
    fn class_08_collapses_connection_exception_codes() {
        assert_eq!(classify_sqlstate("08000"), "08");
        assert_eq!(classify_sqlstate("08003"), "08");
        assert_eq!(classify_sqlstate("08006"), "08");
        assert_eq!(classify_sqlstate("08P01"), "08");
    }

    #[test]
    fn class_53_collapses_insufficient_resources_codes() {
        assert_eq!(classify_sqlstate("53000"), "53");
        assert_eq!(classify_sqlstate("53100"), "53");
        assert_eq!(classify_sqlstate("53200"), "53");
        assert_eq!(classify_sqlstate("53300"), "53");
    }

    #[test]
    fn class_57_collapses_operator_intervention_codes() {
        assert_eq!(classify_sqlstate("57000"), "57");
        assert_eq!(classify_sqlstate("57014"), "57");
        assert_eq!(classify_sqlstate("57P01"), "57");
        assert_eq!(classify_sqlstate("57P03"), "57");
    }

    #[test]
    fn special_codes_keep_their_full_value() {
        assert_eq!(classify_sqlstate("25P02"), "25P02");
        assert_eq!(classify_sqlstate("26000"), "26000");
    }

    #[test]
    fn unknown_codes_collapse_into_other() {
        assert_eq!(classify_sqlstate("23505"), "other");
        assert_eq!(classify_sqlstate("42P01"), "other");
        assert_eq!(classify_sqlstate("XX000"), "other");
        assert_eq!(classify_sqlstate(""), "other");
    }

    #[test]
    fn other_class_25_codes_do_not_steal_25p02_label() {
        assert_eq!(classify_sqlstate("25000"), "other");
        assert_eq!(classify_sqlstate("25001"), "other");
        assert_eq!(classify_sqlstate("25P01"), "other");
    }

    #[test]
    fn pool_transaction_observe_drops_zero_microseconds() {
        // idle(0) and add_xact_time_and_idle(0) fire on backend
        // creation and on `Drop for Client`; recording those zeros
        // would tug the lowest bucket toward background noise.
        let user = "zero_obs_user";
        let database = "zero_obs_db";
        let child = super::super::SHOW_POOLS_TRANSACTION_DURATION_SECONDS
            .with_label_values(&[user, database]);

        let before = child.get_sample_count();
        super::observe_pool_transaction_microseconds(user, database, 0);
        assert_eq!(child.get_sample_count(), before);

        super::observe_pool_transaction_microseconds(user, database, 1);
        assert_eq!(child.get_sample_count(), before + 1);
    }

    #[test]
    fn pool_query_observe_records_zero_microseconds() {
        // query_time_add_microseconds historically records zero-elapsed
        // queries (sub-microsecond ones) — keep parity here so the
        // gauge family and the histogram see the same denominator.
        let user = "zero_q_user";
        let database = "zero_q_db";
        let child =
            super::super::SHOW_POOLS_QUERY_DURATION_SECONDS.with_label_values(&[user, database]);

        let before = child.get_sample_count();
        super::observe_pool_query_microseconds(user, database, 0);
        assert_eq!(child.get_sample_count(), before + 1);
    }

    #[test]
    fn pool_wait_observe_records_zero_microseconds() {
        // Zero-length checkouts are the healthy-pool baseline; keep
        // recording them so histogram_quantile reflects "checkout was
        // instant" instead of going silent on a healthy pool.
        let user = "zero_w_user";
        let database = "zero_w_db";
        let child =
            super::super::SHOW_POOLS_WAIT_DURATION_SECONDS.with_label_values(&[user, database]);

        let before = child.get_sample_count();
        super::observe_pool_wait_microseconds(user, database, 0);
        assert_eq!(child.get_sample_count(), before + 1);
    }

    #[test]
    fn counter_delta_tracker_emits_post_reset_delta_only_once() {
        // Reproduces the codex review HIGH 1 / HIGH 2 scenario: a
        // pool whose `AddressStats` was replaced by `Pool::from_config`
        // reports a smaller cumulative value than what the Prometheus
        // counter holds. The tracker must increment by `current` once
        // and a follow-up scrape with the same `current` must not
        // fabricate another delta.
        let tracker: super::CounterDeltaTracker<&'static str> = super::CounterDeltaTracker::new();
        let counter =
            prometheus::IntCounter::new("pg_doorman_test_reset_counter", "test counter").unwrap();

        tracker.observe(&counter, "key", 1_000);
        assert_eq!(counter.get(), 1_000, "first observe sets the baseline");

        tracker.observe(&counter, "key", 10);
        assert_eq!(
            counter.get(),
            1_010,
            "source reset emits `current` as the post-reset delta",
        );

        tracker.observe(&counter, "key", 10);
        assert_eq!(
            counter.get(),
            1_010,
            "the same source value on the next scrape must not produce more increments",
        );

        tracker.observe(&counter, "key", 15);
        assert_eq!(
            counter.get(),
            1_015,
            "subsequent growth advances by the actual delta",
        );
    }

    #[test]
    fn counter_delta_tracker_drain_stale_forgets_removed_keys() {
        // After a key disappears from the active scrape (pool removed
        // by RELOAD), the tracker must forget its previous mark so a
        // future pool that reuses the same `(user, database)` does not
        // inherit a non-zero baseline.
        let tracker: super::CounterDeltaTracker<&'static str> = super::CounterDeltaTracker::new();
        let counter =
            prometheus::IntCounter::new("pg_doorman_test_stale_counter", "test counter").unwrap();

        tracker.observe(&counter, "alive", 5);
        tracker.observe(&counter, "stale", 7);

        let mut current = std::collections::HashSet::new();
        current.insert("alive");
        let stale = tracker.drain_stale(&current);
        assert_eq!(stale, vec!["stale"]);

        // After drain the tracker no longer remembers `stale`; observing
        // a fresh value treats it as a brand-new key.
        tracker.observe(&counter, "stale", 100);
        assert_eq!(
            counter.get(),
            5 + 7 + 100,
            "drained keys are forgotten — re-observing emits from zero",
        );
    }
}
