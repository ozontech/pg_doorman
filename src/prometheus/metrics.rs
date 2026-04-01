//! Metrics update functions for Prometheus exporter.

#[cfg(target_os = "linux")]
use log::error;
use once_cell::sync::Lazy;
use std::sync::atomic::Ordering;

use crate::pool::{PoolIdentifier, AUTH_QUERY_STATE, COORDINATORS, DYNAMIC_POOLS};
#[cfg(target_os = "linux")]
use crate::stats::get_socket_states_count;
use crate::stats::pool::PoolStats;
use crate::stats::{
    get_server_stats, CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};

use super::system::get_process_memory_usage;
#[cfg(target_os = "linux")]
use super::SHOW_SOCKETS;
use super::{
    AUTH_QUERY_AUTH, AUTH_QUERY_CACHE, AUTH_QUERY_DYNAMIC_POOLS, AUTH_QUERY_EXECUTOR, COORDINATOR,
    COORDINATOR_TOTALS, SHOW_ASYNC_CLIENTS_COUNT, SHOW_CLIENT_CACHE_BYTES,
    SHOW_CLIENT_CACHE_ENTRIES, SHOW_CONNECTIONS, SHOW_POOLS_BYTES, SHOW_POOLS_CLIENT,
    SHOW_POOLS_QUERIES_COUNTER, SHOW_POOLS_QUERIES_PERCENTILE, SHOW_POOLS_QUERIES_TOTAL_TIME,
    SHOW_POOLS_SERVER, SHOW_POOLS_TRANSACTIONS_COUNTER, SHOW_POOLS_TRANSACTIONS_PERCENTILE,
    SHOW_POOLS_TRANSACTIONS_TOTAL_TIME, SHOW_POOLS_WAIT_TIME_AVG, SHOW_POOL_CACHE_BYTES,
    SHOW_POOL_CACHE_ENTRIES, SHOW_POOL_SIZE, SHOW_SERVERS_PREPARED_HITS,
    SHOW_SERVERS_PREPARED_MISSES, TOTAL_MEMORY,
};

/// Updates all metrics before they are exposed via the Prometheus endpoint.
pub fn update_metrics() {
    update_memory_metrics();
    update_connection_metrics();

    #[cfg(target_os = "linux")]
    update_socket_metrics();

    update_pool_metrics();
    update_server_metrics();
    update_auth_query_metrics();
    update_coordinator_metrics();
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
        update_pool_cache_metrics(identifier, stats);
        update_pool_size_metrics(identifier, stats);
    }
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
    SHOW_ASYNC_CLIENTS_COUNT
        .with_label_values(&[user, database])
        .set(stats.async_clients_count as f64);
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

    for (metric, value) in &avg_metrics {
        metric
            .with_label_values(&[identifier.user.as_str(), identifier.db.as_str()])
            .set(*value);
    }
}

fn update_pool_server_metrics(identifier: &PoolIdentifier, stats: &PoolStats) {
    let server_states = [("active", stats.sv_active), ("idle", stats.sv_idle)];

    for (state, value) in server_states {
        let labels: [&str; 3] = [state, identifier.user.as_str(), identifier.db.as_str()];
        SHOW_POOLS_SERVER
            .with_label_values(&labels)
            .set(value as f64);
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
    SHOW_POOL_SIZE.reset();
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
    let labels_recv: [&str; 3] = ["received", identifier.user.as_str(), identifier.db.as_str()];
    SHOW_POOLS_BYTES
        .with_label_values(&labels_recv)
        .set(stats.bytes_received as f64);
    let labels_sent: [&str; 3] = ["sent", identifier.user.as_str(), identifier.db.as_str()];
    SHOW_POOLS_BYTES
        .with_label_values(&labels_sent)
        .set(stats.bytes_sent as f64);
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

/// Previous coordinator database set — used to clean up stale label combinations
/// when a database's coordinator is removed on RELOAD.
static COORDINATOR_PREV_DBS: Lazy<std::sync::Mutex<std::collections::HashSet<String>>> =
    Lazy::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

fn update_coordinator_metrics() {
    let coordinators = COORDINATORS.load();

    let current_dbs: std::collections::HashSet<String> = coordinators.keys().cloned().collect();

    // Remove stale label combinations for databases no longer coordinated.
    if let Ok(mut prev) = COORDINATOR_PREV_DBS.lock() {
        for old_db in prev.difference(&current_dbs) {
            let db: &str = old_db.as_str();
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
        *prev = current_dbs;
    }

    for (db, coordinator) in coordinators.iter() {
        let stats = coordinator.stats();
        let config = coordinator.config();

        // Current state (gauges)
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

        // Cumulative counters — increment by delta since last scrape.
        // IntCounter only supports inc(), so we compute the difference
        // from the current Prometheus value to the coordinator's total.
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
