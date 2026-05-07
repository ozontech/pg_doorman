use std::sync::atomic::Ordering;

use crate::web::routes::dto::{ServerDto, ServerFilters, ServerSort, ServersDto, SortOrder};

use super::{now_unix_ms, snapshot, MAX_LIMIT};

pub(crate) fn collect_servers(filters: &ServerFilters) -> ServersDto {
    let snap = snapshot();
    let servers: Vec<_> = snap.server_states.values().cloned().collect();
    collect_servers_from(servers, filters)
}

/// Pure inner logic for `collect_servers` — operates on a pre-built snapshot
/// so it can be called from unit tests without touching global state.
fn collect_servers_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ServerStats>>,
    filters: &ServerFilters,
) -> ServersDto {
    let mut rows: Vec<ServerDto> = snapshot
        .iter()
        .filter(|s| server_matches(s, filters))
        .map(server_to_dto)
        .collect();

    let total = rows.len() as u64;

    rows.sort_by(|a, b| {
        let ord = match filters.sort {
            ServerSort::AgeSeconds => a.age_seconds.cmp(&b.age_seconds),
            ServerSort::QueriesTotal => a.queries_total.cmp(&b.queries_total),
            ServerSort::ErrorsTotal => a.errors_total.cmp(&b.errors_total),
            ServerSort::ActiveAgeMs => a.active_age_ms.cmp(&b.active_age_ms),
        };
        match filters.order {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        }
    });

    let limit = filters.limit.clamp(1, MAX_LIMIT);
    let offset = filters.offset;
    let page: Vec<_> = rows
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    ServersDto {
        ts: now_unix_ms(),
        total,
        limit,
        offset,
        servers: page,
    }
}

fn server_matches(s: &crate::stats::ServerStats, f: &ServerFilters) -> bool {
    let pool_name = s.pool_name();
    let user = s.username();

    if let Some(p) = &f.pool {
        let id = format!("{}@{}", user, pool_name);
        if id != *p {
            return false;
        }
    }
    if let Some(db) = &f.database {
        if pool_name != *db {
            return false;
        }
    }
    if let Some(u) = &f.user {
        if user != *u {
            return false;
        }
    }
    true
}

fn server_to_dto(s: &std::sync::Arc<crate::stats::ServerStats>) -> ServerDto {
    let age_seconds = s.connect_time().elapsed().as_secs();
    let application_name = s.application_name();
    ServerDto {
        server_id: s.server_id(),
        process_id: s.process_id(),
        database: s.pool_name(),
        user: s.username(),
        application_name,
        tls: s.tls(),
        state: s.state_to_string(),
        wait: s.wait_to_string(),
        age_seconds,
        active_age_ms: s.active_age_ms().unwrap_or(0),
        transactions_total: s.transaction_count.load(Ordering::Relaxed),
        queries_total: s.query_count.load(Ordering::Relaxed),
        errors_total: s.error_count.load(Ordering::Relaxed),
        bytes_sent: s.bytes_sent.load(Ordering::Relaxed),
        bytes_received: s.bytes_received.load(Ordering::Relaxed),
        prepared_hits_total: s.prepared_hit_count.load(Ordering::Relaxed),
        prepared_misses_total: s.prepared_miss_count.load(Ordering::Relaxed),
        prepared_cache_size: s.prepared_cache_size.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::server::ServerStats;
    use crate::utils::clock;
    use std::sync::Arc;

    fn make_server(db: &str, user: &str) -> Arc<ServerStats> {
        let address = crate::config::Address {
            pool_name: db.to_string(),
            username: user.to_string(),
            ..crate::config::Address::default()
        };
        Arc::new(ServerStats::new(address, clock::now()))
    }

    fn default_server_filters() -> ServerFilters {
        ServerFilters {
            limit: 100,
            offset: 0,
            sort: ServerSort::AgeSeconds,
            order: SortOrder::Asc,
            pool: None,
            database: None,
            user: None,
        }
    }

    // ---------------------------------------------------------------------------
    // Server filter tests
    // ---------------------------------------------------------------------------

    #[test]
    fn server_filter_by_pool() {
        let servers = vec![make_server("db1", "alice"), make_server("db2", "bob")];
        let mut f = default_server_filters();
        f.pool = Some("alice@db1".to_string());
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.servers[0].database, "db1");
    }

    #[test]
    fn server_filter_by_database() {
        let servers = vec![
            make_server("prod", "alice"),
            make_server("staging", "alice"),
        ];
        let mut f = default_server_filters();
        f.database = Some("prod".to_string());
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.servers[0].database, "prod");
    }

    #[test]
    fn server_filter_by_user() {
        let servers = vec![make_server("db", "alice"), make_server("db", "bob")];
        let mut f = default_server_filters();
        f.user = Some("alice".to_string());
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.servers[0].user, "alice");
    }

    // ---------------------------------------------------------------------------
    // Server sort tests
    // ---------------------------------------------------------------------------

    #[test]
    fn server_sort_queries_total_asc() {
        let servers = vec![
            make_server("db", "u"),
            make_server("db", "u"),
            make_server("db", "u"),
        ];
        servers[0].query_count.store(30, Ordering::Relaxed);
        servers[1].query_count.store(10, Ordering::Relaxed);
        servers[2].query_count.store(20, Ordering::Relaxed);
        let mut f = default_server_filters();
        f.sort = ServerSort::QueriesTotal;
        f.order = SortOrder::Asc;
        let result = collect_servers_from(servers, &f);
        let counts: Vec<u64> = result.servers.iter().map(|s| s.queries_total).collect();
        assert_eq!(counts, vec![10, 20, 30]);
    }

    #[test]
    fn server_sort_queries_total_desc() {
        let servers = vec![
            make_server("db", "u"),
            make_server("db", "u"),
            make_server("db", "u"),
        ];
        servers[0].query_count.store(30, Ordering::Relaxed);
        servers[1].query_count.store(10, Ordering::Relaxed);
        servers[2].query_count.store(20, Ordering::Relaxed);
        let mut f = default_server_filters();
        f.sort = ServerSort::QueriesTotal;
        f.order = SortOrder::Desc;
        let result = collect_servers_from(servers, &f);
        let counts: Vec<u64> = result.servers.iter().map(|s| s.queries_total).collect();
        assert_eq!(counts, vec![30, 20, 10]);
    }

    #[test]
    fn server_sort_errors_total_asc() {
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        servers[0].error_count.store(5, Ordering::Relaxed);
        servers[1].error_count.store(1, Ordering::Relaxed);
        let mut f = default_server_filters();
        f.sort = ServerSort::ErrorsTotal;
        f.order = SortOrder::Asc;
        let result = collect_servers_from(servers, &f);
        let errs: Vec<u64> = result.servers.iter().map(|s| s.errors_total).collect();
        assert_eq!(errs, vec![1, 5]);
    }

    #[test]
    fn server_sort_active_age_ms_desc() {
        // Servers not in ACTIVE state return active_age_ms == 0.
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        let mut f = default_server_filters();
        f.sort = ServerSort::ActiveAgeMs;
        f.order = SortOrder::Desc;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn server_sort_age_seconds_desc() {
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        let mut f = default_server_filters();
        f.sort = ServerSort::AgeSeconds;
        f.order = SortOrder::Desc;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 2);
    }

    // ---------------------------------------------------------------------------
    // Server pagination tests
    // ---------------------------------------------------------------------------

    #[test]
    fn server_pagination_offset_beyond_total_returns_empty() {
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        let mut f = default_server_filters();
        f.offset = 10;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 2);
        assert!(result.servers.is_empty());
    }

    #[test]
    fn server_pagination_limit_clamped_to_max_limit() {
        let servers: Vec<_> = (0..5).map(|_| make_server("db", "u")).collect();
        let mut f = default_server_filters();
        f.limit = MAX_LIMIT + 9999;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.limit, MAX_LIMIT);
    }
}
