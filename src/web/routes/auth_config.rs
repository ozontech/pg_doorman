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
        current_user,
    })
}

fn source_str(source: AuthSource) -> &'static str {
    match source {
        AuthSource::Basic => "basic",
        AuthSource::Sso => "sso",
    }
}
