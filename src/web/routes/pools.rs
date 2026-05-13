//! GET /api/pools handler.

use crate::web::auth::Role;
use crate::web::routes::collect::collect_pools;
use crate::web::server::Response;

pub(crate) fn handle_pools(role: Role) -> Response {
    // Only Admin reads literal startup_parameter values. SSO is treated
    // like anonymous here so a broad `sso_allowed_users` list does not
    // expose tenant routing tags, audit identifiers, or accidental
    // secrets stored in startup_parameters.
    let reveal_startup_values = role >= Role::Admin;
    Response::ok_json(&collect_pools(reveal_startup_values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_response_is_200_json_with_array() {
        let r = handle_pools(Role::Admin);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""), "body={body}");
        assert!(body.contains("\"pools\""), "body={body}");
    }

    #[test]
    fn pools_response_hides_startup_value_for_anonymous() {
        // Smoke check on the wired path: anonymous /api/pools must not
        // surface a "value" field anywhere in the response body. The
        // actual redaction logic lives in `StartupParameterDto::from_resolved`
        // and is covered by the dedicated unit test in `dto.rs`.
        let r = handle_pools(Role::Anonymous);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(
            !body.contains("\"value\""),
            "anonymous /api/pools must not include any startup_parameter value, body={body}"
        );
    }
}
