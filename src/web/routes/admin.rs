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

use crate::admin::operations::{pause_now, reconnect_now, reload_now, resume_now};
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
        "pause" => json_ok("pause", pause_now(db) as u64),
        "resume" => json_ok("resume", resume_now(db) as u64),
        "reconnect" => json_ok("reconnect", reconnect_now(db) as u64),
        _ => Response::json(
            404,
            "Not Found",
            &format!(
                r#"{{"error":"unknown_action","message":"unknown admin action: {action}"}}"#
            ),
        ),
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
}
