//! GET /api/top/prepared?by=hits|misses&n=20 handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_top_prepared;
use crate::web::routes::dto::{TopPreparedBy, TopPreparedFilters};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_top_prepared(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = TopPreparedFilters {
        by: match first(query, "by").as_deref() {
            Some("misses") => TopPreparedBy::Misses,
            _ => TopPreparedBy::Hits,
        },
        n: parse_u64(query, "n", 0),
    };
    Response::ok_json(&collect_top_prepared(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_prepared_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_top_prepared(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"hits\""));
        assert!(body.contains("\"n\":20"));
        assert!(body.contains("\"prepared\""));
    }

    #[test]
    fn top_prepared_by_misses_param() {
        let mut q = BTreeMap::new();
        q.insert("by".into(), vec!["misses".into()]);
        let r = handle_top_prepared(&q);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"misses\""));
    }
}
