//! Pure collection functions for the REST API.
//!
//! Each function reads from project-wide global state (POOLS,
//! get_client_stats(), get_server_stats(), connection counters) and assembles
//! a Serializable DTO. No I/O, no Mutex acquisition outside what the global
//! reads already do internally.

use std::sync::atomic::AtomicUsize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::pool::get_all_pools;
use crate::stats::pool::PoolStats;
use crate::stats::{
    get_client_stats, get_server_stats, CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER,
    TLS_CONNECTION_COUNTER, TOTAL_CONNECTION_COUNTER,
};

use crate::web::routes::dto::{OverviewDto, PoolDto, PoolsDto, VersionDto};

fn cnt(counter: &AtomicUsize) -> u64 {
    counter.load(std::sync::atomic::Ordering::Relaxed) as u64
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn collect_version() -> VersionDto {
    VersionDto {
        version: env!("CARGO_PKG_VERSION"),
        git_commit: option_env!("PG_DOORMAN_GIT_COMMIT").unwrap_or("unknown"),
        build_date: option_env!("PG_DOORMAN_BUILD_DATE").unwrap_or("unknown"),
        ts: now_unix_ms(),
    }
}

pub fn collect_overview() -> OverviewDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let client_states = get_client_stats();
    let server_states = get_server_stats();

    let mut active_clients = 0u64;
    let mut idle_clients = 0u64;
    let mut waiting_clients = 0u64;
    for stats in client_states.values() {
        match stats.state_to_string().as_str() {
            "active" => active_clients += 1,
            "idle" => idle_clients += 1,
            "waiting" => waiting_clients += 1,
            _ => {}
        }
    }

    let mut active_servers = 0u64;
    let mut idle_servers = 0u64;
    for stats in server_states.values() {
        match stats.state_to_string().as_str() {
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
        prepared_hits_total += stats
            .prepared_hit_count
            .load(std::sync::atomic::Ordering::Relaxed);
        prepared_misses_total += stats
            .prepared_miss_count
            .load(std::sync::atomic::Ordering::Relaxed);
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
    }
}

pub fn collect_pools() -> PoolsDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let pools_map = get_all_pools();

    let mut pools = Vec::with_capacity(pool_lookup.len());
    for (identifier, stats) in pool_lookup.iter() {
        let Some(pool) = pools_map.get(identifier) else {
            continue;
        };
        let address = pool.address();
        let dto = PoolDto {
            id: format!("{}@{}", identifier.user, identifier.db),
            user: identifier.user.clone(),
            database: identifier.db.clone(),
            host: address.host.clone(),
            port: address.port,
            pool_mode: stats.mode.to_string(),
            max_connections: stats.pool_size,
            // min_pool_size lives on the per-user config; PoolSettings wraps it via settings.user.
            min_connections: pool.settings.user.min_pool_size.unwrap_or(0),
            connections: stats.sv_active + stats.sv_idle + stats.sv_used + stats.sv_login,
            idle: stats.sv_idle,
            active: stats.sv_active,
            waiting: stats.cl_waiting,
            max_active_age_ms: stats.oldest_active_age_ms,
            // Percentile fields are plain u64, not methods.
            query_p95_ms: stats.query_percentile.p95,
            query_p99_ms: stats.query_percentile.p99,
            transactions_p95_ms: stats.xact_percentile.p95,
            transactions_p99_ms: stats.xact_percentile.p99,
            wait_avg_ms: stats.avg_wait_time / 1_000, // micros -> ms
            wait_p95_ms: stats.wait_percentile.p95 / 1_000, // micros -> ms
            queries_total: stats.total_query_count,
            transactions_total: stats.total_xact_count,
            errors_total: stats.errors,
            paused: stats.paused,
            // TODO: epoch wiring in phase 3e (no epoch field on PoolSettings yet).
            epoch: 0,
        };
        pools.push(dto);
    }

    // Stable order for snapshot tests.
    pools.sort_by(|a, b| a.id.cmp(&b.id));

    PoolsDto {
        ts: now_unix_ms(),
        pools,
    }
}
