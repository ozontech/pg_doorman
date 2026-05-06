//! End-to-end smoke tests for the web listener mux.
//!
//! Each test spawns a real listener on `127.0.0.1:<random port>` (chosen via
//! `portpicker`, already in dev-dependencies), opens a TcpStream and sends a
//! hand-rolled HTTP/1.1 request line + headers. We avoid pulling reqwest into
//! dev-deps for these few cases.

use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::web::{start_web_server, WebServerOptions};

fn opts(ui_active: bool, ui_anonymous: bool) -> WebServerOptions {
    WebServerOptions {
        ui_active,
        ui_anonymous,
        admin_username: "admin".into(),
        admin_password: "secret".into(),
    }
}

async fn spawn_server(opts: WebServerOptions) -> u16 {
    let port = portpicker::pick_unused_port().expect("free port");
    let host = format!("127.0.0.1:{port}");
    tokio::spawn(async move {
        start_web_server(&host, opts).await;
    });
    // small wait for bind; matches existing test pattern
    tokio::time::sleep(Duration::from_millis(150)).await;
    port
}

async fn send(port: u16, request: &str) -> String {
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
async fn metrics_endpoint_serves_prometheus_body_when_ui_inactive() {
    let port = spawn_server(opts(false, true)).await;
    let raw = send(port, "GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.contains("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("pg_doorman_"), "raw={raw}");
}

#[tokio::test]
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
async fn api_admin_route_returns_401_without_auth() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/logs HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
    assert!(raw.contains("WWW-Authenticate: Basic"), "raw={raw}");
}

#[tokio::test]
async fn api_admin_route_with_auth_returns_501() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/logs HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    assert!(raw.starts_with("HTTP/1.1 501"), "raw={raw}");
}

#[tokio::test]
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
async fn api_version_returns_json() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/version HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("Content-Type: application/json"), "raw={raw}");
    assert!(raw.contains("\"version\""), "raw={raw}");
    assert!(raw.contains("\"git_commit\""), "raw={raw}");
}

#[tokio::test]
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
async fn api_pools_returns_json_when_ui_active() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/pools HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"pools\""), "raw={raw}");
}

#[tokio::test]
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
async fn api_servers_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/servers HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"servers\""), "raw={raw}");
}

#[tokio::test]
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
async fn api_stats_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/stats HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"stats\""), "raw={raw}");
}

#[tokio::test]
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
async fn api_users_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/users HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"users\""), "raw={raw}");
}

#[tokio::test]
async fn api_config_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/config HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"config\""), "raw={raw}");
}

#[tokio::test]
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
async fn api_sockets_returns_503_on_non_linux() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/sockets HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 503"), "raw={raw}");
}
