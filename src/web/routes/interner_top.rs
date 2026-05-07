//! GET /api/interner/top?n=N handler. Admin-only (mux gates the prefix).

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_interner_top;
use crate::web::routes::query::parse_u64;
use crate::web::server::Response;

pub(crate) fn handle_interner_top(query: &BTreeMap<String, Vec<String>>) -> Response {
    let n = parse_u64(query, "n", 0); // 0 → default in clamp_top_n
    Response::ok_json(&collect_interner_top(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interner_top_response_is_200() {
        let q = BTreeMap::new();
        let r = handle_interner_top(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"entries\""));
        assert!(body.contains("\"n\":20"));
    }

    #[test]
    fn interner_top_honours_n_query_param() {
        let mut q = BTreeMap::new();
        q.insert("n".into(), vec!["50".into()]);
        let r = handle_interner_top(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"n\":50"));
    }
}
