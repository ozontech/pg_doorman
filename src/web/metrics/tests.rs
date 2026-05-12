//! Tests for Prometheus metrics exporter.

use crate::stats::{
    CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};
use crate::web::{start_web_server, WebServerOptions};
use serial_test::serial;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// Test for the HTTP server functionality
// This test is focused on the public interface of the prometheus module
//
// `#[serial]` because `start_web_server` writes the process-wide
// `WebServerOptions` slot used by every other web::tests test.
#[tokio::test]
#[serial]
async fn test_prometheus_server_basic() {
    // Set up some test metrics
    PLAIN_CONNECTION_COUNTER.store(10, Ordering::SeqCst);
    TLS_CONNECTION_COUNTER.store(20, Ordering::SeqCst);
    CANCEL_CONNECTION_COUNTER.store(5, Ordering::SeqCst);
    TOTAL_CONNECTION_COUNTER.store(35, Ordering::SeqCst);

    // Start the server in a separate task
    // Use a random high port to avoid conflicts
    let server_addr = "127.0.0.1:16432";
    let server_handle = tokio::spawn(async move {
        // This will run indefinitely, so we'll abort it after the test
        start_web_server(
            server_addr,
            WebServerOptions {
                ui_active: false,
                ui_anonymous: true,
                admin_username: "admin".into(),
                admin_password: "secret".into(),
                sso: None,
                sso_config_error: None,
                trusted_proxies: Vec::new(),
                sso_admin_groups_configured: false,
                sso_require_https: false,
            },
        )
        .await;
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect to the server
    let mut stream =
        match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(server_addr)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                server_handle.abort();
                panic!("Failed to connect to server: {e}");
            }
            Err(_) => {
                server_handle.abort();
                panic!("Timed out connecting to server");
            }
        };

    // Send a simple HTTP request
    let request = "GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    // Read the response
    let mut response = Vec::new();
    let mut buf = [0u8; 1024];

    // Set a timeout for reading
    match tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    response.extend_from_slice(&buf[..n]);
                    if response.len() > 100 {
                        // Just need enough to verify headers
                        break;
                    }
                }
                Err(e) => {
                    panic!("Failed to read from socket: {e}");
                }
            }
        }
    })
    .await
    {
        Ok(_) => {}
        Err(_) => {
            server_handle.abort();
            panic!("Timed out reading response");
        }
    }

    // Convert response to string for easier inspection
    let response_str = String::from_utf8_lossy(&response);

    // Verify response contains expected headers
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "Response should contain 200 OK status"
    );
    assert!(
        response_str.contains("Content-Type: text/plain"),
        "Response should have text/plain content type"
    );

    // Verify response contains expected metrics
    assert!(
        response_str.contains("pg_doorman_connection_count"),
        "Response should contain connection count metric"
    );

    // Clean up
    server_handle.abort();

    // Reset metrics
    PLAIN_CONNECTION_COUNTER.store(0, Ordering::SeqCst);
    TLS_CONNECTION_COUNTER.store(0, Ordering::SeqCst);
    CANCEL_CONNECTION_COUNTER.store(0, Ordering::SeqCst);
    TOTAL_CONNECTION_COUNTER.store(0, Ordering::SeqCst);
}

#[test]
fn test_streaming_counters_register_and_increment() {
    use crate::web::metrics::{
        observe_streaming_bytes, observe_streaming_event, STREAMING_BYTES_TOTAL,
        STREAMING_EVENTS_TOTAL,
    };

    let user = "alice_stream";
    let database = "shop_stream";

    observe_streaming_event(user, database, "data_row", "ok");
    observe_streaming_event(user, database, "data_row", "error");
    observe_streaming_event(user, database, "copy_data", "ok");
    observe_streaming_bytes(user, database, "data_row", 16_777_216);
    observe_streaming_bytes(user, database, "copy_data", 8_388_608);

    assert_eq!(
        STREAMING_EVENTS_TOTAL
            .with_label_values(&[user, database, "data_row", "ok"])
            .get(),
        1
    );
    assert_eq!(
        STREAMING_EVENTS_TOTAL
            .with_label_values(&[user, database, "data_row", "error"])
            .get(),
        1
    );
    assert_eq!(
        STREAMING_EVENTS_TOTAL
            .with_label_values(&[user, database, "copy_data", "ok"])
            .get(),
        1
    );
    assert_eq!(
        STREAMING_BYTES_TOTAL
            .with_label_values(&[user, database, "data_row"])
            .get(),
        16_777_216
    );
    assert_eq!(
        STREAMING_BYTES_TOTAL
            .with_label_values(&[user, database, "copy_data"])
            .get(),
        8_388_608
    );
}

#[test]
fn test_pool_state_gauges_register_and_export() {
    use crate::web::metrics::{SHOW_POOLS_MAXWAIT_MICROSECONDS, SHOW_POOLS_PAUSED};
    use prometheus::core::Collector;

    SHOW_POOLS_PAUSED
        .with_label_values(&["alice", "shop"])
        .set(1);
    SHOW_POOLS_MAXWAIT_MICROSECONDS
        .with_label_values(&["alice", "shop"])
        .set(750_000.0);

    let descs: Vec<_> = SHOW_POOLS_PAUSED
        .desc()
        .iter()
        .map(|d| d.fq_name.clone())
        .collect();
    assert!(descs.iter().any(|n| n == "pg_doorman_pools_paused"));
    let descs: Vec<_> = SHOW_POOLS_MAXWAIT_MICROSECONDS
        .desc()
        .iter()
        .map(|d| d.fq_name.clone())
        .collect();
    assert!(descs
        .iter()
        .any(|n| n == "pg_doorman_pools_maxwait_microseconds"));

    assert_eq!(
        SHOW_POOLS_PAUSED
            .with_label_values(&["alice", "shop"])
            .get(),
        1
    );
    assert!(
        (SHOW_POOLS_MAXWAIT_MICROSECONDS
            .with_label_values(&["alice", "shop"])
            .get()
            - 750_000.0)
            .abs()
            < 0.5
    );

    SHOW_POOLS_PAUSED.reset();
    SHOW_POOLS_MAXWAIT_MICROSECONDS.reset();
}

// Integration test for the full server
// This is more complex and would start the actual server
#[tokio::test]
#[ignore] // Ignore by default as it requires network access and might conflict with other tests
async fn test_prometheus_server_integration() {
    use std::time::Duration;
    use tokio::net::TcpStream;
    use tokio::time::timeout;

    // Start the server in a separate task
    // Use a random high port to avoid conflicts
    let server_addr = "127.0.0.1:16432";
    let server_handle = tokio::spawn(async move {
        start_web_server(
            server_addr,
            WebServerOptions {
                ui_active: false,
                ui_anonymous: true,
                admin_username: "admin".into(),
                admin_password: "secret".into(),
                sso: None,
                sso_config_error: None,
                trusted_proxies: Vec::new(),
                sso_admin_groups_configured: false,
                sso_require_https: false,
            },
        )
        .await;
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect to the server
    let mut stream = match timeout(Duration::from_secs(1), TcpStream::connect(server_addr)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            server_handle.abort();
            panic!("Failed to connect to server: {e}");
        }
        Err(_) => {
            server_handle.abort();
            panic!("Timed out connecting to server");
        }
    };

    // Send a simple HTTP request
    let request = "GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n";
    stream.write_all(request.as_bytes()).await.unwrap();

    // Read the response
    let mut response = Vec::new();
    let mut buf = [0u8; 1024];

    // Set a timeout for reading
    match timeout(Duration::from_secs(2), async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    response.extend_from_slice(&buf[..n]);
                    if response.len() > 100 {
                        // Just need enough to verify headers
                        break;
                    }
                }
                Err(e) => {
                    panic!("Failed to read from socket: {e}");
                }
            }
        }
    })
    .await
    {
        Ok(_) => {}
        Err(_) => {
            server_handle.abort();
            panic!("Timed out reading response");
        }
    }

    // Convert response to string for easier inspection
    let response_str = String::from_utf8_lossy(&response);

    // Verify response contains expected headers
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "Response should contain 200 OK status"
    );
    assert!(
        response_str.contains("Content-Type: text/plain"),
        "Response should have text/plain content type"
    );

    // Clean up
    server_handle.abort();
}
