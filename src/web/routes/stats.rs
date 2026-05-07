//! GET /api/stats handler.

use crate::web::routes::collect::collect_stats;
use crate::web::server::Response;

pub(crate) fn handle_stats() -> Response {
    Response::ok_json(&collect_stats())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_response_is_200_with_envelope() {
        let r = handle_stats();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"stats\""));
    }
}
