use crate::port_allocator::allocate_port;
use crate::world::PatroniProxyWorld;
use cucumber::given;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Mock backend server that implements simple ping-pong protocol with NAME identification
/// Responds with "PONG\n" to "PING\n" request
/// Responds with "NAME:<server_name>\n" to "NAME\n" request
pub struct MockBackend {
    #[allow(dead_code)]
    name: String,
    port: u16,
    shutdown: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl MockBackend {
    /// Start a new named mock backend server on a free port
    pub fn start(name: String) -> Self {
        let port = allocate_port();
        let listener =
            TcpListener::bind(format!("127.0.0.1:{port}")).expect("Failed to bind mock backend");

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let name_clone = name.clone();

        let thread_handle = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("Failed to set nonblocking");

            let mut clients: Vec<TcpStream> = Vec::new();

            while !shutdown_clone.load(Ordering::Relaxed) {
                // Accept new connections
                match listener.accept() {
                    Ok((stream, addr)) => {
                        eprintln!(
                            "[MockBackend:{name_clone}:{port}] Accepted connection from {addr}"
                        );
                        stream
                            .set_nonblocking(true)
                            .expect("Failed to set nonblocking");
                        stream
                            .set_read_timeout(Some(Duration::from_millis(10)))
                            .ok();
                        clients.push(stream);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // No new connections
                    }
                    Err(e) => {
                        eprintln!("[MockBackend:{name_clone}] Accept error: {e}");
                    }
                }

                // Handle existing clients
                let initial_count = clients.len();
                clients.retain_mut(|stream| {
                    let mut buf = [0u8; 1024];
                    match stream.read(&mut buf) {
                        Ok(0) => {
                            // Connection closed by client
                            eprintln!(
                                "[MockBackend:{name_clone}:{port}] Client disconnected (read 0 bytes)"
                            );
                            false
                        }
                        Ok(n) => {
                            let request = String::from_utf8_lossy(&buf[..n]);
                            eprintln!(
                                "[MockBackend:{}:{}] Received {} bytes: {:?}",
                                name_clone,
                                port,
                                n,
                                request.trim()
                            );

                            // Handle PING command
                            if request.contains("PING") {
                                eprintln!("[MockBackend:{name_clone}:{port}] Sending PONG");
                                match stream.write_all(b"PONG\n") {
                                    Ok(_) => {
                                        let _ = stream.flush();
                                        eprintln!(
                                            "[MockBackend:{name_clone}:{port}] PONG sent successfully"
                                        );
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "[MockBackend:{name_clone}:{port}] Failed to send PONG: {e}"
                                        );
                                        return false;
                                    }
                                }
                            }

                            // Handle NAME command - returns server name for identification
                            if request.contains("NAME") {
                                let response = format!("NAME:{name_clone}\n");
                                eprintln!(
                                    "[MockBackend:{}:{}] Sending NAME response: {}",
                                    name_clone,
                                    port,
                                    response.trim()
                                );
                                match stream.write_all(response.as_bytes()) {
                                    Ok(_) => {
                                        let _ = stream.flush();
                                        eprintln!(
                                            "[MockBackend:{name_clone}:{port}] NAME sent successfully"
                                        );
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "[MockBackend:{name_clone}:{port}] Failed to send NAME: {e}"
                                        );
                                        return false;
                                    }
                                }
                            }

                            // Keep connection alive
                            true
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // No data available, keep connection alive
                            true
                        }
                        Err(e) => {
                            // Connection error
                            eprintln!(
                                "[MockBackend:{name_clone}:{port}] Client connection error: {e}"
                            );
                            false
                        }
                    }
                });
                let final_count = clients.len();
                if initial_count != final_count {
                    eprintln!(
                        "[MockBackend:{name_clone}:{port}] Active connections: {initial_count} -> {final_count}"
                    );
                }

                thread::sleep(Duration::from_millis(10));
            }
        });

        Self {
            name,
            port,
            shutdown,
            thread_handle: Some(thread_handle),
        }
    }

    /// Get the port number
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop the mock backend server
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MockBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start a named mock backend server with ping-pong protocol
/// Usage: Given mock backend server 'pg_master' for ping-pong protocol
/// Creates placeholder: ${BACKEND_PG_MASTER_PORT}
#[given(regex = r"^mock backend server '(.+)' for ping-pong protocol$")]
pub async fn start_named_mock_backend_server(world: &mut PatroniProxyWorld, server_name: String) {
    let backend = MockBackend::start(server_name.clone());
    eprintln!(
        "[MockBackend:{}] Started on port {}",
        server_name,
        backend.port()
    );
    world.mock_backends.insert(server_name, backend);
}

/// Stop all mock backend servers
pub fn stop_mock_backends(world: &mut PatroniProxyWorld) {
    for (_name, mut backend) in world.mock_backends.drain() {
        backend.stop();
    }
}
