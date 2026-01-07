//! HTTP server for Prometheus metrics endpoint.

use flate2::write::GzEncoder;
use flate2::Compression;
use log::{error, info};
use prometheus::{Encoder, TextEncoder};
use std::io::Write;
use std::net::SocketAddr;
use tokio::net::TcpSocket;

use super::metrics::update_metrics;
use super::REGISTRY;

/// Handles HTTP requests for metrics
pub async fn handle_metrics_request(stream: tokio::net::TcpStream) {
    // Clone the stream for reading
    let (read_half, write_half) = stream.into_split();
    let mut stream_reader = tokio::io::BufReader::new(read_half);
    let mut connection = tokio::io::BufWriter::new(write_half);
    let mut headers = [0; 1024];

    // Read HTTP request headers
    let n = match tokio::io::AsyncReadExt::read(&mut stream_reader, &mut headers).await {
        Ok(n) => n,
        Err(e) => {
            error!("Failed to read HTTP request: {e}");
            return;
        }
    };

    let headers_str = match std::str::from_utf8(&headers[..n]) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to parse HTTP headers: {e}");
            return;
        }
    };

    // Check if client accepts gzip encoding
    let accepts_gzip =
        headers_str.contains("Accept-Encoding") && headers_str.to_lowercase().contains("gzip");

    // Update metrics before serving
    update_metrics();

    // Encode metrics to the Prometheus text format
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();

    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {e}");
        return;
    }

    let content_type = encoder.format_type();

    // Prepare response body (compressed or not)
    let (response_body, content_encoding) = if accepts_gzip {
        // Compress the buffer with gzip
        let mut compressed = Vec::new();
        {
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            if let Err(e) = encoder.write_all(&buffer) {
                error!("Failed to compress metrics data: {e}");
                return;
            }
            if let Err(e) = encoder.finish() {
                error!("Failed to finish gzip compression: {e}");
                return;
            }
        }
        (compressed, "Content-Encoding: gzip\r\n")
    } else {
        (buffer, "")
    };

    // Prepare HTTP response
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n{}Content-Length: {}\r\n\r\n",
        content_type,
        content_encoding,
        response_body.len()
    );

    // Send response
    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut connection, response.as_bytes()).await
    {
        error!("Failed to write HTTP response header: {e}");
        return;
    }

    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut connection, &response_body).await {
        error!("Failed to write metrics data: {e}");
        return;
    }

    if let Err(e) = tokio::io::AsyncWriteExt::flush(&mut connection).await {
        error!("Failed to flush connection: {e}");
    }
}

/// Starts the prometheus exporter
pub async fn start_prometheus_server(host: &str) {
    info!("Starting prometheus exporter on {host}");
    let addr: SocketAddr = match host.parse() {
        Ok(addr) => addr,
        Err(e) => {
            panic!("Failed to parse socket address '{host}': {e}");
        }
    };
    let listen_socket = if addr.is_ipv4() {
        match TcpSocket::new_v4() {
            Ok(socket) => socket,
            Err(e) => {
                panic!("Failed to create IPv4 socket: {e}");
            }
        }
    } else {
        match TcpSocket::new_v6() {
            Ok(socket) => socket,
            Err(e) => {
                panic!("Failed to create IPv6 socket: {e}");
            }
        }
    };
    if let Err(e) = listen_socket.set_reuseaddr(true) {
        panic!("Failed to set SO_REUSEADDR: {e}");
    }

    if let Err(e) = listen_socket.set_reuseport(true) {
        panic!("Failed to set SO_REUSEPORT: {e}");
    }

    if let Err(e) = listen_socket.bind(addr) {
        panic!("Failed to bind to address {addr}: {e}");
    }
    match listen_socket.listen(1024) {
        Ok(listener) => {
            info!("prometheus exporter listening on {addr}");

            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(async move {
                            handle_metrics_request(stream).await;
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {e}");
                    }
                }
            }
        }
        Err(e) => {
            panic!("Failed to bind Prometheus metrics server to {addr}: {e}");
        }
    }
}
