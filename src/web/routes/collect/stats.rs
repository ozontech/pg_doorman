use crate::web::routes::dto::{StatsDto, StatsRowDto};

use super::{now_unix_ms, snapshot};

pub(crate) fn collect_stats() -> StatsDto {
    let snap = snapshot();
    let mut stats: Vec<StatsRowDto> = snap
        .pool_lookup
        .iter()
        .map(|(identifier, s)| StatsRowDto {
            id: format!("{}@{}", identifier.user, identifier.db),
            database: identifier.db.clone(),
            user: identifier.user.clone(),
            total_xact_count: s.total_xact_count,
            total_query_count: s.total_query_count,
            total_received: s.total_received,
            total_sent: s.total_sent,
            total_xact_time: s.total_xact_time_microseconds,
            total_query_time: s.total_query_time_microseconds,
            total_wait_time: s.wait_time,
            total_errors: s.total_errors,
            avg_xact_count: s.avg_xact_count,
            avg_query_count: s.avg_query_count,
            avg_recv: s.avg_recv,
            avg_sent: s.avg_sent,
            avg_errors: s.avg_errors,
            avg_xact_time: s.avg_xact_time_microsecons,
            avg_query_time: s.avg_query_time_microseconds,
            avg_wait_time: s.avg_wait_time,
        })
        .collect();

    // Stable order: same `id` ordering as `/api/pools` for deterministic UI.
    stats.sort_by(|a, b| a.id.cmp(&b.id));

    StatsDto {
        ts: now_unix_ms(),
        stats,
    }
}
