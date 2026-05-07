use crate::config::Config;
use crate::web::auth::{AuthIdentity, AuthOutcome, AuthSource, Role};

use super::router::{dispatch, required_role};
use super::state::WebServerOptions;
use super::wire::ParsedRequest;

fn opts(ui_active: bool, ui_anonymous: bool) -> WebServerOptions {
    WebServerOptions {
        ui_active,
        ui_anonymous,
        admin_username: "admin".into(),
        admin_password: "secret".into(),
        sso: None,
        sso_config_error: None,
        trusted_proxies: Vec::new(),
        sso_admin_groups_configured: false,
    }
}

fn admin_outcome() -> AuthOutcome {
    AuthOutcome::Admin(AuthIdentity {
        username: "admin".into(),
        source: AuthSource::Basic,
    })
}

fn split_path(raw_path: &str) -> (&str, Option<&str>) {
    match raw_path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (raw_path, None),
    }
}

fn req<'a>(method: &'a str, raw_path: &'a str) -> ParsedRequest<'a> {
    let (path, query) = split_path(raw_path);
    ParsedRequest {
        method,
        path,
        query,
        authorization: None,
        cookie: None,
        x_forwarded_for: None,
        forwarded: None,
        accepts_gzip: false,
        accepts_json: false,
        connection_close: false,
    }
}

fn req_json<'a>(method: &'a str, raw_path: &'a str) -> ParsedRequest<'a> {
    let (path, query) = split_path(raw_path);
    ParsedRequest {
        method,
        path,
        query,
        authorization: None,
        cookie: None,
        x_forwarded_for: None,
        forwarded: None,
        accepts_gzip: false,
        accepts_json: true,
        connection_close: false,
    }
}

fn config_with(ui: bool, ui_anonymous: bool, admin_password: &str) -> Config {
    let mut cfg = Config::default();
    cfg.web.ui = ui;
    cfg.web.ui_anonymous = ui_anonymous;
    cfg.general.admin_username = "admin".into();
    cfg.general.admin_password = admin_password.into();
    cfg
}

#[test]
fn from_config_demotes_ui_when_admin_password_empty() {
    let opts = WebServerOptions::from_config(&config_with(true, false, ""));
    assert!(!opts.ui_active, "empty password must disable UI");
}

#[test]
fn from_config_demotes_ui_when_admin_password_is_default_admin() {
    let opts = WebServerOptions::from_config(&config_with(true, false, "admin"));
    assert!(!opts.ui_active, "literal 'admin' must disable UI");
}

#[test]
fn from_config_keeps_ui_off_when_web_ui_false_even_with_strong_password() {
    let opts = WebServerOptions::from_config(&config_with(false, false, "secret"));
    assert!(!opts.ui_active);
}

#[test]
fn from_config_enables_ui_when_password_strong_and_web_ui_true() {
    let opts = WebServerOptions::from_config(&config_with(true, true, "secret"));
    assert!(opts.ui_active);
    assert!(opts.ui_anonymous);
}

#[test]
fn from_config_copies_credentials_through() {
    let mut cfg = config_with(false, false, "p4ssw0rd");
    cfg.general.admin_username = "ops".into();
    let opts = WebServerOptions::from_config(&cfg);
    assert_eq!(opts.admin_username, "ops");
    assert_eq!(opts.admin_password, "p4ssw0rd");
}

#[test]
fn parse_minimal_get() {
    let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert_eq!(p.method, "GET");
    assert_eq!(p.path, "/api/foo");
    assert_eq!(p.authorization, None);
    assert!(!p.accepts_gzip);
}

#[test]
fn parse_with_authorization_header() {
    let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nAuthorization: Basic abc\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert_eq!(p.authorization, Some("Basic abc"));
    assert!(!p.accepts_gzip);
}

#[test]
fn parse_with_lowercase_authorization() {
    let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nauthorization: Basic abc\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert_eq!(p.authorization, Some("Basic abc"));
    assert!(!p.accepts_gzip);
}

#[test]
fn parse_rejects_malformed_request_line() {
    assert!(ParsedRequest::parse("garbage").is_none());
}

#[test]
fn parse_detects_accept_application_json() {
    let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nAccept: application/json\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert!(p.accepts_json);
}

#[test]
fn parse_detects_lowercase_accept() {
    let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\naccept: application/json, */*\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert!(p.accepts_json);
}

#[test]
fn parse_does_not_detect_json_when_accept_is_html() {
    let raw = "GET / HTTP/1.1\r\nHost: x\r\nAccept: text/html\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert!(!p.accepts_json);
}

#[test]
fn parse_detects_gzip_in_accept_encoding() {
    let raw = "GET /metrics HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip, deflate\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert!(p.accepts_gzip);
}

#[test]
fn parse_strips_query_from_path() {
    let raw = "GET /api/logs?since=10&limit=50 HTTP/1.1\r\nHost: x\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert_eq!(p.path, "/api/logs");
    assert_eq!(p.query, Some("since=10&limit=50"));
}

#[test]
fn parse_query_is_none_without_question_mark() {
    let raw = "GET /api/version HTTP/1.1\r\nHost: x\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert_eq!(p.path, "/api/version");
    assert_eq!(p.query, None);
}

#[test]
fn parse_extracts_cookie_header() {
    let raw = "GET / HTTP/1.1\r\nHost: x\r\nCookie: foo=bar; sso_access_token=abc; baz=qux\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert_eq!(p.cookie, Some("foo=bar; sso_access_token=abc; baz=qux"));
}

#[test]
fn parse_extracts_lowercase_cookie() {
    let raw = "GET / HTTP/1.1\r\nHost: x\r\ncookie: a=b\r\n\r\n";
    let p = ParsedRequest::parse(raw).unwrap();
    assert_eq!(p.cookie, Some("a=b"));
}

#[test]
fn dispatch_rejects_post() {
    let r = dispatch(
        &req("POST", "/api/foo"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 405);
}

#[test]
fn dispatch_404_when_ui_inactive() {
    let r = dispatch(
        &req("GET", "/api/foo"),
        &opts(false, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 404);
}

#[test]
fn dispatch_unknown_api_returns_501() {
    let r = dispatch(
        &req("GET", "/api/not-yet-wired"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 501);
}

#[test]
fn dispatch_overview_returns_200() {
    let r = dispatch(
        &req("GET", "/api/overview"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_version_returns_200() {
    let r = dispatch(
        &req("GET", "/api/version"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_pools_returns_200() {
    let r = dispatch(
        &req("GET", "/api/pools"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_admin_anonymous_json_request_returns_401_without_challenge() {
    // SPA / JSON callers must NOT receive WWW-Authenticate, otherwise the
    // browser caches credentials we did not solicit and replays them
    // forever, hiding the React sign-in modal.
    let r = dispatch(
        &req_json("GET", "/api/logs"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
    assert!(
        !r.extra_headers
            .iter()
            .any(|(k, _)| *k == "WWW-Authenticate"),
        "JSON 401 should not advertise Basic auth"
    );
}

#[test]
fn dispatch_401_on_anonymous_admin_path() {
    let r = dispatch(
        &req("GET", "/api/logs"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
    assert!(r
        .extra_headers
        .iter()
        .any(|(k, _)| *k == "WWW-Authenticate"));
}

// Note: the admin-success path for /api/logs is handled by the bypass
// in `handle_connection` (async handler over LogTap mpsc), not by
// `dispatch`. Integration tests in src/web/tests.rs cover that route.

#[test]
fn dispatch_401_on_anonymous_public_when_ui_anonymous_false() {
    let r = dispatch(
        &req("GET", "/api/overview"),
        &opts(true, false),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_serves_spa_shell_at_root() {
    let r = dispatch(&req("GET", "/"), &opts(true, true), &admin_outcome());
    assert_eq!(r.status, 200);
    assert!(
        r.extra_headers
            .iter()
            .any(|(k, v)| *k == "Content-Type" && v.contains("text/html")),
        "root should serve the SPA shell"
    );
}

#[test]
fn dispatch_serves_spa_shell_for_unknown_route() {
    // Client-side router hits this path on a hard refresh of a deep link.
    let r = dispatch(&req("GET", "/pools"), &opts(true, true), &admin_outcome());
    assert_eq!(r.status, 200);
    assert!(
        r.extra_headers
            .iter()
            .any(|(k, v)| *k == "Content-Type" && v.contains("text/html")),
        "deep link should fall back to the SPA shell"
    );
}

#[test]
fn dispatch_serves_spa_shell_anonymously_when_ui_anonymous_false() {
    // Hard-refreshing a deep link must not trigger a browser-native
    // basic-auth prompt on top of the React `AuthGate`. The SPA shell is
    // anonymous regardless of `ui_anonymous`; only `/api/*` is gated.
    for path in ["/", "/overview", "/pools/some-pool"] {
        let r = dispatch(
            &req("GET", path),
            &opts(true, false),
            &AuthOutcome::Anonymous,
        );
        assert_eq!(
            r.status, 200,
            "anonymous SPA shell should serve {path} with ui_anonymous=false"
        );
        assert!(
            r.extra_headers
                .iter()
                .any(|(k, v)| *k == "Content-Type" && v.contains("text/html")),
            "{path} should serve the SPA shell as text/html"
        );
    }
}

#[test]
fn dispatch_still_gates_api_when_ui_anonymous_false_after_spa_relax() {
    // Counterpart to `dispatch_serves_spa_shell_anonymously_when_ui_anonymous_false`:
    // the loosened gate must not leak `/api/*` to anonymous callers.
    let r = dispatch(
        &req("GET", "/api/overview"),
        &opts(true, false),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_returns_404_for_unknown_asset_when_index_missing() {
    // The bundle is committed in this repo so this test only exercises the
    // Cache-Control / mime path, not the missing-bundle branch — keep the
    // assertion shape forward-compatible by allowing 200 (asset hit) or
    // SPA fallback.
    let r = dispatch(
        &req("GET", "/assets/missing.js"),
        &opts(true, true),
        &admin_outcome(),
    );
    // SPA fallback always returns 200 with the index when the bundle is
    // present. If we ever ship without dist, this would be 404.
    assert!(r.status == 200 || r.status == 404, "got {}", r.status);
}

#[test]
fn required_role_admin_for_management() {
    assert_eq!(required_role("/api/admin/reload", true), Role::Admin);
    assert_eq!(required_role("/api/admin/pause", false), Role::Admin);
}

#[test]
fn required_role_sso_for_personal_data() {
    assert_eq!(required_role("/api/logs", true), Role::Sso);
    assert_eq!(required_role("/api/prepared/text/abc", true), Role::Sso);
    assert_eq!(required_role("/api/top/queries", true), Role::Sso);
    assert_eq!(required_role("/api/interner/top", true), Role::Sso);
}

#[test]
fn required_role_public_when_anonymous_allowed() {
    assert_eq!(required_role("/api/version", true), Role::Anonymous);
    assert_eq!(required_role("/api/overview", true), Role::Anonymous);
    assert_eq!(required_role("/api/pools", true), Role::Anonymous);
    assert_eq!(required_role("/api/prepared", true), Role::Anonymous);
}

#[test]
fn required_role_sso_when_anonymous_forbidden() {
    assert_eq!(required_role("/api/version", false), Role::Sso);
    assert_eq!(required_role("/api/overview", false), Role::Sso);
    assert_eq!(required_role("/api/pools", false), Role::Sso);
}

fn sso_outcome() -> AuthOutcome {
    AuthOutcome::Sso(AuthIdentity {
        username: "alice".into(),
        source: AuthSource::Sso,
    })
}

#[test]
fn dispatch_sso_can_get_logs_via_async_branch() {
    // /api/logs is normally handled by the async branch in handle_connection;
    // when it falls into dispatch(), Sso role passes the role check (200/501
    // — never 401/403).
    let r = dispatch(&req("GET", "/api/logs"), &opts(true, false), &sso_outcome());
    assert!(r.status != 401 && r.status != 403, "got {}", r.status);
}

#[test]
fn dispatch_sso_forbidden_on_management_path() {
    // /api/admin/* requires Admin; Sso role gets 403, not 401.
    let r = dispatch(
        &req("GET", "/api/admin/reload"),
        &opts(true, true),
        &sso_outcome(),
    );
    assert_eq!(r.status, 403);
    let body = std::str::from_utf8(&r.body).unwrap();
    assert!(body.contains("admin role required"));
}

#[test]
fn dispatch_anonymous_unauthorized_on_management_path() {
    // /api/admin/* — Anonymous gets 401 (login), not 403.
    let r = dispatch(
        &req("GET", "/api/admin/reload"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_anonymous_cannot_get_logs() {
    let r = dispatch(
        &req("GET", "/api/logs"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_rejected_on_public_anonymous_endpoint_passes() {
    // Rejected has role=Anonymous. On a public endpoint with
    // ui_anonymous=true the required role is Anonymous, so the request
    // proceeds. This guards against the regression where the SPA's
    // poison `Authorization: Basic ` header (sent on every fetch to
    // override browser-cached creds) would otherwise turn every
    // anonymous request into a 401.
    let r = dispatch(
        &req("GET", "/api/version"),
        &opts(true, true),
        &AuthOutcome::Rejected,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_rejected_on_personal_data_returns_401() {
    // Rejected on /api/logs (Sso required) still 401s — the fall-
    // through above only covers paths whose required role is
    // Anonymous.
    let r = dispatch(
        &req("GET", "/api/logs"),
        &opts(true, true),
        &AuthOutcome::Rejected,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_rejected_on_public_when_anonymous_disabled_returns_401() {
    // ui_anonymous=false raises the bar for /api/version to Sso, so
    // Rejected.role()=Anonymous is now insufficient.
    let r = dispatch(
        &req("GET", "/api/version"),
        &opts(true, false),
        &AuthOutcome::Rejected,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn auth_config_anonymous_when_sso_disabled() {
    let r = dispatch(
        &req("GET", "/api/auth/config"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
    let body = std::str::from_utf8(&r.body).unwrap();
    assert!(body.contains(r#""sso_enabled":false"#));
    assert!(body.contains(r#""current_user":null"#));
}

#[test]
fn auth_config_carries_admin_identity() {
    let r = dispatch(
        &req("GET", "/api/auth/config"),
        &opts(true, false),
        &admin_outcome(),
    );
    assert_eq!(r.status, 200);
    let body = std::str::from_utf8(&r.body).unwrap();
    assert!(body.contains(r#""role":"admin""#));
    assert!(body.contains(r#""source":"basic""#));
    assert!(body.contains(r#""username":"admin""#));
}

#[test]
fn dispatch_clients_returns_200() {
    let r = dispatch(
        &req("GET", "/api/clients"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_clients_with_query_params_returns_200() {
    let r = dispatch(
        &req("GET", "/api/clients?limit=10&sort=errors_total"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_servers_returns_200() {
    let r = dispatch(
        &req("GET", "/api/servers"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_connections_returns_200() {
    let r = dispatch(
        &req("GET", "/api/connections"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_stats_returns_200() {
    let r = dispatch(
        &req("GET", "/api/stats"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_databases_returns_200() {
    let r = dispatch(
        &req("GET", "/api/databases"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_users_returns_200() {
    let r = dispatch(
        &req("GET", "/api/users"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_auth_query_returns_200() {
    let r = dispatch(
        &req("GET", "/api/auth_query"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_config_returns_200() {
    let r = dispatch(
        &req("GET", "/api/config"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_log_level_returns_200() {
    let r = dispatch(
        &req("GET", "/api/log_level"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_pool_coordinator_returns_200() {
    let r = dispatch(
        &req("GET", "/api/pool_coordinator"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_pool_scaling_returns_200() {
    let r = dispatch(
        &req("GET", "/api/pool_scaling"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[cfg(target_os = "linux")]
#[test]
fn dispatch_sockets_returns_200_on_linux() {
    let r = dispatch(
        &req("GET", "/api/sockets"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    // 500 acceptable in sandbox; handler did not panic = pass.
    assert!(r.status == 200 || r.status == 500, "got {}", r.status);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn dispatch_sockets_returns_503_on_non_linux() {
    let r = dispatch(
        &req("GET", "/api/sockets"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 503);
}

#[test]
fn dispatch_prepared_returns_200() {
    let r = dispatch(
        &req("GET", "/api/prepared"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_interner_returns_200() {
    let r = dispatch(
        &req("GET", "/api/interner"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_interner_top_anonymous_returns_401() {
    let r = dispatch(
        &req("GET", "/api/interner/top"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_interner_top_admin_returns_200() {
    let r = dispatch(
        &req("GET", "/api/interner/top?n=10"),
        &opts(true, true),
        &admin_outcome(),
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_prepared_text_anonymous_returns_401() {
    let r = dispatch(
        &req("GET", "/api/prepared/text/0x123"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_prepared_text_admin_unknown_hash_returns_404() {
    let r = dispatch(
        &req("GET", "/api/prepared/text/0xdeadbeef"),
        &opts(true, true),
        &admin_outcome(),
    );
    assert_eq!(r.status, 404);
}

#[test]
fn dispatch_top_clients_returns_200() {
    let r = dispatch(
        &req("GET", "/api/top/clients"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_top_queries_anonymous_returns_401() {
    // /api/top/queries returns SQL previews — admin-only regardless of
    // ui_anonymous so tenant identifiers and embedded secrets do not leak.
    let r = dispatch(
        &req("GET", "/api/top/queries"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 401);
}

#[test]
fn dispatch_top_queries_admin_returns_200() {
    let r = dispatch(
        &req("GET", "/api/top/queries"),
        &opts(true, true),
        &admin_outcome(),
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_top_prepared_returns_200() {
    let r = dispatch(
        &req("GET", "/api/top/prepared"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_apps_returns_200() {
    let r = dispatch(
        &req("GET", "/api/apps"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_events_returns_200() {
    let r = dispatch(
        &req("GET", "/api/events"),
        &opts(true, true),
        &AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}
