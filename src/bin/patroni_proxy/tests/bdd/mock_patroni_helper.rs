use crate::port_allocator::allocate_port;
use crate::world::PatroniProxyWorld;
use cucumber::{gherkin::Step, given, when};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::sleep;

/// Start a named mock Patroni HTTP server with JSON response
/// Usage: Given mock Patroni server 'node1' with response:
///        """
///        {"members": [...]}
///        """
#[given(regex = r"^mock Patroni server '(.+)' with response:$")]
pub async fn start_named_mock_patroni_server(
    world: &mut PatroniProxyWorld,
    server_name: String,
    step: &Step,
) {
    let response_json = step
        .docstring
        .as_ref()
        .expect("JSON response not found")
        .to_string();

    // Replace placeholders in response JSON
    let response_json = world.replace_placeholders(&response_json);

    let port = allocate_port();

    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to bind mock Patroni server");

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let server_name_clone = server_name.clone();

    // Use Arc<RwLock<String>> for dynamic response updates
    let response_holder = Arc::new(RwLock::new(response_json));
    let response_holder_clone = Arc::clone(&response_holder);

    // Spawn HTTP server task
    tokio::spawn(async move {
        eprintln!("[MockPatroni:{server_name_clone}:{port}] Started");

        loop {
            if shutdown_clone.load(Ordering::Relaxed) {
                break;
            }

            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((mut stream, _addr)) => {
                            let response_holder = Arc::clone(&response_holder_clone);
                            let server_name = server_name_clone.clone();
                            tokio::spawn(async move {
                                let mut buffer = vec![0u8; 4096];

                                match stream.read(&mut buffer).await {
                                    Ok(n) if n > 0 => {
                                        let request = String::from_utf8_lossy(&buffer[..n]);
                                        let first_line = request.lines().next().unwrap_or("");
                                        eprintln!("[MockPatroni:{server_name}] Request: {first_line}");

                                        let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                                        let response = if path == "/cluster" {
                                            let response_json = response_holder.read().unwrap().clone();
                                            eprintln!("[MockPatroni:{server_name}] Response: {response_json}");
                                            format!(
                                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                                                response_json.len(),
                                                response_json
                                            )
                                        } else {
                                            "HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_string()
                                        };

                                        if let Err(e) = stream.write_all(response.as_bytes()).await {
                                            eprintln!("[MockPatroni:{server_name}] Write error: {e}");
                                        }
                                        if let Err(e) = stream.flush().await {
                                            eprintln!("[MockPatroni:{server_name}] Flush error: {e}");
                                        }
                                    }
                                    _ => {}
                                }

                                let _ = stream.shutdown().await;
                            });
                        }
                        Err(_) => break,
                    }
                }
                _ = sleep(Duration::from_millis(100)) => {
                    // Check shutdown flag periodically
                }
            }
        }

        eprintln!("[MockPatroni:{server_name_clone}] Stopped");
    });

    let host_url = format!("http://127.0.0.1:{port}");
    world
        .mock_patroni_shutdowns
        .insert(host_url.clone(), shutdown);
    world.mock_patroni_ports.push(port);
    world.mock_patroni_names.insert(server_name.clone(), port);
    world
        .mock_patroni_responses
        .insert(server_name, response_holder);

    // Wait for server to be ready
    wait_for_http_server_ready(port).await;
}

/// Update mock Patroni server response dynamically
/// Usage: When mock Patroni server 'node1' response is updated to:
///        """
///        {"members": [...]}
///        """
#[when(regex = r"^mock Patroni server '(.+)' response is updated to:$")]
pub async fn update_mock_patroni_response(
    world: &mut PatroniProxyWorld,
    server_name: String,
    step: &Step,
) {
    let new_response = step
        .docstring
        .as_ref()
        .expect("JSON response not found")
        .to_string();

    // Replace placeholders in response JSON
    let new_response = world.replace_placeholders(&new_response);

    let response_holder = world
        .mock_patroni_responses
        .get(&server_name)
        .unwrap_or_else(|| panic!("Mock Patroni server '{server_name}' not found"));

    let mut response = response_holder.write().unwrap();
    *response = new_response;
    eprintln!("[MockPatroni:{server_name}] Response updated");
}

/// Helper function to wait for HTTP server to be ready (max 5 seconds)
async fn wait_for_http_server_ready(port: u16) {
    for _ in 0..20 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            return;
        }
        sleep(Duration::from_millis(250)).await;
    }
    panic!("Mock Patroni server failed to start on port {port} (timeout 5s)");
}

/// Stop all mock Patroni servers
pub fn stop_mock_patroni_servers(world: &mut PatroniProxyWorld) {
    for (host_url, shutdown) in world.mock_patroni_shutdowns.drain() {
        shutdown.store(true, Ordering::Relaxed);
        eprintln!("Stopped mock Patroni server: {host_url}");
    }
    world.mock_patroni_ports.clear();
    world.mock_patroni_names.clear();
}
