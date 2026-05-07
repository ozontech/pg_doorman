//! GET /api/servers handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_servers;
use crate::web::routes::dto::{ServerFilters, ServerSort, SortOrder};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_servers(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = parse_filters(query);
    Response::ok_json(&collect_servers(&filters))
}

fn parse_filters(query: &BTreeMap<String, Vec<String>>) -> ServerFilters {
    ServerFilters {
        limit: parse_u64(query, "limit", 100),
        offset: parse_u64(query, "offset", 0),
        sort: match first(query, "sort").as_deref() {
            Some("queries_total") => ServerSort::QueriesTotal,
            Some("errors_total") => ServerSort::ErrorsTotal,
            Some("active_age_ms") => ServerSort::ActiveAgeMs,
            _ => ServerSort::AgeSeconds,
        },
        order: match first(query, "order").as_deref() {
            Some("asc") => SortOrder::Asc,
            _ => SortOrder::Desc,
        },
        pool: first(query, "pool"),
        database: first(query, "database"),
        user: first(query, "user"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_response_is_200_json_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_servers(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"total\"",
            "\"limit\"",
            "\"offset\"",
            "\"servers\"",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }
}
