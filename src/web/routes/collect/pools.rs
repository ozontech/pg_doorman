use crate::pool::get_all_pools;
use crate::web::metrics::{
    FALLBACK_ACTIVE, SHOW_SERVER_TLS_CONNECTIONS, SHOW_SERVER_TLS_HANDSHAKE_ERRORS,
};
use crate::web::routes::dto::{PoolDto, PoolsDto, StartupParameterDto};

use super::{now_unix_ms, snapshot};

pub(crate) fn collect_pools(reveal_startup_values: bool) -> PoolsDto {
    let snap = snapshot();
    let pool_lookup = &snap.pool_lookup;
    let pools_map = get_all_pools();

    let mut pools = Vec::with_capacity(pool_lookup.len());
    for (identifier, stats) in pool_lookup.iter() {
        let Some(pool) = pools_map.get(identifier) else {
            continue;
        };
        let address = pool.address();
        let errors_by_sqlstate = address.stats.errors_by_sqlstate_snapshot();
        // The TLS / fallback metrics are written per database (the
        // Address::pool_name field, which mirrors the database segment of
        // `user@db`), so we read them with the same label. Every user@db
        // pool of the same database therefore reports the same value —
        // accurate for fallback (Patroni state is per database) and for
        // backend TLS counters that share one connection set per backend.
        let db_label = identifier.db.as_str();
        let fallback_active = FALLBACK_ACTIVE.with_label_values(&[db_label]).get() > 0.5;
        let tls_handshake_errors_total = SHOW_SERVER_TLS_HANDSHAKE_ERRORS
            .with_label_values(&[db_label])
            .get();
        let tls_backend_connections = SHOW_SERVER_TLS_CONNECTIONS
            .with_label_values(&[db_label])
            .get() as u64;
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
            // The HDR histograms underneath store microseconds; every DTO
            // field on `_ms` is divided by 1_000.0 in floating-point so
            // sub-millisecond percentiles do not collapse to zero — a
            // pool whose true p95 is 420 µs reports `0.42` rather than
            // `0` and matches the log line ("query_ms p95 = 0.42").
            query_p95_ms: stats.query_percentile.p95 as f64 / 1_000.0,
            query_p99_ms: stats.query_percentile.p99 as f64 / 1_000.0,
            transactions_p95_ms: stats.xact_percentile.p95 as f64 / 1_000.0,
            transactions_p99_ms: stats.xact_percentile.p99 as f64 / 1_000.0,
            wait_avg_ms: stats.avg_wait_time as f64 / 1_000.0,
            wait_p95_ms: stats.wait_percentile.p95 as f64 / 1_000.0,
            queries_total: stats.total_query_count,
            transactions_total: stats.total_xact_count,
            errors_total: stats.total_errors,
            errors_by_sqlstate,
            paused: stats.paused,
            // RECONNECT bumps the per-pool epoch; surfacing it lets a DBA
            // verify that a `RECONNECT db=...` rotated cached connections
            // (e.g. after `ALTER ROLE`, grant change, or TLS rotation).
            epoch: pool.database.reconnect_epoch() as u64,
            fallback_active,
            tls_handshake_errors_total,
            tls_backend_connections,
            startup_parameters: StartupParameterDto::from_resolved(
                pool.database.effective_startup_parameters_with_sources(),
                reveal_startup_values,
            ),
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
