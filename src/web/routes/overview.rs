//! GET /api/overview handler.

use crate::web::routes::collect::collect_overview;
use crate::web::server::Response;

pub(crate) fn handle_overview() -> Response {
    Response::ok_json(&collect_overview()).with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_response_is_200_json() {
        let r = handle_overview();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"active_clients\"",
            "\"idle_clients\"",
            "\"waiting_clients\"",
            "\"active_servers\"",
            "\"idle_servers\"",
            "\"connections_total\"",
            "\"connections_tls_total\"",
            "\"connections_plain_total\"",
            "\"connections_cancel_total\"",
            "\"query_count_total\"",
            "\"transaction_count_total\"",
            "\"errors_count_total\"",
            "\"prepared_hits_total\"",
            "\"prepared_misses_total\"",
            "\"pools_total\"",
            "\"pools_paused\"",
        ] {
            assert!(body.contains(field), "missing {field} in body={body}");
        }
    }
}
