use std::sync::atomic::Ordering;

use crate::web::routes::dto::{AppFilters, AppRowDto, AppSort, AppsDto, SortOrder};

use super::{now_unix_ms, snapshot};

pub(crate) fn collect_apps(filters: &AppFilters) -> AppsDto {
    let snap = snapshot();
    let clients: Vec<_> = snap.client_states.values().cloned().collect();
    apps_from(clients, filters)
}

fn apps_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ClientStats>>,
    filters: &AppFilters,
) -> AppsDto {
    use std::collections::HashMap;

    let mut acc: HashMap<String, AppRowDto> = HashMap::new();
    for s in &snapshot {
        let app = s.application_name();
        let entry = acc.entry(app.clone()).or_insert_with(|| AppRowDto {
            application_name: app,
            clients: 0,
            queries_total: 0,
            transactions_total: 0,
            errors_total: 0,
        });
        entry.clients += 1;
        entry.queries_total += s.query_count.load(Ordering::Relaxed);
        entry.transactions_total += s.transaction_count.load(Ordering::Relaxed);
        entry.errors_total += s.error_count.load(Ordering::Relaxed);
    }

    let mut apps: Vec<AppRowDto> = acc.into_values().collect();
    apps.sort_by(|a, b| {
        let ord = match filters.sort {
            AppSort::Clients => a.clients.cmp(&b.clients),
            AppSort::Queries => a.queries_total.cmp(&b.queries_total),
            AppSort::Transactions => a.transactions_total.cmp(&b.transactions_total),
            AppSort::Errors => a.errors_total.cmp(&b.errors_total),
        };
        match filters.order {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        }
    });

    AppsDto {
        ts: now_unix_ms(),
        apps,
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

    #[test]
    fn apps_aggregate_counts_clients_per_application_name() {
        let clients = vec![
            make_client(1, "db", "u", "appA", 10, 0),
            make_client(2, "db", "u", "appA", 20, 0),
            make_client(3, "db", "u", "appB", 5, 0),
        ];
        let f = AppFilters {
            sort: AppSort::Clients,
            order: SortOrder::Desc,
        };
        let result = apps_from(clients, &f);
        let app_a = result
            .apps
            .iter()
            .find(|a| a.application_name == "appA")
            .unwrap();
        let app_b = result
            .apps
            .iter()
            .find(|a| a.application_name == "appB")
            .unwrap();
        assert_eq!(app_a.clients, 2);
        assert_eq!(app_a.queries_total, 30);
        assert_eq!(app_b.clients, 1);
        assert_eq!(app_b.queries_total, 5);
    }

    #[test]
    fn apps_sort_by_queries_desc() {
        let clients = vec![
            make_client(1, "db", "u", "appA", 10, 0),
            make_client(2, "db", "u", "appB", 100, 0),
            make_client(3, "db", "u", "appC", 50, 0),
        ];
        let f = AppFilters {
            sort: AppSort::Queries,
            order: SortOrder::Desc,
        };
        let result = apps_from(clients, &f);
        let names: Vec<_> = result
            .apps
            .iter()
            .map(|a| a.application_name.clone())
            .collect();
        assert_eq!(names, vec!["appB", "appC", "appA"]);
    }
}
