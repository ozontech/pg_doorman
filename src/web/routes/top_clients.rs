//! GET /api/top/clients?by=qps|errors|age&n=20&pool= handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_top_clients;
use crate::web::routes::dto::{TopClientBy, TopClientFilters};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_top_clients(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = TopClientFilters {
        by: match first(query, "by").as_deref() {
            Some("errors") => TopClientBy::Errors,
            Some("age") => TopClientBy::Age,
            _ => TopClientBy::Qps,
        },
        n: parse_u64(query, "n", 0),
        pool: first(query, "pool"),
    };
    Response::ok_json(&collect_top_clients(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_clients_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_top_clients(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"qps\""));
        assert!(body.contains("\"n\":20"));
        assert!(body.contains("\"clients\""));
    }

    #[test]
    fn top_clients_by_errors_param() {
        let mut q = BTreeMap::new();
        q.insert("by".into(), vec!["errors".into()]);
        let r = handle_top_clients(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"errors\""));
    }
}
