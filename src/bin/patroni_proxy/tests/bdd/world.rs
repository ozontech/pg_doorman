use cucumber::World;
use std::collections::HashMap;
use std::net::TcpStream;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use tempfile::NamedTempFile;

/// The World struct holds the state shared across all steps in a scenario.
#[derive(World)]
pub struct PatroniProxyWorld {
    /// patroni_proxy process handle
    pub proxy_process: Option<Child>,
    /// Temporary config file for patroni_proxy (kept alive while process runs)
    pub proxy_config_file: Option<NamedTempFile>,
    /// Mock Patroni HTTP server shutdown flags (host_url -> shutdown flag)
    pub mock_patroni_shutdowns: HashMap<String, Arc<AtomicBool>>,
    /// Mock Patroni server ports (in order of creation)
    pub mock_patroni_ports: Vec<u16>,
    /// Mock Patroni server names to ports (server_name -> port)
    pub mock_patroni_names: HashMap<String, u16>,
    /// Mock Patroni server responses for dynamic updates (server_name -> response)
    pub mock_patroni_responses: HashMap<String, Arc<RwLock<String>>>,
    /// Proxy listen addresses (port_name -> listen_address)
    pub proxy_listen_addresses: HashMap<String, String>,
    /// API listen address
    pub api_listen_address: Option<String>,
    /// Active TCP connections (port_name -> TcpStream)
    pub active_connections: HashMap<String, TcpStream>,
    /// Mock backend servers for ping-pong testing (port_name -> MockBackend)
    pub mock_backends: HashMap<String, crate::mock_backend_helper::MockBackend>,
}

impl Default for PatroniProxyWorld {
    fn default() -> Self {
        Self {
            proxy_process: None,
            proxy_config_file: None,
            mock_patroni_shutdowns: HashMap::new(),
            mock_patroni_ports: Vec::new(),
            mock_patroni_names: HashMap::new(),
            mock_patroni_responses: HashMap::new(),
            proxy_listen_addresses: HashMap::new(),
            api_listen_address: None,
            active_connections: HashMap::new(),
            mock_backends: HashMap::new(),
        }
    }
}

impl PatroniProxyWorld {
    /// Replace all known placeholders in the given text
    pub fn replace_placeholders(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Replace named mock Patroni server ports (e.g., ${PATRONI_NODE1_PORT})
        for (server_name, port) in &self.mock_patroni_names {
            let placeholder = format!("${{PATRONI_{}_PORT}}", server_name.to_uppercase());
            result = result.replace(&placeholder, &port.to_string());
        }

        // Replace mock Patroni server ports by index (e.g., ${PATRONI_PORT_0})
        for (i, port) in self.mock_patroni_ports.iter().enumerate() {
            let placeholder = format!("${{PATRONI_PORT_{}}}", i);
            result = result.replace(&placeholder, &port.to_string());
        }

        // Replace proxy listen addresses
        for (port_name, listen_addr) in &self.proxy_listen_addresses {
            let placeholder = format!("${{PROXY_{}_ADDR}}", port_name.to_uppercase());
            result = result.replace(&placeholder, listen_addr);
        }

        // Replace API listen address
        if let Some(ref api_addr) = self.api_listen_address {
            result = result.replace("${API_ADDR}", api_addr);
        }

        // Replace mock backend server ports
        for (port_name, backend) in &self.mock_backends {
            let placeholder = format!("${{BACKEND_{}_PORT}}", port_name.to_uppercase());
            result = result.replace(&placeholder, &backend.port().to_string());
        }

        result
    }
}

impl std::fmt::Debug for PatroniProxyWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatroniProxyWorld")
            .field(
                "proxy_process",
                &self.proxy_process.as_ref().map(|p| p.id()),
            )
            .field(
                "proxy_config_file",
                &self.proxy_config_file.as_ref().map(|f| f.path()),
            )
            .field("mock_patroni_ports", &self.mock_patroni_ports)
            .field("proxy_listen_addresses", &self.proxy_listen_addresses)
            .field("api_listen_address", &self.api_listen_address)
            .finish()
    }
}
