//! Metrics update functions for Prometheus exporter.

#[cfg(target_os = "linux")]
use log::error;
use std::sync::atomic::Ordering;

use crate::pool::PoolIdentifier;
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
    SHOW_CONNECTIONS, SHOW_POOLS_BYTES, SHOW_POOLS_CLIENT, SHOW_POOLS_QUERIES_COUNTER,
    SHOW_POOLS_QUERIES_PERCENTILE, SHOW_POOLS_QUERIES_TOTAL_TIME, SHOW_POOLS_SERVER,
    SHOW_POOLS_TRANSACTIONS_COUNTER, SHOW_POOLS_TRANSACTIONS_PERCENTILE,
    SHOW_POOLS_TRANSACTIONS_TOTAL_TIME, SHOW_POOLS_WAIT_TIME_AVG, SHOW_SERVERS_PREPARED_HITS,
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
