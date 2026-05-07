//! GET /api/users handler.

use crate::web::routes::collect::collect_users;
use crate::web::server::Response;

pub(crate) fn handle_users() -> Response {
    Response::ok_json(&collect_users())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_response_is_200_with_envelope() {
        let r = handle_users();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"users\""));
    }
}
