use std::sync::atomic::Ordering;

use crate::app::server::{
    CLIENTS_IN_TRANSACTIONS, CURRENT_CLIENT_COUNT, MIGRATION_IN_PROGRESS, SHUTDOWN_IN_PROGRESS,
    STARTED_AT,
};
use crate::stats::{
    CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};
use crate::web::metrics::system::get_process_memory_usage;
use crate::web::routes::dto::OverviewDto;

use super::{cnt, now_unix_ms, snapshot};

pub(crate) fn collect_overview() -> OverviewDto {
    let snap = snapshot();
    let client_states = &snap.client_states;
    let server_states = &snap.server_states;
    let pool_lookup = &snap.pool_lookup;

    let mut active_clients = 0u64;
    let mut idle_clients = 0u64;
    let mut waiting_clients = 0u64;
    for stats in client_states.values() {
        match stats.state_str() {
            "active" => active_clients += 1,
            "idle" => idle_clients += 1,
            "waiting" => waiting_clients += 1,
            _ => {}
        }
    }

    let mut active_servers = 0u64;
    let mut idle_servers = 0u64;
    for stats in server_states.values() {
        match stats.state_str() {
            "active" => active_servers += 1,
            "idle" => idle_servers += 1,
            _ => {}
        }
    }

    let connections_total = cnt(&TOTAL_CONNECTION_COUNTER);
    let connections_tls_total = cnt(&TLS_CONNECTION_COUNTER);
    let connections_plain_total = cnt(&PLAIN_CONNECTION_COUNTER);
    let connections_cancel_total = cnt(&CANCEL_CONNECTION_COUNTER);

    let mut query_count_total = 0u64;
    let mut transaction_count_total = 0u64;
    let mut prepared_hits_total = 0u64;
    let mut prepared_misses_total = 0u64;
    let mut pools_paused = 0u64;
    for stats in pool_lookup.values() {
        query_count_total += stats.total_query_count;
        transaction_count_total += stats.total_xact_count;
        if stats.paused {
            pools_paused += 1;
        }
    }
    for stats in server_states.values() {
        prepared_hits_total += stats.prepared_hit_count.load(Ordering::Relaxed);
        prepared_misses_total += stats.prepared_miss_count.load(Ordering::Relaxed);
    }

    OverviewDto {
        ts: now_unix_ms(),

        active_clients,
        idle_clients,
        waiting_clients,

        active_servers,
        idle_servers,

        connections_total,
        connections_tls_total,
        connections_plain_total,
        connections_cancel_total,

        query_count_total,
        transaction_count_total,
        // Sum of per-pool error counters. Already populated by PoolStats.errors.
        errors_count_total: pool_lookup.values().map(|s| s.errors).sum(),

        prepared_hits_total,
        prepared_misses_total,

        pools_total: pool_lookup.len() as u64,
        pools_paused,

        rss_bytes: get_process_memory_usage(),
        uptime_seconds: STARTED_AT.elapsed().map(|d| d.as_secs()).unwrap_or(0),
        pid: std::process::id(),
        current_clients: CURRENT_CLIENT_COUNT.load(Ordering::Relaxed),
        clients_in_transactions: CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed),
        shutdown_in_progress: SHUTDOWN_IN_PROGRESS.load(Ordering::Relaxed),
        migration_in_progress: MIGRATION_IN_PROGRESS.load(Ordering::Relaxed),
    }
}
