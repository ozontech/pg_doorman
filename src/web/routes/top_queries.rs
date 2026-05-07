//! GET /api/top/queries?by=count|duration&n=20 handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_top_queries;
use crate::web::routes::dto::{TopQueryBy, TopQueryFilters};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_top_queries(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = TopQueryFilters {
        by: match first(query, "by").as_deref() {
            Some("duration") => TopQueryBy::Duration,
            _ => TopQueryBy::Count,
        },
        n: parse_u64(query, "n", 0),
    };
    Response::ok_json(&collect_top_queries(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_queries_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_top_queries(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"count\""));
        assert!(body.contains("\"n\":20"));
        assert!(body.contains("\"queries\""));
    }

    #[test]
    fn top_queries_by_duration_param() {
        let mut q = BTreeMap::new();
        q.insert("by".into(), vec!["duration".into()]);
        let r = handle_top_queries(&q);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"duration\""));
    }
}
