// This module is under active development, suppress unused warnings
#![allow(unused)]

mod config;
mod patroni;
mod port;
mod stream;

use config::{ClusterDiff, ConfigDiff, ConfigRepository};
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "patroni_proxy.yaml".to_string());

    info!("Starting patroni-proxy with config: {}", config_path);

    // Load configuration
    let config_repo = Arc::new(ConfigRepository::new(&config_path).map_err(|e| {
        error!("Failed to load configuration: {}", e);
        e
    })?);

    info!("Configuration loaded successfully");

    // Log initial configuration
    let config = config_repo.get();
    for (cluster_name, cluster) in &config.clusters {
        info!(
            "Cluster '{}': {} hosts, {} ports",
            cluster_name,
            cluster.hosts.len(),
            cluster.ports.len()
        );
    }

    // Setup SIGHUP handler for configuration reload
    let config_repo_clone = Arc::clone(&config_repo);
    tokio::spawn(async move {
        let mut sighup = signal(SignalKind::hangup()).expect("Failed to setup SIGHUP handler");

        loop {
            sighup.recv().await;
            info!("Received SIGHUP, reloading configuration...");

            match config_repo_clone.reload() {
                Ok(diff) => {
                    if diff.has_changes() {
                        log_config_changes(&diff);
                        info!("Configuration reloaded successfully");
                    } else {
                        info!("Configuration unchanged");
                    }
                }
                Err(e) => {
                    error!("Failed to reload configuration: {}", e);
                    warn!("Keeping previous configuration");
                }
            }
        }
    });

    // TODO: Start TCP listeners for each port configuration
    // TODO: Implement Patroni API client
    // TODO: Implement health checking and failover logic
    // TODO: Implement TCP proxy with connection routing

    info!("patroni-proxy is running. Send SIGHUP to reload configuration.");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutting down patroni-proxy...");

    Ok(())
}

fn log_config_changes(diff: &ConfigDiff) {
    for change in &diff.changes {
        match change {
            ClusterDiff::Added(name, _) => {
                info!("Cluster '{}' added", name);
            }
            ClusterDiff::Removed(name) => {
                info!("Cluster '{}' removed", name);
            }
            ClusterDiff::HostsChanged(name, old, new) => {
                info!("Cluster '{}' hosts changed: {:?} -> {:?}", name, old, new);
            }
            ClusterDiff::PortsChanged(name, _, _) => {
                info!("Cluster '{}' ports configuration changed", name);
            }
            ClusterDiff::TlsChanged(name) => {
                info!("Cluster '{}' TLS configuration changed", name);
            }
        }
    }
}
