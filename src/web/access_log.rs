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
    log::log!(
        target: "pg_doorman::web::access",
        level,
        "method={method} path={path} query={query_present} \
         status={status} bytes={bytes} latency_ms={latency_ms} \
         peer={peer} auth_role={auth_role} auth_source={auth_source} \
         auth_user={auth_user}"
    );

    WEB_AUTH_ATTEMPTS
        .with_label_values(&[auth_role, auth_source_for_metric(auth)])
        .inc();
    WEB_REQUESTS_TOTAL
        .with_label_values(&[status_class(status), auth_role])
        .inc();
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
