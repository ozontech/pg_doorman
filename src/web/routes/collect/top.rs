use std::sync::atomic::Ordering;

use crate::pool::get_all_pools;
use crate::server::{anon_snapshot, named_snapshot};
use crate::stats::get_client_stats;
use crate::web::routes::dto::{
    TopClientBy, TopClientFilters, TopClientRowDto, TopClientsDto, TopPreparedBy, TopPreparedDto,
    TopPreparedFilters, TopPreparedRowDto, TopQueriesDto, TopQueryBy, TopQueryFilters,
    TopQueryRowDto,
};

use super::{clamp_top_clients_n, now_unix_ms};

pub(crate) fn collect_top_prepared(filters: &TopPreparedFilters) -> TopPreparedDto {
    let n = clamp_top_clients_n(filters.n);

    let mut rows: Vec<TopPreparedRowDto> = Vec::new();
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        for (hash, parse, count_used, kind, hits, misses) in cache.get_entries() {
            rows.push(TopPreparedRowDto {
                pool: identifier.to_string(),
                hash: hash.to_string(),
                name: parse.name.clone(),
                count_used,
                hits,
                misses,
                kind: kind.as_str().to_string(),
            });
        }
    }

    rows.sort_by(|a, b| match filters.by {
        TopPreparedBy::Hits => b.hits.cmp(&a.hits),
        TopPreparedBy::Misses => b.misses.cmp(&a.misses),
    });

    let prepared: Vec<_> = rows.into_iter().take(n as usize).collect();

    TopPreparedDto {
        ts: now_unix_ms(),
        by: filters.by.as_str().to_string(),
        n,
        prepared,
    }
}

pub(crate) fn collect_top_clients(filters: &TopClientFilters) -> TopClientsDto {
    let snapshot: Vec<_> = get_client_stats().values().cloned().collect();
    top_clients_from(snapshot, filters)
}

fn top_clients_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ClientStats>>,
    filters: &TopClientFilters,
) -> TopClientsDto {
    let n = clamp_top_clients_n(filters.n);

    let mut rows: Vec<TopClientRowDto> = snapshot
        .iter()
        .filter(|s| {
            if let Some(p) = &filters.pool {
                let id = format!("{}@{}", s.username(), s.pool_name());
                if id != *p {
                    return false;
                }
            }
            true
        })
        .map(|s| {
            let age_seconds = s.connect_time().elapsed().as_secs();
            let queries_total = s.query_count.load(Ordering::Relaxed);
            let errors_total = s.error_count.load(Ordering::Relaxed);
            let qps = queries_total as f64 / age_seconds.max(1) as f64;
            TopClientRowDto {
                client_id: format!("#c{}", s.connection_id()),
                application_name: s.application_name(),
                user: s.username(),
                database: s.pool_name(),
                addr: s.ipaddr(),
                age_seconds,
                queries_total,
                errors_total,
                qps,
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        // All Top-N sorts are descending — operators want busiest first.
        match filters.by {
            TopClientBy::Qps => b
                .qps
                .partial_cmp(&a.qps)
                .unwrap_or(std::cmp::Ordering::Equal),
            TopClientBy::Errors => b.errors_total.cmp(&a.errors_total),
            TopClientBy::Age => b.age_seconds.cmp(&a.age_seconds),
        }
    });

    let clients: Vec<_> = rows.into_iter().take(n as usize).collect();

    TopClientsDto {
        ts: now_unix_ms(),
        by: filters.by.as_str().to_string(),
        n,
        clients,
    }
}

pub(crate) fn collect_top_queries(filters: &TopQueryFilters) -> TopQueriesDto {
    let n = clamp_top_clients_n(filters.n);

    let mut rows: Vec<TopQueryRowDto> = Vec::new();

    for (hash, entry) in named_snapshot() {
        let count = entry.count();
        let total_duration_us = entry.total_duration_us();
        let avg_duration_ms = if count == 0 {
            0.0
        } else {
            total_duration_us as f64 / count as f64 / 1_000.0
        };
        let preview: String = entry.text().chars().take(120).collect();
        rows.push(TopQueryRowDto {
            hash: format!("{:#x}", hash),
            kind: "named".to_string(),
            query: preview,
            count,
            total_duration_us,
            avg_duration_ms,
        });
    }
    for (hash, entry) in anon_snapshot() {
        let count = entry.count();
        let total_duration_us = entry.total_duration_us();
        let avg_duration_ms = if count == 0 {
            0.0
        } else {
            total_duration_us as f64 / count as f64 / 1_000.0
        };
        let preview: String = entry.text().chars().take(120).collect();
        rows.push(TopQueryRowDto {
            hash: format!("{:#x}", hash),
            kind: "anonymous".to_string(),
            query: preview,
            count,
            total_duration_us,
            avg_duration_ms,
        });
    }

    rows.sort_by(|a, b| match filters.by {
        TopQueryBy::Count => b.count.cmp(&a.count),
        TopQueryBy::Duration => b
            .avg_duration_ms
            .partial_cmp(&a.avg_duration_ms)
            .unwrap_or(std::cmp::Ordering::Equal),
    });

    let queries: Vec<_> = rows.into_iter().take(n as usize).collect();

    TopQueriesDto {
        ts: now_unix_ms(),
        by: filters.by.as_str().to_string(),
        n,
        queries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::client::ClientStats;
    use crate::utils::clock;
    use crate::web::routes::dto::TopClientBy;
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
    fn top_clients_sort_by_errors_desc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 5),
            make_client(2, "db", "u", "a", 0, 1),
            make_client(3, "db", "u", "a", 0, 3),
        ];
        let f = TopClientFilters {
            by: TopClientBy::Errors,
            n: 10,
            pool: None,
        };
        let result = top_clients_from(clients, &f);
        let errs: Vec<u64> = result.clients.iter().map(|c| c.errors_total).collect();
        assert_eq!(errs, vec![5, 3, 1]);
        assert_eq!(result.by, "errors");
    }

    #[test]
    fn top_clients_n_default_when_zero() {
        let clients: Vec<_> = (0..5)
            .map(|i| make_client(i, "db", "u", "a", 0, 0))
            .collect();
        let f = TopClientFilters {
            by: TopClientBy::Qps,
            n: 0,
            pool: None,
        };
        let result = top_clients_from(clients, &f);
        assert_eq!(result.n, 20);
    }

    #[test]
    fn top_clients_pool_filter_excludes_others() {
        let clients = vec![
            make_client(1, "db1", "alice", "a", 0, 0),
            make_client(2, "db2", "bob", "a", 0, 0),
        ];
        let f = TopClientFilters {
            by: TopClientBy::Qps,
            n: 10,
            pool: Some("alice@db1".to_string()),
        };
        let result = top_clients_from(clients, &f);
        assert_eq!(result.clients.len(), 1);
        assert_eq!(result.clients[0].user, "alice");
    }
}
