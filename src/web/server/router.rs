//! Path dispatch and three-role gating. The router has no knowledge of
//! the wire format beyond what [`ParsedRequest`] exposes — it picks a
//! handler and returns a [`Response`].

use crate::web::auth::{AuthOutcome, Role};
use crate::web::routes;
use crate::web::routes::query::parse_query;

use super::state::WebServerOptions;
use super::wire::{ParsedRequest, Response};

/// Mutating endpoints. Only Admin (Basic) may call them.
const MANAGEMENT_PREFIXES: &[&str] = &["/api/admin/"];

/// Read-only endpoints that expose personal data — SQL text, logs, top
/// queries. Sso role and Admin role may call them; Anonymous cannot
/// regardless of `ui_anonymous`.
///
/// `/api/top/queries` returns SQL previews — first 120 chars of cached
/// statements. Tenant ids, literal values, schema names, and the
/// occasional accidental secret embedded in SQL all leak through, so it
/// stays gated alongside `/api/logs` and the prepared-statement views.
const PERSONAL_DATA_PREFIXES: &[&str] = &[
    "/api/logs",
    "/api/prepared/text/",
    "/api/interner/top",
    "/api/top/queries",
];

/// Decide which `Role` an `/api/*` path requires.
///
/// - Management paths (mutating admin operations) → `Admin`.
/// - Personal-data paths (SQL text, logs) → `Sso` (Admin satisfies via
///   ordering).
/// - Other public paths → `Sso` when `ui_anonymous=false`, otherwise
///   `Anonymous`.
pub(super) fn required_role(path: &str, ui_anonymous: bool) -> Role {
    if MANAGEMENT_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return Role::Admin;
    }
    let is_personal = PERSONAL_DATA_PREFIXES.iter().any(|p| path.starts_with(p));
    if is_personal || !ui_anonymous {
        Role::Sso
    } else {
        Role::Anonymous
    }
}

/// Picks the right 401 shape for the caller. Browsers and curl get the
/// `WWW-Authenticate: Basic` challenge so existing tooling keeps working;
/// `Accept: application/json` (the SPA) gets a plain 401 so the React
/// modal can take over without the browser caching credentials.
pub(super) fn unauthorized_for(req: &ParsedRequest<'_>) -> Response {
    if req.accepts_json {
        Response::unauthorized_silent()
    } else {
        Response::unauthorized()
    }
}

fn route_api(req: &ParsedRequest<'_>) -> Response {
    // ParsedRequest already split path on `?` — no further work here.
    let query = parse_query(req.query.unwrap_or(""));

    // Prefix-routed paths first (admin-only; mux already gated auth).
    if let Some(hash) = req.path.strip_prefix("/api/prepared/text/") {
        return routes::prepared_text::handle_prepared_text(hash);
    }

    match req.path {
        "/api/version" => routes::version::handle_version(),
        "/api/overview" => routes::overview::handle_overview(),
        "/api/pools" => routes::pools::handle_pools(),
        "/api/clients" => routes::clients::handle_clients(&query),
        "/api/connections" => routes::connections::handle_connections(),
        "/api/databases" => routes::databases::handle_databases(),
        "/api/servers" => routes::servers::handle_servers(&query),
        "/api/stats" => routes::stats::handle_stats(),
        "/api/users" => routes::users::handle_users(),
        "/api/auth_query" => routes::auth_query::handle_auth_query(),
        "/api/config" => routes::config::handle_config(),
        "/api/log_level" => routes::log_level::handle_log_level(),
        "/api/pool_coordinator" => routes::pool_coordinator::handle_pool_coordinator(),
        "/api/pool_scaling" => routes::pool_scaling::handle_pool_scaling(),
        "/api/process" => routes::process::handle_process(),
        "/api/process/memory" => routes::process::handle_process_memory(),
        "/api/sockets" => routes::sockets::handle_sockets(),
        "/api/prepared" => routes::prepared::handle_prepared(),
        "/api/interner" => routes::interner::handle_interner(),
        "/api/interner/top" => routes::interner_top::handle_interner_top(&query),
        "/api/top/clients" => routes::top_clients::handle_top_clients(&query),
        "/api/top/prepared" => routes::top_prepared::handle_top_prepared(&query),
        "/api/top/queries" => routes::top_queries::handle_top_queries(&query),
        "/api/apps" => routes::apps::handle_apps(&query),
        "/api/events" => routes::events::handle_events(&query),
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
}

pub(super) fn dispatch(
    req: &ParsedRequest<'_>,
    opts: &WebServerOptions,
    auth: &AuthOutcome,
) -> Response {
    let is_admin_post = req.method == "POST" && req.path.starts_with("/api/admin/");
    if req.method != "GET" && req.method != "HEAD" && !is_admin_post {
        return Response::status(405, "Method Not Allowed");
    }

    if !opts.ui_active {
        // /metrics already handled before dispatch().
        return Response::status(404, "Not Found");
    }

    let is_api = req.path.starts_with("/api/");

    // /api/auth/config is anonymous on purpose: the SPA needs to learn
    // whether SSO is configured (and what role the current request holds)
    // before showing a sign-in screen.
    if is_api && req.path == "/api/auth/config" {
        return crate::web::routes::auth_config::handle_auth_config(opts, auth);
    }

    if is_api {
        let needed = required_role(req.path, opts.ui_anonymous);
        let actual = auth.role();
        if matches!(auth, AuthOutcome::Rejected) {
            return unauthorized_for(req);
        }
        if actual < needed {
            return match auth {
                AuthOutcome::Sso(_) => Response::forbidden("admin role required"),
                _ => unauthorized_for(req),
            };
        }
        return route_api(req);
    }

    // SPA shell: serve the embedded bundle. Anything that is not /api or
    // /metrics resolves to a static asset (or 404 — the SPA shell file
    // is registered through the same lookup) so client-side routes work
    // on a hard refresh.
    if let Some(asset) = crate::web::static_assets::lookup(req.path) {
        return Response::static_asset(&asset, req.accepts_gzip);
    }

    Response::status(404, "Not Found")
}
