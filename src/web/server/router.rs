//! Path dispatch and admin-only gating. The router has no knowledge of
//! the wire format beyond what [`ParsedRequest`] exposes — it picks a
//! handler and returns a [`Response`].

use crate::web::auth::AuthOutcome;
use crate::web::routes;
use crate::web::routes::query::parse_query;

use super::state::WebServerOptions;
use super::wire::{ParsedRequest, Response};

/// Admin-only path prefixes (require `Admin` auth regardless of `ui_anonymous`).
/// Spec section 6.1.
const ADMIN_ONLY_PREFIXES: &[&str] = &[
    "/api/logs",
    "/api/prepared/text/",
    "/api/interner/top",
    // /api/top/queries returns SQL previews — first 120 chars of cached
    // statements. Tenant ids, literal values, schema names, and the
    // occasional accidental secret embedded in SQL all leak through;
    // keep it admin-only regardless of `ui_anonymous`.
    "/api/top/queries",
    "/api/admin/",
];

pub(super) fn is_admin_only(path: &str) -> bool {
    ADMIN_ONLY_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
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
    let (path, query_str) = match req.path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (req.path, ""),
    };
    let query = parse_query(query_str);

    // Prefix-routed paths first (admin-only; mux already gated auth).
    if let Some(hash) = path.strip_prefix("/api/prepared/text/") {
        return routes::prepared_text::handle_prepared_text(hash);
    }

    match path {
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
    auth: AuthOutcome,
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
    let admin_only = is_api && is_admin_only(req.path);

    // The SPA shell (HTML, CSS, JS, fonts, favicon) carries no operator data
    // — the basic-auth challenge is reserved for `/api/*`. Letting the shell
    // load anonymously avoids the double-prompt operators saw on a deep link:
    // browser-native basic auth on the HTML, then the React `AuthGate` modal
    // on the first JSON fetch. Now the React modal is the single password
    // prompt the operator ever sees.
    let needs_admin = admin_only || (is_api && !opts.ui_anonymous);
    if needs_admin && auth != AuthOutcome::Admin {
        return unauthorized_for(req);
    }

    if is_api {
        return route_api(req);
    }

    // SPA: serve the embedded bundle. Anything that is not /api or /metrics
    // resolves to a static asset or falls back to the SPA shell so client-side
    // routes (`/pools`, `/clients/...`) work on a hard refresh.
    let bundle_path = req.path.split_once('?').map(|(p, _)| p).unwrap_or(req.path);
    if let Some(asset) = crate::web::static_assets::lookup(bundle_path) {
        return Response::static_asset(&asset, req.accepts_gzip);
    }

    Response::status(404, "Not Found")
}
