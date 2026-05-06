//! GET /api/log_level handler.

use crate::web::routes::collect::collect_log_level;
use crate::web::server::Response;

pub(crate) fn handle_log_level() -> Response {
    Response::ok_json(&collect_log_level())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_response_is_200_with_envelope() {
        let r = handle_log_level();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"log_level\""));
    }
}
