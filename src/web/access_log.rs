//! Per-request access log emitted at info level for the web UI. One line
//! per HTTP response, including 401 / 403 / 404 — everything that hits
//! the listener. Format is logfmt (key=value) so awk / Promtail parsers
//! pick it up without extra schema work.
//!
//! The log target `pg_doorman::web::access` is dedicated so operators
//! can tune verbosity separately and so LogTap consumers can filter the
//! `/api/logs` feed by target if the access stream gets noisy.

use std::net::SocketAddr;

use crate::web::auth::{AuthOutcome, AuthSource};

#[allow(clippy::too_many_arguments)]
pub fn write(
    method: &str,
    path: &str,
    query_present: bool,
    status: u16,
    bytes: usize,
    latency_ms: u64,
    peer_addr: Option<SocketAddr>,
    auth: &AuthOutcome,
) {
    let (auth_role, auth_source, auth_user) = match auth {
        AuthOutcome::Admin(id) => ("admin", source_str(id.source), id.username.as_str()),
        AuthOutcome::Sso(id) => ("sso", source_str(id.source), id.username.as_str()),
        AuthOutcome::Anonymous => ("anonymous", "-", "-"),
        AuthOutcome::Rejected => ("rejected", "-", "-"),
    };
    let peer = peer_addr
        .map(|a| a.to_string())
        .unwrap_or_else(|| "-".to_string());
    log::info!(
        target: "pg_doorman::web::access",
        "method={method} path={path} query={query_present} \
         status={status} bytes={bytes} latency_ms={latency_ms} \
         peer={peer} auth_role={auth_role} auth_source={auth_source} \
         auth_user={auth_user}"
    );
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

    /// `log::info!` writes to the global logger and stable Rust offers no
    /// in-process capture without pulling in a test-logger crate. These
    /// tests therefore exercise the helper to confirm it formats every
    /// `AuthOutcome` variant without panicking.
    #[test]
    fn does_not_panic_on_any_outcome() {
        let admin = AuthOutcome::Admin(AuthIdentity {
            username: "admin".into(),
            source: AuthSource::Basic,
        });
        let sso = AuthOutcome::Sso(AuthIdentity {
            username: "alice".into(),
            source: AuthSource::Sso,
        });
        for o in [admin, sso, AuthOutcome::Anonymous, AuthOutcome::Rejected] {
            write("GET", "/api/version", false, 200, 42, 1, None, &o);
        }
    }

    #[test]
    fn source_str_maps_correctly() {
        assert_eq!(source_str(AuthSource::Basic), "basic");
        assert_eq!(source_str(AuthSource::Sso), "sso");
    }
}
