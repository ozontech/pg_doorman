use crate::config::{self, ClusterDiff, ConfigDiff};
use crate::patroni::PatroniClient;
use crate::port::Port;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Cluster manager that handles ports and member updates
pub struct ClusterManager {
    /// Cluster name
    name: String,
    /// Patroni API hosts
    hosts: Vec<String>,
    /// Patroni API client
    client: PatroniClient,
    /// Active ports
    ports: Arc<RwLock<HashMap<String, Arc<Port>>>>,
    /// Update interval for polling Patroni API
    update_interval: Duration,
}

impl ClusterManager {
    pub fn new(name: String, hosts: Vec<String>, update_interval: Duration) -> Result<Self, String> {
        let client = PatroniClient::new().map_err(|e| e.to_string())?;
        Ok(Self {
            name,
            hosts,
            client,
            ports: Arc::new(RwLock::new(HashMap::new())),
            update_interval,
        })
    }

    /// Start all ports for this cluster
    pub async fn start_ports(
        &self,
        port_configs: &HashMap<String, config::PortConfig>,
    ) -> Result<(), String> {
        let mut ports = self.ports.write().await;

        for (port_name, port_config) in port_configs {
            let full_name = format!("{}:{}", self.name, port_name);
            let port = Arc::new(Port::new(full_name.clone(), port_config).map_err(|e| e.to_string())?);

            info!(
                "Starting port '{}' on {}",
                full_name,
                port.listen_addr()
            );

            // Spawn listener task
            let port_clone = Arc::clone(&port);
            tokio::spawn(async move {
                if let Err(e) = port_clone.run().await {
                    error!("Port '{}' error: {}", full_name, e);
                }
            });

            ports.insert(port_name.clone(), port);
        }

        Ok(())
    }

    /// Stop all ports
    pub async fn stop_ports(&self) {
        let ports = self.ports.read().await;
        for (port_name, port) in ports.iter() {
            info!("Stopping port '{}:{}'", self.name, port_name);
            port.stop().await;
        }
    }

    /// Update cluster members from Patroni API
    pub async fn update_members(&self) {
        match self.client.fetch_members(&self.hosts).await {
            Ok(members) => {
                debug!(
                    "Cluster '{}': fetched {} members from Patroni API",
                    self.name,
                    members.len()
                );

                let ports = self.ports.read().await;
                for (port_name, port) in ports.iter() {
                    port.update_members(members.clone()).await;
                    let backend_count = port.backend_count().await;
                    debug!(
                        "Port '{}:{}': {} backends available",
                        self.name, port_name, backend_count
                    );
                }
            }
            Err(e) => {
                warn!("Cluster '{}': failed to fetch members: {}", self.name, e);
            }
        }
    }

    /// Start periodic member updates
    pub fn start_update_loop(self: Arc<Self>) {
        let update_interval = self.update_interval;
        tokio::spawn(async move {
            loop {
                self.update_members().await;
                tokio::time::sleep(update_interval).await;
            }
        });
    }
}

pub async fn handle_config_changes(
    diff: &ConfigDiff,
    cluster_managers: &Arc<RwLock<HashMap<String, Arc<ClusterManager>>>>,
    update_interval: Duration,
) {
    for change in &diff.changes {
        match change {
            ClusterDiff::Added(name, cluster_config) => {
                // Add new cluster
                match ClusterManager::new(name.clone(), cluster_config.hosts.clone(), update_interval) {
                    Ok(manager) => {
                        let manager = Arc::new(manager);
                        
                        // Start ports
                        if let Err(e) = manager.start_ports(&cluster_config.ports).await {
                            error!("Failed to start ports for cluster '{}': {}", name, e);
                            continue;
                        }
                        
                        // Start update loop
                        manager.clone().start_update_loop();
                        
                        let mut managers = cluster_managers.write().await;
                        managers.insert(name.clone(), manager);
                        
                        info!("Cluster '{}' started successfully", name);
                    }
                    Err(e) => {
                        error!("Failed to create cluster manager for '{}': {}", name, e);
                    }
                }
            }
            ClusterDiff::Removed(name) => {
                // Remove cluster
                let mut managers = cluster_managers.write().await;
                if let Some(manager) = managers.remove(name) {
                    manager.stop_ports().await;
                    info!("Cluster '{}' stopped and removed", name);
                }
            }
            ClusterDiff::HostsChanged(name, _old, new) => {
                // Update hosts for existing cluster
                let managers = cluster_managers.read().await;
                if managers.contains_key(name) {
                    // Update hosts in the manager
                    // Note: We need to update the hosts field, but it's not mutable
                    // For now, we just log a warning that restart is needed
                    warn!(
                        "Cluster '{}' hosts changed to {:?}. Full restart required for this change to take effect.",
                        name, new
                    );
                }
            }
            ClusterDiff::PortsChanged(name, old, new) => {
                // Ports changed - incremental update: stop removed/changed, add new
                let managers = cluster_managers.read().await;
                if let Some(manager) = managers.get(name) {
                    info!("Cluster '{}' ports changed, applying incremental update", name);
                    
                    let mut ports = manager.ports.write().await;
                    
                    // Find ports to remove (in old but not in new, or config changed)
                    let mut to_remove = Vec::new();
                    for (port_name, old_config) in old {
                        match new.get(port_name) {
                            None => {
                                // Port removed
                                to_remove.push(port_name.clone());
                            }
                            Some(new_config) => {
                                // Check if config changed
                                if old_config != new_config {
                                    to_remove.push(port_name.clone());
                                }
                            }
                        }
                    }
                    
                    // Stop and remove old ports
                    for port_name in &to_remove {
                        if let Some(port) = ports.remove(port_name) {
                            info!("Stopping port '{}:{}' (removed or changed)", name, port_name);
                            port.stop().await;
                        }
                    }
                    
                    // Add new ports (not in old, or config changed)
                    for (port_name, new_config) in new {
                        let should_add = match old.get(port_name) {
                            None => true, // New port
                            Some(old_config) => old_config != new_config, // Config changed
                        };
                        
                        if should_add {
                            let full_name = format!("{}:{}", name, port_name);
                            match Port::new(full_name.clone(), new_config) {
                                Ok(port) => {
                                    let port = Arc::new(port);
                                    info!("Starting port '{}' on {}", full_name, port.listen_addr());
                                    
                                    // Spawn listener task
                                    let port_clone = Arc::clone(&port);
                                    tokio::spawn(async move {
                                        if let Err(e) = port_clone.run().await {
                                            error!("Port '{}' error: {}", full_name, e);
                                        }
                                    });
                                    
                                    ports.insert(port_name.clone(), port);
                                }
                                Err(e) => {
                                    error!("Failed to create port '{}:{}': {}", name, port_name, e);
                                }
                            }
                        }
                    }
                    
                    info!("Cluster '{}' ports updated successfully", name);
                }
            }
            ClusterDiff::TlsChanged(name) => {
                // TLS changed - log warning
                warn!(
                    "Cluster '{}' TLS configuration changed. Full restart required for this change to take effect.",
                    name
                );
            }
        }
    }
}
