//! GET /api/version handler.

use crate::web::routes::collect::collect_version;
use crate::web::server::Response;

pub(crate) fn handle_version() -> Response {
    Response::ok_json(&collect_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_response_is_200_json_with_required_fields() {
        let r = handle_version();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"version\""), "body={body}");
        assert!(body.contains("\"git_commit\""), "body={body}");
        assert!(body.contains("\"build_date\""), "body={body}");
        assert!(body.contains("\"ts\""), "body={body}");
    }
}
