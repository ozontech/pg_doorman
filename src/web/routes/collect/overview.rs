use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::app::server::{
    CLIENTS_IN_TRANSACTIONS, CURRENT_CLIENT_COUNT, MIGRATION_IN_PROGRESS, SHUTDOWN_IN_PROGRESS,
    STARTED_AT,
};
use crate::pool::PoolIdentifier;
use crate::stats::pool::PoolStats;
use crate::stats::{
    CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};
use crate::web::metrics::system::get_process_memory_usage;
use crate::web::routes::dto::{HottestDatabaseDto, OverviewDto};

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
        // Sum of per-pool cumulative error counters across all pools.
        errors_count_total: pool_lookup.values().map(|s| s.total_errors).sum(),

        prepared_hits_total,
        prepared_misses_total,

        pools_total: pool_lookup.len() as u64,
        pools_paused,

        hottest_database: compute_hottest_database(pool_lookup),

        rss_bytes: get_process_memory_usage(),
        uptime_seconds: STARTED_AT.elapsed().map(|d| d.as_secs()).unwrap_or(0),
        started_at_ms: STARTED_AT
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        pid: std::process::id(),
        current_clients: CURRENT_CLIENT_COUNT.load(Ordering::Relaxed),
        clients_in_transactions: CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed),
        shutdown_in_progress: SHUTDOWN_IN_PROGRESS.load(Ordering::Relaxed),
        migration_in_progress: MIGRATION_IN_PROGRESS.load(Ordering::Relaxed),
    }
}

/// Pick the database with the highest live backend-connection count.
///
/// `total_connections` mirrors the per-pool `connections` field
/// (`sv_active + sv_idle + sv_used + sv_login`), summed across every
/// `<user>@<db>` pool that targets the database. `active_connections`
/// sums only `sv_active`. Returns `None` when no pool has a live
/// connection. Ties on `total_connections` resolve to the
/// lexicographically smallest database name so the result is stable
/// across snapshots even though `pool_lookup` iterates in
/// HashMap-undefined order.
fn compute_hottest_database(
    pool_lookup: &HashMap<PoolIdentifier, PoolStats>,
) -> Option<HottestDatabaseDto> {
    let mut by_db: HashMap<&str, (u64, u64)> = HashMap::new();
    for (id, stats) in pool_lookup {
        let total = stats.sv_active + stats.sv_idle + stats.sv_used + stats.sv_login;
        if total == 0 {
            continue;
        }
        let entry = by_db.entry(id.db.as_str()).or_insert((0, 0));
        entry.0 += total;
        entry.1 += stats.sv_active;
    }
    by_db
        .into_iter()
        .max_by(|(a_name, (a_total, _)), (b_name, (b_total, _))| {
            a_total.cmp(b_total).then_with(|| b_name.cmp(a_name))
        })
        .map(|(name, (total, active))| HottestDatabaseDto {
            name: name.to_string(),
            total_connections: total,
            active_connections: active,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PoolMode;
    use crate::stats::pool::Percentile;

    fn zero_percentile() -> Percentile {
        Percentile {
            p99: 0,
            p95: 0,
            p90: 0,
            p50: 0,
        }
    }

    fn pool_stats(
        db: &str,
        user: &str,
        sv_active: u64,
        sv_idle: u64,
        sv_used: u64,
        sv_login: u64,
    ) -> (PoolIdentifier, PoolStats) {
        let id = PoolIdentifier::new(db, user);
        let mut stats = PoolStats::new_with_percentiles(
            id.clone(),
            PoolMode::Transaction,
            zero_percentile(),
            zero_percentile(),
            zero_percentile(),
        );
        stats.sv_active = sv_active;
        stats.sv_idle = sv_idle;
        stats.sv_used = sv_used;
        stats.sv_login = sv_login;
        (id, stats)
    }

    #[test]
    fn empty_pool_lookup_returns_none() {
        let pool_lookup = HashMap::new();
        assert!(compute_hottest_database(&pool_lookup).is_none());
    }

    #[test]
    fn pools_with_zero_connections_are_skipped() {
        let mut pool_lookup = HashMap::new();
        let (id, stats) = pool_stats("idle_db", "u", 0, 0, 0, 0);
        pool_lookup.insert(id, stats);
        assert!(compute_hottest_database(&pool_lookup).is_none());
    }

    #[test]
    fn picks_database_with_largest_total() {
        let mut pool_lookup = HashMap::new();
        let (id_a, stats_a) = pool_stats("small", "u", 1, 1, 0, 0);
        let (id_b, stats_b) = pool_stats("big", "u", 5, 3, 1, 1);
        pool_lookup.insert(id_a, stats_a);
        pool_lookup.insert(id_b, stats_b);
        let hottest = compute_hottest_database(&pool_lookup).expect("hottest db");
        assert_eq!(hottest.name, "big");
        assert_eq!(hottest.total_connections, 10);
        assert_eq!(hottest.active_connections, 5);
    }

    #[test]
    fn aggregates_across_user_pools_of_the_same_database() {
        let mut pool_lookup = HashMap::new();
        let (id_w, stats_w) = pool_stats("shared_db", "writer", 3, 4, 0, 0);
        let (id_r, stats_r) = pool_stats("shared_db", "reader", 2, 6, 0, 0);
        let (id_o, stats_o) = pool_stats("other_db", "u", 8, 0, 0, 0);
        pool_lookup.insert(id_w, stats_w);
        pool_lookup.insert(id_r, stats_r);
        pool_lookup.insert(id_o, stats_o);
        let hottest = compute_hottest_database(&pool_lookup).expect("hottest db");
        // shared_db total = 3+4+2+6 = 15, beats other_db total = 8.
        assert_eq!(hottest.name, "shared_db");
        assert_eq!(hottest.total_connections, 15);
        assert_eq!(hottest.active_connections, 5);
    }

    #[test]
    fn ties_resolve_to_lexicographically_smallest_name() {
        let mut pool_lookup = HashMap::new();
        let (id_z, stats_z) = pool_stats("zeta", "u", 2, 3, 0, 0);
        let (id_a, stats_a) = pool_stats("alpha", "u", 2, 3, 0, 0);
        pool_lookup.insert(id_z, stats_z);
        pool_lookup.insert(id_a, stats_a);
        let hottest = compute_hottest_database(&pool_lookup).expect("hottest db");
        assert_eq!(hottest.name, "alpha");
        // Lock totals on the tie-break path to catch regressions where
        // the wrong row's aggregates leak through with the right name.
        assert_eq!(hottest.total_connections, 5);
        assert_eq!(hottest.active_connections, 2);
    }

    #[test]
    fn login_and_used_states_count_toward_total() {
        let mut pool_lookup = HashMap::new();
        // login_heavy holds backends only in transient states (login + used);
        // steady holds them as active+idle. Both should aggregate the same
        // way — the test pins login_heavy as the winner with total=8 and
        // confirms sv_login + sv_used are in the sum.
        let (id_t, stats_t) = pool_stats("login_heavy", "u", 0, 0, 4, 4);
        let (id_s, stats_s) = pool_stats("steady", "u", 2, 2, 0, 0);
        pool_lookup.insert(id_t, stats_t);
        pool_lookup.insert(id_s, stats_s);
        let hottest = compute_hottest_database(&pool_lookup).expect("hottest db");
        assert_eq!(hottest.name, "login_heavy");
        assert_eq!(hottest.total_connections, 8);
        assert_eq!(hottest.active_connections, 0);
    }
}
