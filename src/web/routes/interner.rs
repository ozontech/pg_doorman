//! GET /api/interner handler. Public — aggregate without SQL preview.

use crate::web::routes::collect::collect_interner;
use crate::web::server::Response;

pub(crate) fn handle_interner() -> Response {
    Response::ok_json(&collect_interner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interner_response_is_200_with_envelope() {
        let r = handle_interner();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"named\""));
        assert!(body.contains("\"anonymous\""));
    }
}
