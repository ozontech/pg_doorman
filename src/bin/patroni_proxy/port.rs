use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, RwLock};
use tracing::{debug, error, info, warn};

use crate::config::{PortConfig, Role};
use crate::patroni::Member;
use crate::stream::{spawn_proxy, StopHandle};

/// Backend member with connection counter for load balancing
#[derive(Debug)]
struct Backend {
    /// Member host
    host: String,
    /// Member port (from config, not from Member struct)
    port: u16,
    /// Current number of active connections
    connections: AtomicU64,
}

impl Backend {
    fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            connections: AtomicU64::new(0),
        }
    }

    fn addr(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        format!("{}:{}", self.host, self.port).parse()
    }

    fn increment_connections(&self) -> u64 {
        self.connections.fetch_add(1, Ordering::SeqCst)
    }

    fn decrement_connections(&self) {
        self.connections.fetch_sub(1, Ordering::SeqCst);
    }

    fn connection_count(&self) -> u64 {
        self.connections.load(Ordering::SeqCst)
    }
}

/// Port listener that distributes connections to backends using Least Connections strategy
pub struct Port {
    /// Port name (for logging)
    name: String,
    /// Listen address
    listen_addr: SocketAddr,
    /// Allowed roles for this port
    roles: HashSet<Role>,
    /// Host port to connect to on backends
    host_port: u16,
    /// Maximum allowed lag in bytes (None = no limit)
    max_lag_in_bytes: Option<u64>,
    /// Current list of available backends
    backends: Arc<RwLock<Vec<Arc<Backend>>>>,
    /// Shutdown flag
    shutdown: Arc<RwLock<bool>>,
    /// Notify for stopping the listener
    stop_notify: Arc<Notify>,
    /// Active connection stop handles, grouped by backend host
    active_connections: Arc<Mutex<HashMap<String, Vec<StopHandle>>>>,
}

impl Port {
    /// Creates a new Port from PortConfig
    pub fn new(name: String, config: &PortConfig) -> Result<Self, PortError> {
        let listen_addr: SocketAddr = config
            .listen
            .parse()
            .map_err(|e| PortError::InvalidListenAddress(format!("{}: {}", config.listen, e)))?;

        let mut roles = HashSet::new();
        for role_str in &config.roles {
            let role =
                Role::from_str(role_str).map_err(|_| PortError::InvalidRole(role_str.clone()))?;
            roles.insert(role);
        }

        Ok(Self {
            name,
            listen_addr,
            roles,
            host_port: config.host_port,
            max_lag_in_bytes: config.max_lag_in_bytes,
            backends: Arc::new(RwLock::new(Vec::new())),
            shutdown: Arc::new(RwLock::new(false)),
            stop_notify: Arc::new(Notify::new()),
            active_connections: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Updates the list of available members
    ///
    /// Filters members by:
    /// - Role matching (leader, sync, async, or any)
    /// - State must be "running"
    /// - Lag must be within max_lag_in_bytes (if configured)
    /// - noloadbalance tag must not be true
    ///
    /// When a member is removed, all its active connections are terminated.
    /// Existing backends are preserved to maintain connection counters.
    pub async fn update_members(&self, members: Vec<Member>) {
        // Filter eligible members
        let eligible_hosts: Vec<String> = members
            .into_iter()
            .filter(|m| self.is_member_eligible(m))
            .map(|m| m.host.clone())
            .collect();

        let new_hosts: HashSet<String> = eligible_hosts.iter().cloned().collect();

        // Build new backends list, reusing existing Backend objects to preserve connection counters
        let new_backends: Vec<Arc<Backend>> = {
            let current_backends = self.backends.read().await;

            // Create a map of existing backends by host for quick lookup
            let existing_map: HashMap<&String, &Arc<Backend>> =
                current_backends.iter().map(|b| (&b.host, b)).collect();

            eligible_hosts
                .iter()
                .map(|host| {
                    if let Some(existing) = existing_map.get(host) {
                        // Reuse existing backend to preserve connection counter
                        Arc::clone(existing)
                    } else {
                        // Create new backend for new host
                        Arc::new(Backend::new(host.clone(), self.host_port))
                    }
                })
                .collect()
        };

        // Get old hosts and find removed ones
        let removed_hosts: Vec<String> = {
            let backends = self.backends.read().await;
            backends
                .iter()
                .map(|b| b.host.clone())
                .filter(|h| !new_hosts.contains(h))
                .collect()
        };

        // Disconnect clients of removed backends
        if !removed_hosts.is_empty() {
            let mut active = self.active_connections.lock().await;
            let mut total_disconnected = 0;

            for host in &removed_hosts {
                if let Some(handles) = active.remove(host) {
                    let count = handles.len();
                    for handle in handles {
                        handle.stop();
                    }
                    total_disconnected += count;
                    info!(
                        "Port '{}': backend '{}' removed, disconnected {} clients",
                        self.name, host, count
                    );
                }
            }

            if total_disconnected > 0 {
                debug!(
                    "Port '{}': total {} clients disconnected due to backend removal",
                    self.name, total_disconnected
                );
            }
        }

        let count = new_backends.len();
        let mut backends = self.backends.write().await;
        *backends = new_backends;

        debug!(
            "Port '{}': updated backends, {} members available",
            self.name, count
        );
    }

    /// Checks if a member is eligible for this port
    fn is_member_eligible(&self, member: &Member) -> bool {
        // Check state - must be running
        if member.state != "running" {
            return false;
        }

        // Check noloadbalance tag
        if let Some(ref tags) = member.tags {
            if tags.noloadbalance == Some(true) {
                return false;
            }
        }

        // Check role
        let member_role = match member.get_role() {
            Some(r) => r,
            None => return false, // Unknown role - skip
        };

        let role_matches = self.roles.iter().any(|r| match r {
            Role::Any => true,
            Role::Leader => matches!(member_role, crate::patroni::Role::Leader),
            Role::Sync => matches!(member_role, crate::patroni::Role::Sync),
            Role::Async => matches!(member_role, crate::patroni::Role::Async),
        });

        if !role_matches {
            return false;
        }

        // Check lag (only for replicas, leader has no lag)
        if let Some(max_lag) = self.max_lag_in_bytes {
            if !matches!(member_role, crate::patroni::Role::Leader) {
                if let Some(ref lag_value) = member.lag {
                    // Lag can be a number or string
                    let lag: Option<u64> = if lag_value.is_u64() {
                        lag_value.as_u64()
                    } else if lag_value.is_i64() {
                        lag_value
                            .as_i64()
                            .and_then(|v| if v >= 0 { Some(v as u64) } else { None })
                    } else if lag_value.is_string() {
                        lag_value.as_str().and_then(|s| s.parse().ok())
                    } else {
                        None
                    };

                    if let Some(lag) = lag {
                        if lag > max_lag {
                            return false;
                        }
                    }
                }
            }
        }

        true
    }

    /// Selects a backend using Least Connections strategy
    async fn select_backend(&self) -> Option<Arc<Backend>> {
        let backends = self.backends.read().await;

        if backends.is_empty() {
            return None;
        }

        // Find backend with minimum connections
        backends
            .iter()
            .min_by_key(|b| b.connection_count())
            .cloned()
    }

    /// Starts listening for incoming connections
    pub async fn run(&self) -> Result<(), PortError> {
        let listener = TcpListener::bind(self.listen_addr)
            .await
            .map_err(|e| PortError::BindFailed(format!("{}: {}", self.listen_addr, e)))?;

        info!("Port '{}': listening on {}", self.name, self.listen_addr);

        loop {
            // Wait for either a new connection or stop signal
            let accept_result = tokio::select! {
                _ = self.stop_notify.notified() => {
                    info!("Port '{}': received stop signal", self.name);
                    break;
                }
                result = listener.accept() => result,
            };

            let (client_stream, client_addr) = match accept_result {
                Ok((stream, addr)) => (stream, addr),
                Err(e) => {
                    // Check if we're shutting down
                    let shutdown = self.shutdown.read().await;
                    if *shutdown {
                        break;
                    }
                    error!("Port '{}': accept error: {}", self.name, e);
                    continue;
                }
            };

            debug!("Port '{}': new connection from {}", self.name, client_addr);

            // Select backend
            let backend = match self.select_backend().await {
                Some(b) => b,
                None => {
                    warn!(
                        "Port '{}': no backends available, closing connection from {}",
                        self.name, client_addr
                    );
                    drop(client_stream);
                    continue;
                }
            };

            // Get backend address
            let backend_addr = match backend.addr() {
                Ok(addr) => addr,
                Err(e) => {
                    error!(
                        "Port '{}': invalid backend address {}:{}: {}",
                        self.name, backend.host, backend.port, e
                    );
                    continue;
                }
            };

            // Increment connection count
            backend.increment_connections();

            debug!(
                "Port '{}': routing {} -> {} (connections: {})",
                self.name,
                client_addr,
                backend_addr,
                backend.connection_count()
            );

            // Spawn proxy and store stop handle
            let (stop_handle, join_handle) = spawn_proxy(backend_addr, client_stream);

            // Store stop handle for later cleanup, grouped by backend host
            let backend_host = backend.host.clone();
            {
                let mut active = self.active_connections.lock().await;
                active
                    .entry(backend_host.clone())
                    .or_insert_with(Vec::new)
                    .push(stop_handle);
            }

            // Spawn task to wait for proxy completion and cleanup
            let name = self.name.clone();
            let backend_clone = Arc::clone(&backend);
            let active_connections = Arc::clone(&self.active_connections);

            tokio::spawn(async move {
                // Wait for proxy to complete
                match join_handle.await {
                    Ok(Ok(result)) => {
                        debug!(
                            "Port '{}': connection {} -> {} completed, \
                            transferred {} bytes to server, {} bytes to client",
                            name,
                            client_addr,
                            backend_addr,
                            result.client_to_server_bytes,
                            result.server_to_client_bytes
                        );
                    }
                    Ok(Err(e)) => {
                        debug!(
                            "Port '{}': connection {} -> {} error: {}",
                            name, client_addr, backend_addr, e
                        );
                    }
                    Err(e) => {
                        error!("Port '{}': proxy task panicked: {}", name, e);
                    }
                }

                // Decrement connection count
                backend_clone.decrement_connections();

                // Clean up completed connections (remove stopped handles)
                let mut active = active_connections.lock().await;
                if let Some(handles) = active.get_mut(&backend_host) {
                    handles.retain(|h| !h.is_stopped());
                    // Remove empty entries
                    if handles.is_empty() {
                        active.remove(&backend_host);
                    }
                }
            });
        }

        info!("Port '{}': listener stopped", self.name);
        Ok(())
    }

    /// Stops the port: stops accepting new connections and terminates all active TCP connections
    pub async fn stop(&self) {
        info!("Port '{}': stopping...", self.name);

        // Set shutdown flag
        {
            let mut shutdown = self.shutdown.write().await;
            *shutdown = true;
        }

        // Notify listener to stop
        self.stop_notify.notify_waiters();

        // Stop all active connections
        let connections = {
            let mut active = self.active_connections.lock().await;
            std::mem::take(&mut *active)
        };

        let mut connection_count = 0;
        for (_host, handles) in connections {
            for stop_handle in handles {
                stop_handle.stop();
                connection_count += 1;
            }
        }

        info!(
            "Port '{}': stopped, terminated {} active connections",
            self.name, connection_count
        );
    }

    /// Returns the listen address
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Returns the port name
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns current number of backends
    pub async fn backend_count(&self) -> usize {
        self.backends.read().await.len()
    }
}

/// Port errors
#[derive(Debug)]
pub enum PortError {
    /// Invalid listen address
    InvalidListenAddress(String),
    /// Invalid role
    InvalidRole(String),
    /// Failed to bind to address
    BindFailed(String),
}

impl std::fmt::Display for PortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PortError::InvalidListenAddress(e) => write!(f, "Invalid listen address: {e}"),
            PortError::InvalidRole(r) => write!(f, "Invalid role: {r}"),
            PortError::BindFailed(e) => write!(f, "Failed to bind: {e}"),
        }
    }
}

impl std::error::Error for PortError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patroni::MemberTags;
    use serde_json::json;

    fn create_test_config() -> PortConfig {
        PortConfig {
            listen: "127.0.0.1:15432".to_string(),
            roles: vec!["leader".to_string()],
            host_port: 6432,
            max_lag_in_bytes: None,
        }
    }

    fn create_member(name: &str, role: &str, state: &str, host: &str, lag: Option<u64>) -> Member {
        Member {
            name: name.to_string(),
            role: role.to_string(),
            state: state.to_string(),
            api_url: format!("http://{host}:8008/patroni"),
            host: host.to_string(),
            port: 5432,
            timeline: json!(1),
            lag: lag.map(|l| json!(l)),
            tags: None,
        }
    }

    #[test]
    fn test_port_creation() {
        let config = create_test_config();
        let port = Port::new("test".to_string(), &config).unwrap();

        assert_eq!(port.name(), "test");
        assert_eq!(port.listen_addr().to_string(), "127.0.0.1:15432");
        assert_eq!(port.host_port, 6432);
    }

    #[test]
    fn test_member_eligibility_by_role() {
        let config = PortConfig {
            listen: "127.0.0.1:15432".to_string(),
            roles: vec!["leader".to_string()],
            host_port: 6432,
            max_lag_in_bytes: None,
        };
        let port = Port::new("test".to_string(), &config).unwrap();

        let leader = create_member("node1", "leader", "running", "192.168.0.1", None);
        let replica = create_member("node2", "replica", "running", "192.168.0.2", Some(0));

        assert!(port.is_member_eligible(&leader));
        assert!(!port.is_member_eligible(&replica));
    }

    #[test]
    fn test_member_eligibility_any_role() {
        let config = PortConfig {
            listen: "127.0.0.1:15432".to_string(),
            roles: vec!["any".to_string()],
            host_port: 6432,
            max_lag_in_bytes: None,
        };
        let port = Port::new("test".to_string(), &config).unwrap();

        let leader = create_member("node1", "leader", "running", "192.168.0.1", None);
        let replica = create_member("node2", "replica", "running", "192.168.0.2", Some(0));
        let sync = create_member("node3", "sync_standby", "running", "192.168.0.3", Some(0));

        assert!(port.is_member_eligible(&leader));
        assert!(port.is_member_eligible(&replica));
        assert!(port.is_member_eligible(&sync));
    }

    #[test]
    fn test_member_eligibility_by_state() {
        let config = create_test_config();
        let port = Port::new("test".to_string(), &config).unwrap();

        let running = create_member("node1", "leader", "running", "192.168.0.1", None);
        let stopped = create_member("node2", "leader", "stopped", "192.168.0.2", None);

        assert!(port.is_member_eligible(&running));
        assert!(!port.is_member_eligible(&stopped));
    }

    #[test]
    fn test_member_eligibility_by_lag() {
        let config = PortConfig {
            listen: "127.0.0.1:15432".to_string(),
            roles: vec!["async".to_string()],
            host_port: 6432,
            max_lag_in_bytes: Some(1000),
        };
        let port = Port::new("test".to_string(), &config).unwrap();

        let low_lag = create_member("node1", "replica", "running", "192.168.0.1", Some(500));
        let high_lag = create_member("node2", "replica", "running", "192.168.0.2", Some(2000));

        assert!(port.is_member_eligible(&low_lag));
        assert!(!port.is_member_eligible(&high_lag));
    }

    #[test]
    fn test_member_eligibility_noloadbalance() {
        let config = create_test_config();
        let port = Port::new("test".to_string(), &config).unwrap();

        let mut member = create_member("node1", "leader", "running", "192.168.0.1", None);
        member.tags = Some(MemberTags {
            clonefrom: None,
            noloadbalance: Some(true),
            replicatefrom: None,
            nosync: None,
            nofailover: None,
            extra: std::collections::HashMap::new(),
        });

        assert!(!port.is_member_eligible(&member));
    }

    #[tokio::test]
    async fn test_update_members() {
        let config = PortConfig {
            listen: "127.0.0.1:15432".to_string(),
            roles: vec!["any".to_string()],
            host_port: 6432,
            max_lag_in_bytes: None,
        };
        let port = Port::new("test".to_string(), &config).unwrap();

        let members = vec![
            create_member("node1", "leader", "running", "192.168.0.1", None),
            create_member("node2", "replica", "running", "192.168.0.2", Some(0)),
            create_member("node3", "replica", "stopped", "192.168.0.3", Some(0)),
        ];

        port.update_members(members).await;

        // Only 2 members should be available (node3 is stopped)
        assert_eq!(port.backend_count().await, 2);
    }

    #[tokio::test]
    async fn test_least_connections_selection() {
        let config = PortConfig {
            listen: "127.0.0.1:15432".to_string(),
            roles: vec!["any".to_string()],
            host_port: 6432,
            max_lag_in_bytes: None,
        };
        let port = Port::new("test".to_string(), &config).unwrap();

        let members = vec![
            create_member("node1", "leader", "running", "192.168.0.1", None),
            create_member("node2", "replica", "running", "192.168.0.2", Some(0)),
        ];

        port.update_members(members).await;

        // First selection - should get one of the backends
        let backend1 = port.select_backend().await.unwrap();
        backend1.increment_connections();

        // Second selection - should get the other backend (with fewer connections)
        let backend2 = port.select_backend().await.unwrap();

        // They should be different (one has 1 connection, other has 0)
        assert_ne!(backend1.host, backend2.host);
    }

    #[tokio::test]
    async fn test_port_stop_listener() {
        use std::sync::Arc;
        use std::time::Duration;

        let config = PortConfig {
            listen: "127.0.0.1:0".to_string(), // Use port 0 for automatic assignment
            roles: vec!["any".to_string()],
            host_port: 6432,
            max_lag_in_bytes: None,
        };
        let port = Arc::new(Port::new("test_stop".to_string(), &config).unwrap());

        // Start the port in a separate task
        let port_clone = Arc::clone(&port);
        let run_handle = tokio::spawn(async move { port_clone.run().await });

        // Give the listener some time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Stop the port
        port.stop().await;

        // The run task should complete within a reasonable time
        let result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(result.is_ok(), "Port should stop within timeout");

        // The run() should return Ok
        let run_result = result.unwrap();
        assert!(run_result.is_ok(), "run_handle should not panic");
        assert!(run_result.unwrap().is_ok(), "run() should return Ok");
    }

    #[tokio::test]
    async fn test_port_stop_with_active_connections() {
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        // Create a mock backend server
        let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();

        // Start backend that keeps connection open
        let backend_task = tokio::spawn(async move {
            loop {
                match backend_listener.accept().await {
                    Ok((mut stream, _)) => {
                        // Keep connection open until it's closed
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            let _ = stream.shutdown().await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        // First bind a listener to get an available port
        let temp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener); // Release the port

        let config = PortConfig {
            listen: format!("127.0.0.1:{listen_port}"),
            roles: vec!["any".to_string()],
            host_port: backend_addr.port(),
            max_lag_in_bytes: None,
        };
        let port = Arc::new(Port::new("test_stop_conn".to_string(), &config).unwrap());

        // Add a backend member
        let members = vec![Member {
            name: "node1".to_string(),
            role: "leader".to_string(),
            state: "running".to_string(),
            api_url: format!("http://{}:8008/patroni", backend_addr.ip()),
            host: backend_addr.ip().to_string(),
            port: backend_addr.port(),
            timeline: json!(1),
            lag: None,
            tags: None,
        }];
        port.update_members(members).await;

        // Start the port
        let port_clone = Arc::clone(&port);
        let run_handle = tokio::spawn(async move { port_clone.run().await });

        // Give the listener time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect a client to the port
        let listen_addr = port.listen_addr();
        let _client = tokio::net::TcpStream::connect(listen_addr).await.unwrap();

        // Give time for connection to be established and proxied
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify we have an active connection
        let active_count = port.active_connections.lock().await.len();
        assert!(
            active_count > 0,
            "Should have at least one active connection"
        );

        // Stop the port - this should terminate all connections
        port.stop().await;

        // The run task should complete
        let result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(result.is_ok(), "Port should stop within timeout");

        // Active connections should be cleared
        let active_count_after = port.active_connections.lock().await.len();
        assert_eq!(
            active_count_after, 0,
            "All active connections should be cleared after stop"
        );

        // Cleanup
        backend_task.abort();
    }

    #[tokio::test]
    async fn test_update_members_disconnects_removed_backend_clients() {
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        // Create two mock backend servers
        let backend1_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend1_addr = backend1_listener.local_addr().unwrap();

        let backend2_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend2_addr = backend2_listener.local_addr().unwrap();

        // Start backends that keep connections open
        let backend1_task = tokio::spawn(async move {
            loop {
                match backend1_listener.accept().await {
                    Ok((mut stream, _)) => {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            let _ = stream.shutdown().await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        let backend2_task = tokio::spawn(async move {
            loop {
                match backend2_listener.accept().await {
                    Ok((mut stream, _)) => {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            let _ = stream.shutdown().await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        // Get an available port for the proxy
        let temp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let config = PortConfig {
            listen: format!("127.0.0.1:{listen_port}"),
            roles: vec!["any".to_string()],
            host_port: backend1_addr.port(), // Will be overridden by member host
            max_lag_in_bytes: None,
        };
        let port = Arc::new(Port::new("test_disconnect".to_string(), &config).unwrap());

        // Add both backend members
        let members = vec![
            Member {
                name: "node1".to_string(),
                role: "leader".to_string(),
                state: "running".to_string(),
                api_url: format!("http://{}:8008/patroni", backend1_addr.ip()),
                host: backend1_addr.ip().to_string(),
                port: backend1_addr.port(),
                timeline: json!(1),
                lag: None,
                tags: None,
            },
            Member {
                name: "node2".to_string(),
                role: "replica".to_string(),
                state: "running".to_string(),
                api_url: format!("http://{}:8008/patroni", backend2_addr.ip()),
                host: "127.0.0.2".to_string(), // Different host to distinguish
                port: backend2_addr.port(),
                timeline: json!(1),
                lag: Some(json!(0)),
                tags: None,
            },
        ];
        port.update_members(members).await;

        // Start the port
        let port_clone = Arc::clone(&port);
        let run_handle = tokio::spawn(async move { port_clone.run().await });

        // Give the listener time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect clients to the port - they will be distributed to backends
        let listen_addr = port.listen_addr();
        let _client1 = tokio::net::TcpStream::connect(listen_addr).await.unwrap();
        let _client2 = tokio::net::TcpStream::connect(listen_addr).await.unwrap();

        // Give time for connections to be established
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify we have active connections
        let active_hosts: Vec<String> = {
            let active = port.active_connections.lock().await;
            active.keys().cloned().collect()
        };
        assert!(!active_hosts.is_empty(), "Should have active connections");

        // Now update members - remove node2 (127.0.0.2)
        let updated_members = vec![Member {
            name: "node1".to_string(),
            role: "leader".to_string(),
            state: "running".to_string(),
            api_url: format!("http://{}:8008/patroni", backend1_addr.ip()),
            host: backend1_addr.ip().to_string(),
            port: backend1_addr.port(),
            timeline: json!(1),
            lag: None,
            tags: None,
        }];
        port.update_members(updated_members).await;

        // Give time for disconnection to process
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify that connections to removed backend (127.0.0.2) are gone
        {
            let active = port.active_connections.lock().await;
            assert!(
                !active.contains_key("127.0.0.2"),
                "Connections to removed backend should be disconnected"
            );
        }

        // Stop the port
        port.stop().await;

        // The run task should complete
        let result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(result.is_ok(), "Port should stop within timeout");

        // Cleanup
        backend1_task.abort();
        backend2_task.abort();
    }
}
