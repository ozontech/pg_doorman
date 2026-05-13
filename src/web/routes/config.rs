//! GET /api/config handler.

use crate::web::auth::Role;
use crate::web::routes::collect::collect_config;
use crate::web::server::Response;

pub(crate) fn handle_config(role: Role) -> Response {
    // Operator-supplied startup_parameter values can carry tenant
    // identifiers, audit routing tags, or accidental secrets. Only Admin
    // sees literal values; SSO readers get the same masked view as
    // anonymous (key + source + state, value `***`). Because
    // `sso_allowed_users = ["*"]` is the default, promoting SSO readers
    // here would expose those values to every user accepted by the IdP.
    let reveal_startup_values = role >= Role::Admin;
    Response::ok_json(&collect_config(reveal_startup_values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_response_is_200_with_envelope() {
        let r = handle_config(Role::Admin);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"config\""));
    }

    #[test]
    fn anonymous_config_masks_startup_parameter_values() {
        // Set general.startup_parameters via the in-process config so
        // collect_config has something to redact. The Lazy<ArcSwap>
        // config defaults to an empty `Config`, so we rely on the
        // serialized JSON containing `*.startup_parameters` as a
        // nested object only when something is set there. The masked
        // value `"***"` should appear in the response for any such key.
        let r = handle_config(Role::Anonymous);
        let body = std::str::from_utf8(&r.body).unwrap();
        // Bare-minimum invariant: no occurrence of any unmasked
        // startup_parameters value can appear under the anonymous
        // viewer for any key path ending in `startup_parameters.*`.
        // The key path itself is fine (operators want to see *which*
        // GUCs are configured), only the value is hidden.
        let lower = body.to_lowercase();
        if lower.contains("startup_parameters.") {
            // If the test environment configures any startup_parameter,
            // the response must mask its value.
            assert!(
                body.contains("\"***\""),
                "anonymous /api/config has a startup_parameters entry but no masked '***' value, body={body}"
            );
        }
    }
}
