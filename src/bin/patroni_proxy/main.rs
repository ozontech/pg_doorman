mod api;
mod cluster_manager;
mod config;
mod patroni;
mod port;
mod stream;

use api::start_http_server;
use clap::Parser;
use cluster_manager::{handle_config_changes, ClusterManager};
use config::{ClusterDiff, ConfigDiff, ConfigRepository};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Patroni Proxy: PostgreSQL proxy for Patroni clusters
#[derive(Parser, Debug)]
#[command(name = "patroni_proxy", author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(default_value = "patroni_proxy.yaml")]
    config_file: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments (handles --version and --help automatically)
    let args = Args::parse();
    let config_path = args.config_file;

    info!("Starting patroni-proxy with config: {}", config_path);

    // Load configuration
    let config_repo = Arc::new(ConfigRepository::new(&config_path).map_err(|e| {
        error!("Failed to load configuration: {}", e);
        e
    })?);

    info!("Configuration loaded successfully");

    // Cluster managers
    let cluster_managers: Arc<RwLock<HashMap<String, Arc<ClusterManager>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Initialize clusters and start ports
    {
        let config = config_repo.get();
        let update_interval = Duration::from_secs(config.cluster_update_interval);
        let mut managers = cluster_managers.write().await;

        for (cluster_name, cluster_config) in &config.clusters {
            info!(
                "Initializing cluster '{}': {} hosts, {} ports",
                cluster_name,
                cluster_config.hosts.len(),
                cluster_config.ports.len()
            );

            let manager = Arc::new(ClusterManager::new(
                cluster_name.clone(),
                cluster_config.hosts.clone(),
                update_interval,
            )?);

            // Start ports
            manager.start_ports(&cluster_config.ports).await?;

            // Start update loop
            manager.clone().start_update_loop();

            managers.insert(cluster_name.clone(), manager);
        }
    }

    // Start HTTP server
    {
        let config = config_repo.get();
        start_http_server(config.listen_address.clone(), Arc::clone(&cluster_managers)).await?;
    }

    // Setup SIGHUP handler for configuration reload
    let config_repo_clone = Arc::clone(&config_repo);
    let cluster_managers_clone = Arc::clone(&cluster_managers);
    tokio::spawn(async move {
        let mut sighup = signal(SignalKind::hangup()).expect("Failed to setup SIGHUP handler");

        loop {
            sighup.recv().await;
            info!("Received SIGHUP, reloading configuration...");

            match config_repo_clone.reload() {
                Ok(diff) => {
                    if diff.has_changes() {
                        log_config_changes(&diff);
                        let config = config_repo_clone.get();
                        let update_interval = Duration::from_secs(config.cluster_update_interval);
                        handle_config_changes(&diff, &cluster_managers_clone, update_interval)
                            .await;
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

    info!("patroni-proxy is running. Send SIGHUP to reload configuration, Ctrl+C to stop.");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutting down patroni-proxy...");

    // Stop all clusters
    {
        let managers = cluster_managers.read().await;
        for (cluster_name, manager) in managers.iter() {
            info!("Stopping cluster '{}'", cluster_name);
            manager.stop_ports().await;
        }
    }

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
