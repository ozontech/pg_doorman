//! GET /api/clients handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_clients;
use crate::web::routes::dto::{ClientFilters, ClientSort, SortOrder};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_clients(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = parse_filters(query);
    Response::ok_json(&collect_clients(&filters))
}

fn parse_filters(query: &BTreeMap<String, Vec<String>>) -> ClientFilters {
    ClientFilters {
        limit: parse_u64(query, "limit", 100),
        offset: parse_u64(query, "offset", 0),
        sort: match first(query, "sort").as_deref() {
            Some("errors_total") => ClientSort::ErrorsTotal,
            Some("age_seconds") => ClientSort::AgeSeconds,
            Some("current_query_age_ms") => ClientSort::CurrentQueryAgeMs,
            _ => ClientSort::QueriesTotal,
        },
        order: match first(query, "order").as_deref() {
            Some("asc") => SortOrder::Asc,
            _ => SortOrder::Desc,
        },
        pool: first(query, "pool"),
        database: first(query, "database"),
        user: first(query, "user"),
        application_name: query.get("application_name").cloned().unwrap_or_default(),
        state: query.get("state").cloned().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_response_is_200_json_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_clients(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"total\"",
            "\"limit\"",
            "\"offset\"",
            "\"clients\"",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }

    #[test]
    fn parse_filters_picks_up_query_params() {
        let mut q = BTreeMap::new();
        q.insert("limit".into(), vec!["50".into()]);
        q.insert("sort".into(), vec!["errors_total".into()]);
        q.insert("order".into(), vec!["asc".into()]);
        q.insert("pool".into(), vec!["main@db1".into()]);
        let f = parse_filters(&q);
        assert_eq!(f.limit, 50);
        assert!(matches!(f.sort, ClientSort::ErrorsTotal));
        assert!(matches!(f.order, SortOrder::Asc));
        assert_eq!(f.pool.as_deref(), Some("main@db1"));
    }
}
