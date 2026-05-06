//! GET /api/databases handler.

use crate::web::routes::collect::collect_databases;
use crate::web::server::Response;

pub(crate) fn handle_databases() -> Response {
    Response::ok_json(&collect_databases())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databases_response_is_200_with_envelope() {
        let r = handle_databases();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"databases\""));
    }
}
