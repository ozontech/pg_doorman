//! GET /api/prepared handler. Public — aggregate without SQL text.

use crate::web::routes::collect::collect_prepared;
use crate::web::server::Response;

pub(crate) fn handle_prepared() -> Response {
    Response::ok_json(&collect_prepared())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_response_is_200_with_envelope() {
        let r = handle_prepared();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"prepared\""));
    }
}
