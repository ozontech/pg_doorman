//! GET /api/connections handler.

use crate::web::routes::collect::collect_connections;
use crate::web::server::Response;

pub(crate) fn handle_connections() -> Response {
    Response::ok_json(&collect_connections())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_response_is_200_with_envelope() {
        let r = handle_connections();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"total\"",
            "\"tls\"",
            "\"plain\"",
            "\"cancel\"",
            "\"errors\"",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }
}
