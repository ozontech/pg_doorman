//! GET /api/pools handler.

use crate::web::routes::collect::collect_pools;
use crate::web::server::Response;

pub(crate) fn handle_pools() -> Response {
    Response::ok_json(&collect_pools())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_response_is_200_json_with_array() {
        let r = handle_pools();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""), "body={body}");
        assert!(body.contains("\"pools\""), "body={body}");
    }
}
