use crate::world::PatroniProxyWorld;
use cucumber::{given, then, when};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Check that TCP connection to a specific proxy port succeeds
#[then(regex = r"^TCP connection to proxy port '(.+)' succeeds$")]
pub async fn tcp_connection_succeeds(world: &mut PatroniProxyWorld, port_name: String) {
    let listen_addr = world
        .proxy_listen_addresses
        .get(&port_name)
        .unwrap_or_else(|| panic!("Port '{port_name}' not found in proxy_listen_addresses"));

    match TcpStream::connect_timeout(
        &listen_addr.parse().expect("Invalid listen address"),
        Duration::from_secs(5),
    ) {
        Ok(_stream) => {
            // Connection succeeded
        }
        Err(e) => {
            panic!("Failed to connect to proxy port '{port_name}' at {listen_addr}: {e}");
        }
    }
}

/// Check that TCP connection to a specific proxy port fails
#[then(regex = r"^TCP connection to proxy port '(.+)' fails$")]
pub async fn tcp_connection_fails(world: &mut PatroniProxyWorld, port_name: String) {
    let listen_addr = world
        .proxy_listen_addresses
        .get(&port_name)
        .unwrap_or_else(|| panic!("Port '{port_name}' not found in proxy_listen_addresses"));

    match TcpStream::connect_timeout(
        &listen_addr.parse().expect("Invalid listen address"),
        Duration::from_millis(500),
    ) {
        Ok(_stream) => {
            panic!(
                "Connection to proxy port '{port_name}' at {listen_addr} succeeded, but was expected to fail"
            );
        }
        Err(_) => {
            // Connection failed as expected
        }
    }
}

/// Check that all proxy ports are accepting connections
#[then("all proxy ports are accepting connections")]
pub async fn all_proxy_ports_accepting(world: &mut PatroniProxyWorld) {
    for (port_name, listen_addr) in &world.proxy_listen_addresses {
        match TcpStream::connect_timeout(
            &listen_addr.parse().expect("Invalid listen address"),
            Duration::from_secs(5),
        ) {
            Ok(_stream) => {
                // Connection succeeded
            }
            Err(e) => {
                panic!("Failed to connect to proxy port '{port_name}' at {listen_addr}: {e}");
            }
        }
    }
}

/// Open a named session to a specific proxy port
#[given(regex = r"^I open session to '(.+)' named '(.+)'$")]
pub async fn open_session(world: &mut PatroniProxyWorld, port_name: String, session_name: String) {
    let listen_addr = world
        .proxy_listen_addresses
        .get(&port_name)
        .unwrap_or_else(|| panic!("Port '{port_name}' not found in proxy_listen_addresses"));

    let stream = TcpStream::connect_timeout(
        &listen_addr.parse().expect("Invalid listen address"),
        Duration::from_secs(5),
    )
    .unwrap_or_else(|e| {
        panic!(
            "Failed to open session '{session_name}' to proxy port '{port_name}' at {listen_addr}: {e}"
        )
    });

    // Set read timeout for operations
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("Failed to set read timeout");

    // Set write timeout for operations
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("Failed to set write timeout");

    // Set blocking mode
    stream
        .set_nonblocking(false)
        .expect("Failed to set blocking mode");

    world.active_connections.insert(session_name, stream);
}

/// Execute ping-pong on a named session
#[then(regex = r"^I execute ping on session '(.+)' and receive pong$")]
pub async fn ping_pong_session(world: &mut PatroniProxyWorld, session_name: String) {
    // Get the session
    let stream = world
        .active_connections
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("No session found with name '{session_name}'"));

    // Send PING
    if let Err(e) = stream.write_all(b"PING\n") {
        panic!("Failed to send PING to session '{session_name}': {e}. Connection was closed.");
    }

    // Read PONG response
    let mut buf = [0u8; 1024];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => {
            let response = String::from_utf8_lossy(&buf[..n]);
            if !response.trim().starts_with("PONG") {
                panic!(
                    "Expected PONG response from session '{}', got: {}",
                    session_name,
                    response.trim()
                );
            }
            // Ping-pong successful
        }
        Ok(_) => {
            panic!("Session '{session_name}' was closed (read returned 0 bytes)");
        }
        Err(e) => {
            panic!(
                "Failed to read PONG from session '{session_name}': {e}. Connection was closed."
            );
        }
    }
}

/// Call API /update_clusters endpoint
#[when("API /update_clusters is called")]
pub async fn call_update_clusters_api(world: &mut PatroniProxyWorld) {
    let api_addr = world
        .api_listen_address
        .as_ref()
        .expect("API listen address not set");

    let mut stream = TcpStream::connect_timeout(
        &api_addr.parse().expect("Invalid API address"),
        Duration::from_secs(5),
    )
    .expect("Failed to connect to API");

    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("Failed to set read timeout");

    // Send HTTP GET request
    let request =
        format!("GET /update_clusters HTTP/1.1\r\nHost: {api_addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("Failed to send request");

    // Read response
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("Failed to read response");

    if !response.contains("200 OK") {
        panic!("API /update_clusters failed: {response}");
    }
}

/// Check that session is closed (ping fails)
#[then(regex = r"^session '(.+)' is closed$")]
pub async fn session_is_closed(world: &mut PatroniProxyWorld, session_name: String) {
    let stream = world
        .active_connections
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("No session found with name '{session_name}'"));

    // Try to send PING - should fail
    match stream.write_all(b"PING\n") {
        Ok(_) => {
            // Write succeeded, try to read - should fail or return 0
            let mut buf = [0u8; 1024];
            match stream.read(&mut buf) {
                Ok(0) => {
                    // Connection closed as expected
                }
                Ok(n) => {
                    let response = String::from_utf8_lossy(&buf[..n]);
                    panic!(
                        "Session '{}' is still alive, got response: {}",
                        session_name,
                        response.trim()
                    );
                }
                Err(_) => {
                    // Connection error - session is closed
                }
            }
        }
        Err(_) => {
            // Write failed - session is closed
        }
    }
}

/// Check that session is still alive (ping succeeds)
#[then(regex = r"^session '(.+)' is still alive$")]
pub async fn session_is_alive(world: &mut PatroniProxyWorld, session_name: String) {
    ping_pong_session(world, session_name).await;
}

/// Check that session is connected to a specific backend using NAME protocol
/// Usage: Then session 'session1' is connected to backend 'replica1'
#[then(regex = r"^session '(.+)' is connected to backend '(.+)'$")]
pub async fn session_connected_to_backend(
    world: &mut PatroniProxyWorld,
    session_name: String,
    expected_backend: String,
) {
    let stream = world
        .active_connections
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("No session found with name '{session_name}'"));

    // Send NAME command
    if let Err(e) = stream.write_all(b"NAME\n") {
        panic!("Failed to send NAME to session '{session_name}': {e}. Connection was closed.");
    }

    // Read NAME response
    let mut buf = [0u8; 1024];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => {
            let response = String::from_utf8_lossy(&buf[..n]);
            let response = response.trim();

            // Expected format: "NAME:<backend_name>"
            if let Some(backend_name) = response.strip_prefix("NAME:") {
                if backend_name != expected_backend {
                    panic!(
                        "Session '{session_name}' is connected to backend '{backend_name}', expected '{expected_backend}'"
                    );
                }
                // Backend matches
            } else {
                panic!("Invalid NAME response from session '{session_name}': {response}");
            }
        }
        Ok(_) => {
            panic!("Session '{session_name}' was closed (read returned 0 bytes)");
        }
        Err(e) => {
            panic!(
                "Failed to read NAME from session '{session_name}': {e}. Connection was closed."
            );
        }
    }
}
