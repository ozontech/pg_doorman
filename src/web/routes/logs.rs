//! GET /api/logs?since=&max=&level=&target= — admin-only live tail
//! over the in-memory LogTap ring.
//!
//! Bypass-routed in `web::server::handle_connection` because the handler
//! must `await` the consumer task via `tokio::sync::mpsc` and `oneshot`;
//! the rest of the API stays sync. See server.rs for the bypass site.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use log::Level;

use crate::config::get_config;
use crate::web::log_tap::{enable_log_tap, log_tap, now_monotonic_ms};
use crate::web::routes::collect::now_unix_ms;
use crate::web::routes::dto::{LogEntryDto, LogsDto};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) async fn handle_logs(query: &BTreeMap<String, Vec<String>>) -> Response {
    let cap = get_config().web.log_tap_max_entries;
    if cap == 0 {
        return Response::json(
            503,
            "Service Unavailable",
            r#"{"error":"log_tap_disabled","message":"log_tap_max_entries is 0 in config"}"#,
        );
    }

    let since = parse_u64(query, "since", 0);
    // 1..=1000 keeps a single drain bounded; 200 matches /api/events default.
    let max_n = parse_u64(query, "max", 200).clamp(1, 1000) as usize;

    let level =
        first(query, "level")
            .as_deref()
            .and_then(|s| match s.to_ascii_uppercase().as_str() {
                "ERROR" => Some(Level::Error),
                "WARN" | "WARNING" => Some(Level::Warn),
                "INFO" => Some(Level::Info),
                "DEBUG" => Some(Level::Debug),
                "TRACE" => Some(Level::Trace),
                _ => None,
            });
    let target = first(query, "target");

    // Activate on first call; subsequent calls reuse the existing Arc.
    let tap = match log_tap() {
        Some(t) => t,
        None => enable_log_tap(cap as usize),
    };
    // Bumps the reaper deadline so the tap stays alive while operators poll.
    tap.last_request_at
        .store(now_monotonic_ms(), Ordering::Relaxed);

    let drain = match tap.drain(since, max_n, level, target).await {
        Ok(d) => d,
        Err(_) => {
            log::warn!("LogTap drain failed: consumer task gone");
            return empty_response(cap);
        }
    };

    let entries: Vec<LogEntryDto> = drain
        .entries
        .into_iter()
        .map(|e| LogEntryDto {
            seq: e.seq,
            ts_ms: e.ts_ms,
            level: e.level.as_str().to_string(),
            target: e.target,
            message: e.message,
        })
        .collect();

    Response::ok_json(&LogsDto {
        ts: now_unix_ms(),
        tap_active: true,
        tap_capacity_entries: cap as u64,
        tap_used_entries: drain.used_entries as u64,
        next_seq: drain.next_seq,
        dropped_before: drain.dropped_before,
        dropped_total: tap.dropped_total.load(Ordering::Relaxed),
        entries,
    })
}

/// Fallback when the consumer task is gone (e.g. shutdown raced an in-flight
/// request). Returns the same envelope shape with an empty entry list and
/// `tap_active = false` so the frontend can recover without surfacing 5xx.
fn empty_response(cap: u32) -> Response {
    Response::ok_json(&LogsDto {
        ts: now_unix_ms(),
        tap_active: false,
        tap_capacity_entries: cap as u64,
        tap_used_entries: 0,
        next_seq: 0,
        dropped_before: 0,
        dropped_total: 0,
        entries: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_zero_503_response_shape() {
        // Shape-parity guard for the `cap == 0` branch in handle_logs:
        // we can't override config in a unit test, so this constructs the
        // equivalent Response and pins the status + body keyword. Behavior
        // of the branch itself is exercised end-to-end by the integration
        // tests in src/web/tests.rs.
        let r = Response::json(
            503,
            "Service Unavailable",
            r#"{"error":"log_tap_disabled","message":"log_tap_max_entries is 0 in config"}"#,
        );
        assert_eq!(r.status, 503);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("log_tap_disabled"));
    }

    #[test]
    fn empty_response_shape_matches_logs_dto() {
        let r = empty_response(8192);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"tap_active\":false",
            "\"tap_capacity_entries\":8192",
            "\"next_seq\":0",
            "\"entries\":[]",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }
}
