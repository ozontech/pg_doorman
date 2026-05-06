//! GET /api/pool_coordinator handler.

use crate::web::routes::collect::collect_pool_coordinator;
use crate::web::server::Response;

pub(crate) fn handle_pool_coordinator() -> Response {
    Response::ok_json(&collect_pool_coordinator())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_coordinator_response_is_200_with_envelope() {
        let r = handle_pool_coordinator();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"databases\""));
    }
}
