//! Per-request access log emitted to the standard logger. One line per
//! HTTP response, formatted as logfmt (key=value) so awk / Promtail
//! parsers pick it up without extra schema work.
//!
//! Log level is chosen by the request kind:
//!
//!   * `info`  — admin actions (`POST /api/admin/*`), personal-data
//!     reads (`/api/logs`, `/api/prepared/text/*`, `/api/interner/top`,
//!     `/api/top/queries`), every non-success response (400/401/403/
//!     404/5xx), and any request that produced an authenticated
//!     identity (Sso or Admin role).
//!   * `debug` — anonymous successful reads of public APIs and
//!     `/metrics` scrapes. These dominate the request rate (Prometheus
//!     scrapes every few seconds, the SPA polls overview/pools), so
//!     keeping them off `info` lets `RUST_LOG=info` stay readable.
//!
//! The `pg_doorman::web::access` target is dedicated so operators can
//! filter the access feed independently of the rest of the logger.

use crate::web::auth::{AuthOutcome, AuthSource};
use crate::web::metrics::{WEB_AUTH_ATTEMPTS, WEB_REQUESTS_TOTAL};

#[allow(clippy::too_many_arguments)]
pub fn write(
    method: &str,
    path: &str,
    query_present: bool,
    status: u16,
    bytes: usize,
    latency_ms: u64,
    peer: &str,
    auth: &AuthOutcome,
) {
    let (auth_role, auth_source, auth_user) = match auth {
        AuthOutcome::Admin(id) => ("admin", source_str(id.source), id.username.as_str()),
        AuthOutcome::Sso(id) => ("sso", source_str(id.source), id.username.as_str()),
        AuthOutcome::Anonymous => ("anonymous", "-", "-"),
        AuthOutcome::Rejected => ("rejected", "-", "-"),
    };
    let level = pick_level(path, status, auth);
    // Quote path / peer / auth_user when they contain whitespace,
    // double-quote, or backslash. JWT-issued usernames can carry any
    // unicode, and a malformed peer (TCP timeout edge cases) can
    // contain unexpected characters — without escaping a parser
    // would split fields incorrectly.
    let path_v = logfmt_value(path);
    let peer_v = logfmt_value(peer);
    let user_v = logfmt_value(auth_user);
    log::log!(
        target: "pg_doorman::web::access",
        level,
        "method={method} path={path_v} query={query_present} \
         status={status} bytes={bytes} latency_ms={latency_ms} \
         peer={peer_v} auth_role={auth_role} auth_source={auth_source} \
         auth_user={user_v}"
    );

    WEB_AUTH_ATTEMPTS
        .with_label_values(&[auth_role, auth_source_for_metric(auth)])
        .inc();
    WEB_REQUESTS_TOTAL
        .with_label_values(&[status_class(status), auth_role])
        .inc();
}

/// Format a value safe for logfmt: bare token when it has none of the
/// reserved characters, otherwise a double-quoted string with control
/// characters and `"` / `\` escaped. Control characters (newline,
/// CR, tab, null, etc.) are written as backslash sequences inside the
/// quoted form so a JWT-issued username carrying `\n` cannot break the
/// "one line per response" promise of the access log.
fn logfmt_value(s: &str) -> String {
    let needs_quote = s
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\' || c == '=');
    if !needs_quote {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                // Other control codepoints (DEL, separators, etc.).
                // Escape as \uXXXX so the logfmt line stays printable.
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn auth_source_for_metric(auth: &AuthOutcome) -> &'static str {
    match auth {
        AuthOutcome::Admin(id) => source_str(id.source),
        AuthOutcome::Sso(id) => source_str(id.source),
        AuthOutcome::Anonymous => "none",
        AuthOutcome::Rejected => "none",
    }
}

fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

fn pick_level(path: &str, status: u16, auth: &AuthOutcome) -> log::Level {
    if !(200..300).contains(&status) {
        return log::Level::Info;
    }
    if matches!(auth, AuthOutcome::Admin(_) | AuthOutcome::Sso(_)) {
        return log::Level::Info;
    }
    if path == "/metrics" {
        return log::Level::Debug;
    }
    if is_personal_data_path(path) {
        return log::Level::Info;
    }
    if path.starts_with("/api/admin/") {
        return log::Level::Info;
    }
    log::Level::Debug
}

fn is_personal_data_path(path: &str) -> bool {
    path == "/api/logs"
        || path.starts_with("/api/prepared/text/")
        || path == "/api/interner/top"
        || path == "/api/top/queries"
}

fn source_str(source: AuthSource) -> &'static str {
    match source {
        AuthSource::Basic => "basic",
        AuthSource::Sso => "sso",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::auth::{AuthIdentity, AuthOutcome, AuthSource};

    fn admin() -> AuthOutcome {
        AuthOutcome::Admin(AuthIdentity {
            username: "admin".into(),
            source: AuthSource::Basic,
        })
    }

    fn sso() -> AuthOutcome {
        AuthOutcome::Sso(AuthIdentity {
            username: "alice".into(),
            source: AuthSource::Sso,
        })
    }

    /// `log::info!` writes to the global logger and stable Rust offers
    /// no in-process capture without a test-logger crate. These tests
    /// exercise the helper to confirm it formats every `AuthOutcome`
    /// variant without panicking, and assert the log-level chooser
    /// directly.
    #[test]
    fn does_not_panic_on_any_outcome() {
        for o in [
            admin(),
            sso(),
            AuthOutcome::Anonymous,
            AuthOutcome::Rejected,
        ] {
            write("GET", "/api/version", false, 200, 42, 1, "-", &o);
        }
    }

    #[test]
    fn source_str_maps_correctly() {
        assert_eq!(source_str(AuthSource::Basic), "basic");
        assert_eq!(source_str(AuthSource::Sso), "sso");
    }

    #[test]
    fn metrics_scrape_is_debug() {
        assert_eq!(
            pick_level("/metrics", 200, &AuthOutcome::Anonymous),
            log::Level::Debug
        );
    }

    #[test]
    fn anonymous_public_read_is_debug() {
        assert_eq!(
            pick_level("/api/version", 200, &AuthOutcome::Anonymous),
            log::Level::Debug
        );
        assert_eq!(
            pick_level("/api/overview", 200, &AuthOutcome::Anonymous),
            log::Level::Debug
        );
    }

    #[test]
    fn personal_data_read_is_info_even_anonymous() {
        // Anonymous /api/logs would 401, but if it ever reaches this
        // helper (e.g. backend evolution), the path itself is
        // sensitive enough to warrant info.
        assert_eq!(
            pick_level("/api/logs", 200, &AuthOutcome::Anonymous),
            log::Level::Info
        );
    }

    #[test]
    fn admin_action_is_info() {
        assert_eq!(
            pick_level("/api/admin/reload", 200, &admin()),
            log::Level::Info
        );
    }

    #[test]
    fn authenticated_read_is_info() {
        assert_eq!(pick_level("/api/version", 200, &sso()), log::Level::Info);
        assert_eq!(pick_level("/api/overview", 200, &admin()), log::Level::Info);
    }

    #[test]
    fn logfmt_value_passes_simple_token() {
        assert_eq!(logfmt_value("alice"), "alice");
        assert_eq!(logfmt_value("/api/version"), "/api/version");
        assert_eq!(logfmt_value("10.0.1.5:42312"), "10.0.1.5:42312");
        assert_eq!(logfmt_value("-"), "-");
    }

    #[test]
    fn logfmt_value_quotes_whitespace() {
        assert_eq!(logfmt_value("alice smith"), "\"alice smith\"");
    }

    #[test]
    fn logfmt_value_escapes_quotes_and_backslash() {
        assert_eq!(logfmt_value("a\"b"), "\"a\\\"b\"");
        assert_eq!(logfmt_value("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn logfmt_value_quotes_equals_sign() {
        // `key=value` with `=` in the value would split parsers.
        assert_eq!(logfmt_value("foo=bar"), "\"foo=bar\"");
    }

    #[test]
    fn logfmt_value_escapes_newline_and_cr() {
        // A JWT username carrying `\n` would otherwise split the
        // single access-log line into two — log forging.
        assert_eq!(logfmt_value("alice\nbob"), "\"alice\\nbob\"");
        assert_eq!(logfmt_value("alice\r\nbob"), "\"alice\\r\\nbob\"");
        assert_eq!(logfmt_value("col\ta"), "\"col\\ta\"");
    }

    #[test]
    fn logfmt_value_escapes_other_control_chars() {
        assert_eq!(logfmt_value("a\x07b"), "\"a\\u0007b\"");
        assert_eq!(logfmt_value("\x00"), "\"\\u0000\"");
    }

    #[test]
    fn non_success_is_info() {
        assert_eq!(
            pick_level("/api/version", 401, &AuthOutcome::Anonymous),
            log::Level::Info
        );
        assert_eq!(
            pick_level("/metrics", 500, &AuthOutcome::Anonymous),
            log::Level::Info
        );
    }
}
