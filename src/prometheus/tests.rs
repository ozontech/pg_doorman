//! Tests for Prometheus metrics exporter.

use super::start_prometheus_server;
use crate::stats::{
    CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// Test for the HTTP server functionality
// This test is focused on the public interface of the prometheus module
#[tokio::test]
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
        start_prometheus_server(server_addr).await;
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
        start_prometheus_server(server_addr).await;
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
