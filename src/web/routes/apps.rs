//! GET /api/apps?sort=&order= handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_apps;
use crate::web::routes::dto::{AppFilters, AppSort, SortOrder};
use crate::web::routes::query::first;
use crate::web::server::Response;

pub(crate) fn handle_apps(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = AppFilters {
        sort: match first(query, "sort").as_deref() {
            Some("queries") => AppSort::Queries,
            Some("transactions") => AppSort::Transactions,
            Some("errors") => AppSort::Errors,
            _ => AppSort::Clients,
        },
        order: match first(query, "order").as_deref() {
            Some("asc") => SortOrder::Asc,
            _ => SortOrder::Desc,
        },
    };
    Response::ok_json(&collect_apps(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_apps(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"apps\""));
    }
}
