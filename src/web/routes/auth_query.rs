//! GET /api/auth_query handler.

use crate::web::routes::collect::collect_auth_query;
use crate::web::server::Response;

pub(crate) fn handle_auth_query() -> Response {
    Response::ok_json(&collect_auth_query())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_query_response_is_200_with_envelope() {
        let r = handle_auth_query();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"pools\""));
    }
}
