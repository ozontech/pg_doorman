//! GET /api/config handler.

use crate::web::routes::collect::collect_config;
use crate::web::server::Response;

pub(crate) fn handle_config() -> Response {
    Response::ok_json(&collect_config())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_response_is_200_with_envelope() {
        let r = handle_config();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"config\""));
    }
}
