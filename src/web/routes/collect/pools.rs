use crate::pool::get_all_pools;
use crate::stats::pool::PoolStats;
use crate::web::routes::dto::{PoolDto, PoolsDto};

use super::now_unix_ms;

pub(crate) fn collect_pools() -> PoolsDto {
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
