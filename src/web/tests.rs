//! End-to-end smoke tests for the web listener mux.
//!
//! Each test spawns a real listener on `127.0.0.1:<random port>` (chosen via
//! `portpicker`, already in dev-dependencies), opens a TcpStream and sends a
//! hand-rolled HTTP/1.1 request line + headers. We avoid pulling reqwest into
//! dev-deps for these few cases.
//!
//! Tests run with `#[serial]` because the listener stores its `WebServerOptions`
//! in a process-wide `ArcSwap` (see `start_web_server`) so that admin
//! `RELOAD` can update auth/gating without a restart. Two parallel listeners
//! in the same test binary would clobber each other's slot — a non-issue in
//! production where exactly one listener exists per process.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serial_test::serial;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::web::sso::test_helpers::{mint_jwt as sso_mint_jwt, ClaimsBuilder};
use crate::web::sso::{test_keys, AllowedUsers, SsoRuntime};
use crate::web::{serve_on, WebServerOptions};

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

fn opts_with_sso(ui_anonymous: bool) -> WebServerOptions {
    let rt = SsoRuntime::from_pem_bytes(
        test_keys::PUBLIC_PEM.as_bytes(),
        &["pg_doorman".to_string()],
        AllowedUsers::Any,
        Some("https://sso.example.com/oauth2/start".into()),
        crate::web::sso::AdminBridge::default(),
    )
    .expect("build sso runtime");
    WebServerOptions {
        ui_active: true,
        ui_anonymous,
        admin_username: "admin".into(),
        admin_password: "secret".into(),
        sso: Some(Arc::new(rt)),
        sso_config_error: None,
        trusted_proxies: Vec::new(),
        sso_admin_groups_configured: false,
    }
}

fn mint_jwt(name: &str, exp_offset: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sso_mint_jwt(&ClaimsBuilder {
        preferred_username: Some(name),
        sub: None,
        aud: serde_json::json!("pg_doorman"),
        exp: now + exp_offset,
    })
}

/// Bind on `127.0.0.1:0`, ask the kernel for an actual port, hand the
/// pre-bound listener to the accept loop. Replaces the previous
/// `portpicker::pick_unused_port + sleep(150ms)` pattern that codex
/// flagged: pick_unused_port races between picking and binding (the
/// port can be claimed in that window), and the fixed sleep was a
/// readiness fudge instead of a synchronisation point.
async fn spawn_server(opts: WebServerOptions) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    tokio::spawn(async move {
        serve_on(listener, opts).await;
    });
    // The listener already accepts connections — this short yield gives
    // the spawned task a tick to enter the accept loop. Two orders of
    // magnitude shorter than the old 150 ms sleep.
    tokio::time::sleep(Duration::from_millis(5)).await;
    port
}

async fn send(port: u16, request: &str) -> String {
    // Auto-inject `Connection: close` so the keep-alive accept loop
    // releases the socket as soon as the response is written. Without
    // this every test would block for `KEEPALIVE_IDLE_TIMEOUT` (30 s)
    // before `read_to_end` saw EOF.
    let mut request = request.to_string();
    if !request.to_lowercase().contains("connection:") {
        request = request.replacen("\r\n\r\n", "\r\nConnection: close\r\n\r\n", 1);
    }
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .expect("connect timeout")
    .expect("connect");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await;
    String::from_utf8_lossy(&response).into_owned()
}

#[tokio::test]
#[serial]
async fn metrics_endpoint_serves_prometheus_body_when_ui_inactive() {
    let port = spawn_server(opts(false, true)).await;
    let raw = send(port, "GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.contains("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("pg_doorman_"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_returns_404_when_ui_inactive() {
    let port = spawn_server(opts(false, true)).await;
    let raw = send(
        port,
        "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 404"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_unknown_route_returns_501_when_ui_active_anonymous() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/not-yet-wired HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 501"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_admin_route_returns_401_without_auth() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/logs HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
    assert!(raw.contains("WWW-Authenticate: Basic"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_logs_admin_returns_envelope_or_disabled() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/logs HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    // 200 when log_tap_max_entries > 0 (default 8192); 503 when an operator
    // explicitly disabled the tap. Both are valid contract outcomes.
    assert!(
        raw.starts_with("HTTP/1.1 200 OK") || raw.starts_with("HTTP/1.1 503"),
        "raw={raw}"
    );
    if raw.starts_with("HTTP/1.1 200 OK") {
        for field in [
            "\"ts\"",
            "\"tap_active\"",
            "\"tap_capacity_entries\"",
            "\"next_seq\"",
            "\"entries\"",
        ] {
            assert!(raw.contains(field), "missing {field} in {raw}");
        }
    } else {
        assert!(raw.contains("log_tap_disabled"), "raw={raw}");
    }
}

#[tokio::test]
#[serial]
async fn api_public_route_returns_401_when_ui_anonymous_false() {
    let port = spawn_server(opts(true, false)).await;
    let raw = send(
        port,
        "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_version_returns_json() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/version HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("Content-Type: application/json"), "raw={raw}");
    assert!(raw.contains("\"version\""), "raw={raw}");
    assert!(raw.contains("\"git_commit\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_overview_returns_json_when_ui_active() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"active_clients\""), "raw={raw}");
    assert!(raw.contains("\"pools_total\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_pools_returns_json_when_ui_active() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/pools HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"pools\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_overview_still_404_when_ui_inactive() {
    let port = spawn_server(opts(false, true)).await;
    let raw = send(
        port,
        "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 404"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_clients_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/clients HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"clients\""), "raw={raw}");
    assert!(raw.contains("\"total\""), "raw={raw}");
    assert!(raw.contains("\"limit\":100"), "raw={raw}");
    assert!(raw.contains("\"offset\":0"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_clients_with_query_params() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/clients?limit=50&offset=10&sort=age_seconds&order=asc HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"limit\":50"), "raw={raw}");
    assert!(raw.contains("\"offset\":10"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_servers_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/servers HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"servers\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_connections_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/connections HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    for field in [
        "\"ts\"",
        "\"total\"",
        "\"tls\"",
        "\"plain\"",
        "\"cancel\"",
        "\"errors\"",
    ] {
        assert!(raw.contains(field), "missing {field} in {raw}");
    }
}

#[tokio::test]
#[serial]
async fn api_stats_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/stats HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"stats\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_databases_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/databases HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"databases\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_users_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/users HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"users\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_config_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/config HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"config\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_log_level_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/log_level HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"log_level\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_auth_query_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/auth_query HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"pools\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_pool_scaling_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/pool_scaling HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"pools\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_pool_coordinator_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/pool_coordinator HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"databases\""), "raw={raw}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[serial]
async fn api_sockets_returns_200_or_500_on_linux() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/sockets HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK") || raw.starts_with("HTTP/1.1 500"),
        "raw={raw}"
    );
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
#[serial]
async fn api_sockets_returns_503_on_non_linux() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/sockets HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 503"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_prepared_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/prepared HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"prepared\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_interner_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/interner HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"named\""), "raw={raw}");
    assert!(raw.contains("\"anonymous\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_interner_top_anonymous_returns_401() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/interner/top HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_interner_top_admin_returns_200() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/interner/top?n=5 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"n\":5"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_prepared_text_anonymous_returns_401() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/prepared/text/0x123 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_top_clients_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/top/clients HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"by\":\"qps\""), "raw={raw}");
    assert!(raw.contains("\"n\":20"), "raw={raw}");
    assert!(raw.contains("\"clients\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_apps_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/apps HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"apps\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_top_queries_anonymous_returns_401() {
    // /api/top/queries returns SQL previews — admin-only regardless of
    // ui_anonymous so SQL literals and secrets do not leak.
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/top/queries HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_top_queries_admin_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/top/queries HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"by\":\"count\""), "raw={raw}");
    assert!(raw.contains("\"queries\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_prepared_text_admin_unknown_hash_returns_404() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/prepared/text/0xdeadbeef HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    assert!(raw.starts_with("HTTP/1.1 404"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_top_prepared_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/top/prepared HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"by\":\"hits\""), "raw={raw}");
    assert!(raw.contains("\"prepared\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn api_events_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/events HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"events\""), "raw={raw}");
    assert!(raw.contains("\"next_seq\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn http_keep_alive_serves_two_requests_on_one_connection() {
    // Two sequential GETs without `Connection: close` should both come
    // back over the same TCP connection. The server then closes when
    // we drop our half (no more requests). Until codex perf P1#2 the
    // listener closed after one request, forcing the SPA to reconnect
    // multiple times per poll interval.
    let port = spawn_server(opts(true, true)).await;
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .expect("connect timeout")
    .expect("connect");
    stream
        .write_all(b"GET /api/version HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    // Request #2 piggybacks immediately — keep-alive means the listener
    // is still reading the same socket.
    stream
        .write_all(b"GET /api/overview HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut buf)).await;
    let raw = String::from_utf8_lossy(&buf);
    // Both bodies should be in the buffer; count 200 status lines.
    let oks = raw.matches("HTTP/1.1 200 OK").count();
    assert_eq!(
        oks, 2,
        "expected two 200 OK responses, got {oks} in:\n{raw}"
    );
    assert!(raw.contains("\"version\""), "first response missing: {raw}");
    assert!(
        raw.contains("\"active_clients\""),
        "second response missing: {raw}"
    );
}

#[tokio::test]
#[serial]
async fn sso_bearer_grants_logs_access() {
    let port = spawn_server(opts_with_sso(false)).await;
    let token = mint_jwt("alice", 600);
    let raw = send(
        port,
        &format!(
            "GET /api/logs HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\n\r\n",
            token
        ),
    )
    .await;
    // 200 (envelope) or 503 (log_tap unavailable) — both mean Sso role passed.
    assert!(
        raw.starts_with("HTTP/1.1 200") || raw.starts_with("HTTP/1.1 503"),
        "raw={raw}"
    );
}

#[tokio::test]
#[serial]
async fn sso_post_admin_returns_403() {
    let port = spawn_server(opts_with_sso(false)).await;
    let token = mint_jwt("alice", 600);
    let raw = send(
        port,
        &format!(
            "POST /api/admin/reload HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\n\r\n",
            token
        ),
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 403"), "raw={raw}");
    assert!(raw.contains("admin role required"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn auth_config_reports_sso_enabled() {
    let port = spawn_server(opts_with_sso(false)).await;
    let raw = send(
        port,
        "GET /api/auth/config HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200"), "raw={raw}");
    assert!(raw.contains(r#""sso_enabled":true"#), "raw={raw}");
    assert!(
        raw.contains(r#""sso_proxy_url":"https://sso.example.com/oauth2/start""#),
        "raw={raw}"
    );
}

#[tokio::test]
#[serial]
async fn cookie_fed_jwt_grants_logs_access() {
    let port = spawn_server(opts_with_sso(false)).await;
    let token = mint_jwt("alice", 600);
    let raw = send(
        port,
        &format!(
            "GET /api/logs HTTP/1.1\r\nHost: localhost\r\nCookie: sso_access_token={}\r\n\r\n",
            token
        ),
    )
    .await;
    assert!(
        raw.starts_with("HTTP/1.1 200") || raw.starts_with("HTTP/1.1 503"),
        "raw={raw}"
    );
}

#[tokio::test]
#[serial]
async fn anonymous_personal_data_path_returns_401() {
    // ui_anonymous=true should still gate /api/logs (it's personal data).
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/logs HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn anonymous_public_passes_with_poison_basic_header() {
    // Regression: the SPA's `api.ts` sends `Authorization: Basic ` on
    // every fetch when there are no credentials, to override the
    // browser's basic-auth cache. The backend used to demote that to
    // Anonymous; the three-role refactor briefly let it fall through
    // as Rejected and 401'd every public endpoint. This test pins the
    // fix.
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/version HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic \r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200"), "raw={raw}");
    assert!(raw.contains("\"version\""), "raw={raw}");
}

#[tokio::test]
#[serial]
async fn anonymous_personal_data_with_poison_basic_returns_401() {
    // The poison Basic header still blocks personal-data paths.
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/logs HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic \r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}
