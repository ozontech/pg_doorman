//! Public endpoint that tells the SPA whether SSO is wired and, if the
//! current request was authenticated, what role its identity holds.
//!
//! Always served anonymously: the SPA needs to learn `sso_proxy_url`
//! before the operator has any credential to send. When the request
//! does carry valid credentials, `current_user` is populated so the SPA
//! can render the role-aware UI without a second probe.

use serde::Serialize;

use crate::web::auth::{AuthOutcome, AuthSource};
use crate::web::server::state::WebServerOptions;
use crate::web::server::wire::Response;

#[derive(Serialize)]
struct AuthConfigResponse<'a> {
    sso_enabled: bool,
    sso_proxy_url: Option<&'a str>,
    /// Returned when `sso_enabled = true` but the runtime did not load
    /// (missing key file, empty audience, unparsable PEM, etc.). The
    /// SPA renders a "SSO is configured but not loaded: <reason>"
    /// banner so the operator sees the broken SSO setup instead of
    /// silently falling back to Basic-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    sso_config_error: Option<&'a str>,
    /// `true` when `[web].sso_admin_groups` is non-empty. The SPA uses
    /// this to drop the "SSO grants read-only access" copy from the
    /// sign-in modal — the real role still resolves on the backend
    /// when the JWT lands.
    sso_admin_groups_configured: bool,
    current_user: Option<CurrentUser<'a>>,
}

#[derive(Serialize)]
struct CurrentUser<'a> {
    username: &'a str,
    source: &'static str,
    role: &'static str,
}

pub(crate) fn handle_auth_config(opts: &WebServerOptions, auth: &AuthOutcome) -> Response {
    let sso = opts.sso.as_ref();
    let proxy_url = sso.and_then(|s| s.proxy_url());
    let current_user = match auth {
        AuthOutcome::Admin(id) => Some(CurrentUser {
            username: id.username.as_str(),
            source: source_str(id.source),
            role: "admin",
        }),
        AuthOutcome::Sso(id) => Some(CurrentUser {
            username: id.username.as_str(),
            source: source_str(id.source),
            role: "sso",
        }),
        _ => None,
    };
    Response::ok_json(&AuthConfigResponse {
        sso_enabled: sso.is_some(),
        sso_proxy_url: proxy_url,
        sso_config_error: opts.sso_config_error.as_deref(),
        sso_admin_groups_configured: opts.sso_admin_groups_configured,
        current_user,
    })
}

fn source_str(source: AuthSource) -> &'static str {
    match source {
        AuthSource::Basic => "basic",
        AuthSource::Sso => "sso",
    }
}
