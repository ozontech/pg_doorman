//! GET /api/events?since=<seq>&max=<N> handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_events;
use crate::web::routes::query::parse_u64;
use crate::web::server::Response;

pub(crate) fn handle_events(query: &BTreeMap<String, Vec<String>>) -> Response {
    let since = parse_u64(query, "since", 0);
    let max = parse_u64(query, "max", 200);
    Response::ok_json(&collect_events(since, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_events(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"next_seq\""));
        assert!(body.contains("\"events\""));
    }
}
