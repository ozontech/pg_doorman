//! `POST /api/admin/{action}` — write surface that mirrors the admin
//! protocol's RELOAD / PAUSE / RESUME / RECONNECT commands. Authorisation
//! is gated by the listener mux (admin basic-auth, see
//! `is_admin_only` in server.rs); this module just dispatches to the
//! async wrappers in `crate::admin::operations` and renders the reply.
//!
//! The optional `?db=<name>` query parameter scopes pause/resume/reconnect
//! to a single database segment of the pool identifier (the second half of
//! `user@db`). RELOAD ignores it.
//!
//! The handler returns the same JSON envelope shape as the read endpoints:
//! `{"ts": ..., "action": "...", "affected_pools": N}`. Each successful
//! action also pushes one or more entries onto the `/api/events` ring so
//! the frontend's chart-annotation overlay paints a vertical line at the
//! moment of the action regardless of which transport triggered it.

use crate::admin::operations::{pause_now, reconnect_now, reload_now, resume_now, AdminEffect};
use crate::web::routes::collect::now_unix_ms;
use crate::web::routes::query::{first, parse_query};
use crate::web::server::Response;

pub(crate) async fn handle_admin_action(raw_path: &str) -> Response {
    let (path, query_str) = match raw_path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (raw_path, ""),
    };
    let query = parse_query(query_str);
    let db = first(&query, "db");

    // The path always carries the `/api/admin/` prefix at this point — the
    // listener already gated on that.
    let action = path.trim_start_matches("/api/admin/");

    match action {
        "reload" => match reload_now().await {
            Ok(()) => json_ok("reload", 1),
            Err(err) => json_err("reload", &err.to_string()),
        },
        "pause" => render_effect("pause", pause_now(db)),
        "resume" => render_effect("resume", resume_now(db)),
        "reconnect" => render_effect("reconnect", reconnect_now(db)),
        _ => Response::json(
            404,
            "Not Found",
            &format!(r#"{{"error":"unknown_action","message":"unknown admin action: {action}"}}"#),
        ),
    }
}

/// Same outcome envelope shape across the two transports: 404 with a
/// `no_matching_db` JSON error when the db filter excluded every pool,
/// 200 with the count of pools touched otherwise.
fn render_effect(action: &str, effect: AdminEffect) -> Response {
    match effect {
        AdminEffect::NoMatchingDb { db } => {
            let escaped = db.replace('"', "\\\"");
            let body = format!(
                r#"{{"ts":{ts},"action":"{action}","error":"no_matching_db","db":"{escaped}"}}"#,
                ts = now_unix_ms()
            );
            Response::json(404, "Not Found", &body)
        }
        AdminEffect::Applied { affected } => json_ok(action, affected as u64),
    }
}

fn json_ok(action: &str, affected: u64) -> Response {
    let body = format!(
        r#"{{"ts":{ts},"action":"{action}","affected_pools":{affected}}}"#,
        ts = now_unix_ms()
    );
    Response::json(200, "OK", &body)
}

fn json_err(action: &str, message: &str) -> Response {
    let escaped = message.replace('"', "\\\"");
    let body = format!(
        r#"{{"ts":{ts},"action":"{action}","error":"{escaped}"}}"#,
        ts = now_unix_ms()
    );
    Response::json(500, "Internal Server Error", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_action_returns_404() {
        let r = handle_admin_action("/api/admin/foo").await;
        assert_eq!(r.status, 404);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("unknown_action"));
    }

    #[tokio::test]
    async fn pause_without_pools_reports_zero_affected() {
        // No pools registered in unit-test global → pause_now returns 0.
        let r = handle_admin_action("/api/admin/pause").await;
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains(r#""action":"pause""#), "{body}");
        assert!(body.contains(r#""affected_pools":0"#), "{body}");
    }

    #[tokio::test]
    async fn pause_with_missing_db_filter_returns_404_with_typed_error() {
        // Mirrors the PG admin protocol: `db` filter that matches no pool
        // is a typo signal, not a silent zero. REST returns 404 + JSON body
        // identifying the unknown db so the SPA can surface it.
        let r = handle_admin_action("/api/admin/pause?db=nonexistent_db").await;
        assert_eq!(r.status, 404);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains(r#""error":"no_matching_db""#), "{body}");
        assert!(body.contains(r#""db":"nonexistent_db""#), "{body}");
    }

    #[tokio::test]
    async fn reconnect_with_missing_db_filter_also_404() {
        let r = handle_admin_action("/api/admin/reconnect?db=ghost").await;
        assert_eq!(r.status, 404);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains(r#""error":"no_matching_db""#), "{body}");
        assert!(body.contains(r#""db":"ghost""#), "{body}");
    }
}
