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
