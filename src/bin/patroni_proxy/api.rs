use crate::cluster_manager::ClusterManager;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Start minimal HTTP server for health checks and metrics
pub async fn start_http_server(
    listen_addr: String,
    cluster_managers: Arc<RwLock<HashMap<String, Arc<ClusterManager>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = listen_addr.parse()?;
    let listener = TcpListener::bind(addr).await?;
    
    info!("HTTP server listening on {}", addr);
    
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, client_addr)) => {
                    let managers = Arc::clone(&cluster_managers);
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 4096];
                        
                        match stream.read(&mut buffer).await {
                            Ok(n) if n > 0 => {
                                let request = String::from_utf8_lossy(&buffer[..n]);
                                let first_line = request.lines().next().unwrap_or("");
                                debug!("HTTP request from {}: {}", client_addr, first_line);
                                
                                // Parse request path
                                let path = first_line
                                    .split_whitespace()
                                    .nth(1)
                                    .unwrap_or("/");
                                
                                let response = match path {
                                    "/update_clusters" => {
                                        handle_update_clusters(managers).await
                                    }
                                    _ => {
                                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nOK".to_string()
                                    }
                                };
                                
                                if let Err(e) = stream.write_all(response.as_bytes()).await {
                                    debug!("Failed to send response to {}: {}", client_addr, e);
                                }
                            }
                            Ok(_) => {
                                debug!("Empty request from {}", client_addr);
                            }
                            Err(e) => {
                                debug!("Failed to read request from {}: {}", client_addr, e);
                            }
                        }
                        
                        let _ = stream.shutdown().await;
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    });
    
    Ok(())
}

/// Handle /update_clusters endpoint - trigger immediate update of all clusters
async fn handle_update_clusters(
    cluster_managers: Arc<RwLock<HashMap<String, Arc<ClusterManager>>>>,
) -> String {
    info!("Received request to update all clusters");
    
    let managers = cluster_managers.read().await;
    let mut updated_count = 0;
    
    for (cluster_name, manager) in managers.iter() {
        info!("Updating cluster '{}'", cluster_name);
        manager.update_members().await;
        updated_count += 1;
    }
    
    let message = format!("Updated {} cluster(s)", updated_count);
    info!("{}", message);
    
    let response_body = format!("{}\n", message);
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
        response_body.len(),
        response_body
    )
}
