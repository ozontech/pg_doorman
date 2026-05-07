use std::sync::atomic::Ordering;

use crate::web::routes::dto::{ClientDto, ClientFilters, ClientSort, ClientsDto, SortOrder};

use super::{now_unix_ms, snapshot, MAX_LIMIT};

pub(crate) fn collect_clients(filters: &ClientFilters) -> ClientsDto {
    let snap = snapshot();
    let clients: Vec<_> = snap.client_states.values().cloned().collect();
    collect_clients_from(clients, filters)
}

/// Pure inner logic for `collect_clients` — operates on a pre-built snapshot
/// so it can be called from unit tests without touching global state.
fn collect_clients_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ClientStats>>,
    filters: &ClientFilters,
) -> ClientsDto {
    let mut rows: Vec<ClientDto> = snapshot
        .iter()
        .filter(|s| client_matches(s, filters))
        .map(client_to_dto)
        .collect();

    let total = rows.len() as u64;

    rows.sort_by(|a, b| {
        let ord = match filters.sort {
            ClientSort::QueriesTotal => a.queries_total.cmp(&b.queries_total),
            ClientSort::ErrorsTotal => a.errors_total.cmp(&b.errors_total),
            ClientSort::AgeSeconds => a.age_seconds.cmp(&b.age_seconds),
            ClientSort::CurrentQueryAgeMs => a.current_query_age_ms.cmp(&b.current_query_age_ms),
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

    ClientsDto {
        ts: now_unix_ms(),
        total,
        limit,
        offset,
        clients: page,
    }
}

fn client_matches(s: &crate::stats::ClientStats, f: &ClientFilters) -> bool {
    let pool_name = s.pool_name();
    let user = s.username();
    let app = s.application_name();
    let state = s.state_str();

    if let Some(p) = &f.pool {
        let id = format!("{}@{}", user, pool_name);
        if id != *p {
            return false;
        }
    }
    if let Some(db) = &f.database {
        if pool_name != db {
            return false;
        }
    }
    if let Some(u) = &f.user {
        if user != u {
            return false;
        }
    }
    if let Some(a) = &f.addr {
        // Substring match — covers both "1.2.3.4" and "1.2.3.4:5432" forms,
        // and supports operator typing partial subnets like "10.0.5.".
        if !s.ipaddr().contains(a.as_str()) {
            return false;
        }
    }
    if !f.application_name.is_empty() && !f.application_name.iter().any(|x| x == app) {
        return false;
    }
    if !f.state.is_empty() && !f.state.iter().any(|x| x == state) {
        return false;
    }
    true
}

fn client_to_dto(s: &std::sync::Arc<crate::stats::ClientStats>) -> ClientDto {
    let age_seconds = s.connect_time().elapsed().as_secs();
    ClientDto {
        client_id: format!("#c{}", s.connection_id()),
        database: s.pool_name().to_string(),
        user: s.username().to_string(),
        application_name: s.application_name().to_string(),
        addr: s.ipaddr().to_string(),
        tls: s.tls(),
        state: s.state_str().to_string(),
        wait: s.wait_str().to_string(),
        wait_ms: s.wait_ms().unwrap_or(0),
        transactions_total: s.transaction_count.load(Ordering::Relaxed),
        queries_total: s.query_count.load(Ordering::Relaxed),
        errors_total: s.error_count.load(Ordering::Relaxed),
        age_seconds,
        current_query_age_ms: s.current_query_age_ms().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::client::ClientStats;
    use crate::utils::clock;
    use std::sync::Arc;

    fn make_client(
        connection_id: u64,
        db: &str,
        user: &str,
        app: &str,
        queries: u64,
        errors: u64,
    ) -> Arc<ClientStats> {
        let stats = Arc::new(ClientStats::new(
            connection_id,
            app,
            user,
            db,
            "127.0.0.1",
            clock::now(),
            false,
        ));
        stats.query_count.store(queries, Ordering::Relaxed);
        stats.error_count.store(errors, Ordering::Relaxed);
        stats
    }

    fn default_client_filters() -> ClientFilters {
        ClientFilters {
            limit: 100,
            offset: 0,
            sort: ClientSort::QueriesTotal,
            order: SortOrder::Asc,
            pool: None,
            database: None,
            user: None,
            addr: None,
            application_name: vec![],
            state: vec![],
        }
    }

    // ---------------------------------------------------------------------------
    // Client filter tests
    // ---------------------------------------------------------------------------

    #[test]
    fn client_filter_by_pool_exact_match() {
        // pool filter uses the "user@db" composite id.
        let clients = vec![
            make_client(1, "db1", "alice", "app", 0, 0),
            make_client(2, "db2", "bob", "app", 0, 0),
        ];
        let mut f = default_client_filters();
        f.pool = Some("alice@db1".to_string());
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.clients[0].user, "alice");
    }

    #[test]
    fn client_filter_by_database_only() {
        let clients = vec![
            make_client(1, "prod", "alice", "app", 0, 0),
            make_client(2, "staging", "alice", "app", 0, 0),
        ];
        let mut f = default_client_filters();
        f.database = Some("prod".to_string());
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.clients[0].database, "prod");
    }

    #[test]
    fn client_filter_by_user_only() {
        let clients = vec![
            make_client(1, "db", "alice", "app", 0, 0),
            make_client(2, "db", "bob", "app", 0, 0),
            make_client(3, "db", "alice", "app2", 0, 0),
        ];
        let mut f = default_client_filters();
        f.user = Some("alice".to_string());
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
        assert!(result.clients.iter().all(|c| c.user == "alice"));
    }

    #[test]
    fn client_filter_application_name_or_semantics() {
        // A row matches if its app_name is in the filter list (OR).
        let clients = vec![
            make_client(1, "db", "alice", "pgadmin", 0, 0),
            make_client(2, "db", "bob", "psql", 0, 0),
            make_client(3, "db", "carol", "other", 0, 0),
        ];
        let mut f = default_client_filters();
        f.application_name = vec!["pgadmin".to_string(), "psql".to_string()];
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
        let apps: Vec<_> = result
            .clients
            .iter()
            .map(|c| c.application_name.as_str())
            .collect();
        assert!(apps.contains(&"pgadmin"));
        assert!(apps.contains(&"psql"));
    }

    #[test]
    fn client_filter_state_or_semantics() {
        // Default state for a fresh ClientStats is "idle".
        let clients = vec![
            make_client(1, "db", "alice", "app", 0, 0),
            make_client(2, "db", "bob", "app", 0, 0),
        ];
        let mut f = default_client_filters();
        f.state = vec!["idle".to_string()];
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);

        // Filter for a state that no client is in returns nothing.
        f.state = vec!["active".to_string()];
        let clients2 = vec![make_client(10, "db", "alice", "app", 0, 0)];
        let result2 = collect_clients_from(clients2, &f);
        assert_eq!(result2.total, 0);
    }

    // ---------------------------------------------------------------------------
    // Client sort tests
    // ---------------------------------------------------------------------------

    #[test]
    fn client_sort_queries_total_asc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 30, 0),
            make_client(2, "db", "u", "a", 10, 0),
            make_client(3, "db", "u", "a", 20, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::QueriesTotal;
        f.order = SortOrder::Asc;
        let result = collect_clients_from(clients, &f);
        let counts: Vec<u64> = result.clients.iter().map(|c| c.queries_total).collect();
        assert_eq!(counts, vec![10, 20, 30]);
    }

    #[test]
    fn client_sort_queries_total_desc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 30, 0),
            make_client(2, "db", "u", "a", 10, 0),
            make_client(3, "db", "u", "a", 20, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::QueriesTotal;
        f.order = SortOrder::Desc;
        let result = collect_clients_from(clients, &f);
        let counts: Vec<u64> = result.clients.iter().map(|c| c.queries_total).collect();
        assert_eq!(counts, vec![30, 20, 10]);
    }

    #[test]
    fn client_sort_errors_total_asc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 5),
            make_client(2, "db", "u", "a", 0, 1),
            make_client(3, "db", "u", "a", 0, 3),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::ErrorsTotal;
        f.order = SortOrder::Asc;
        let result = collect_clients_from(clients, &f);
        let errs: Vec<u64> = result.clients.iter().map(|c| c.errors_total).collect();
        assert_eq!(errs, vec![1, 3, 5]);
    }

    #[test]
    fn client_sort_errors_total_desc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 5),
            make_client(2, "db", "u", "a", 0, 1),
            make_client(3, "db", "u", "a", 0, 3),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::ErrorsTotal;
        f.order = SortOrder::Desc;
        let result = collect_clients_from(clients, &f);
        let errs: Vec<u64> = result.clients.iter().map(|c| c.errors_total).collect();
        assert_eq!(errs, vec![5, 3, 1]);
    }

    #[test]
    fn client_sort_age_seconds_asc() {
        // All fixtures share the same clock::now() so ages will all be 0.
        // The sort should be stable and return all rows without panicking.
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 0),
            make_client(2, "db", "u", "a", 0, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::AgeSeconds;
        f.order = SortOrder::Asc;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn client_sort_current_query_age_ms_desc() {
        // Clients not in ACTIVE state return current_query_age_ms == 0.
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 0),
            make_client(2, "db", "u", "a", 0, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::CurrentQueryAgeMs;
        f.order = SortOrder::Desc;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
    }

    // ---------------------------------------------------------------------------
    // Client pagination tests
    // ---------------------------------------------------------------------------

    #[test]
    fn client_pagination_offset_beyond_total_returns_empty() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 0),
            make_client(2, "db", "u", "a", 0, 0),
        ];
        let mut f = default_client_filters();
        f.offset = 10; // beyond total of 2
        f.limit = 100;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
        assert!(result.clients.is_empty());
    }

    #[test]
    fn client_pagination_limit_clamped_to_max_limit() {
        let clients: Vec<_> = (0..5)
            .map(|i| make_client(i, "db", "u", "a", 0, 0))
            .collect();
        let mut f = default_client_filters();
        f.limit = MAX_LIMIT + 9999; // above cap
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.limit, MAX_LIMIT);
    }

    #[test]
    fn client_pagination_limit_one() {
        let clients: Vec<_> = (0..5)
            .map(|i| make_client(i, "db", "u", "a", 0, 0))
            .collect();
        let mut f = default_client_filters();
        f.limit = 1;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.clients.len(), 1);
        assert_eq!(result.total, 5);
    }
}
