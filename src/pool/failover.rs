use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::future::Shared;
use futures::FutureExt;
use log::{debug, info, warn};
use tokio::net::TcpStream;

use crate::patroni::client::PatroniClient;
use crate::patroni::types::{ClusterResponse, Member, Role};

type SharedClusterFuture =
    Shared<Pin<Box<dyn Future<Output = Result<ClusterResponse, String>> + Send>>>;

pub struct FallbackTarget {
    pub host: String,
    pub port: u16,
    pub lifetime_ms: u64,
}

pub struct FailoverState {
    /// When Some and in the future, local host is considered down
    blacklisted_until: Mutex<Option<Instant>>,
    /// Cached fallback host from last successful discovery
    whitelisted_host: Mutex<Option<(String, u16)>>,
    /// Shared inflight /cluster request for coalescing
    inflight: tokio::sync::Mutex<Option<SharedClusterFuture>>,

    pool_name: String,
    discovery_urls: Vec<String>,
    blacklist_duration: Duration,
    connect_timeout: Duration,
    request_timeout: Duration,
    server_lifetime_ms: u64,
}

impl FailoverState {
    pub fn new(
        pool_name: String,
        discovery_urls: Vec<String>,
        blacklist_duration: Duration,
        connect_timeout: Duration,
        request_timeout: Duration,
        server_lifetime_ms: u64,
    ) -> Self {
        Self {
            blacklisted_until: Mutex::new(None),
            whitelisted_host: Mutex::new(None),
            inflight: tokio::sync::Mutex::new(None),
            pool_name,
            discovery_urls,
            blacklist_duration,
            connect_timeout,
            request_timeout,
            server_lifetime_ms,
        }
    }

    pub fn is_blacklisted(&self) -> bool {
        let guard = self.blacklisted_until.lock().unwrap();
        match *guard {
            Some(until) => Instant::now() < until,
            None => false,
        }
    }

    pub fn blacklist(&self) {
        let mut guard = self.blacklisted_until.lock().unwrap();
        *guard = Some(Instant::now() + self.blacklist_duration);
    }

    /// Reset both blacklist and whitelist. Called on SIGHUP.
    pub fn clear(&self) {
        {
            let mut guard = self.blacklisted_until.lock().unwrap();
            *guard = None;
        }
        {
            let mut guard = self.whitelisted_host.lock().unwrap();
            *guard = None;
        }
        crate::prometheus::FAILOVER_HOST_BLACKLISTED
            .with_label_values(&[&self.pool_name])
            .set(0.0);
    }

    pub async fn get_fallback_target(&self) -> Result<FallbackTarget, String> {
        // 1. Check whitelist — return cached host immediately
        {
            let guard = self.whitelisted_host.lock().unwrap();
            if let Some((ref host, port)) = *guard {
                debug!("failover: returning whitelisted host {}:{}", host, port);
                return Ok(FallbackTarget {
                    host: host.clone(),
                    port,
                    lifetime_ms: self.server_lifetime_ms,
                });
            }
        }

        // 2. Fetch /cluster via coalesced request
        let cluster = self.fetch_cluster_coalesced().await?;

        // 3-4. Filter and sort candidates
        let candidates = select_candidates(&cluster.members);
        if candidates.is_empty() {
            return Err("no eligible members in /cluster response".to_string());
        }

        // 5-7. Parallel TCP connect to all candidates
        let timeout = self.connect_timeout;
        let (host, port) = self.try_connect_candidates(&candidates, timeout).await?;

        // 8. Whitelist the successful host
        {
            let mut guard = self.whitelisted_host.lock().unwrap();
            *guard = Some((host.clone(), port));
        }
        info!("failover: whitelisted {}:{}", host, port);

        // 9. Return FallbackTarget
        Ok(FallbackTarget {
            host,
            port,
            lifetime_ms: self.server_lifetime_ms,
        })
    }

    async fn fetch_cluster_coalesced(&self) -> Result<ClusterResponse, String> {
        let shared = {
            let mut guard = self.inflight.lock().await;
            if let Some(ref shared) = *guard {
                shared.clone()
            } else {
                let urls = self.discovery_urls.clone();
                let client = PatroniClient::new(self.request_timeout, self.connect_timeout);
                let fut =
                    async move { client.fetch_cluster(&urls).await.map_err(|e| e.to_string()) };
                let shared = fut.boxed().shared();
                *guard = Some(shared.clone());
                shared
            }
        };

        let start = std::time::Instant::now();
        crate::prometheus::FAILOVER_DISCOVERY_TOTAL
            .with_label_values(&[&self.pool_name])
            .inc();

        let result = shared.await;

        crate::prometheus::FAILOVER_DISCOVERY_DURATION
            .with_label_values(&[&self.pool_name])
            .observe(start.elapsed().as_secs_f64());

        // Clear inflight after completion
        {
            let mut guard = self.inflight.lock().await;
            *guard = None;
        }

        result
    }

    /// TCP-connect to candidates in parallel, returning the first that responds.
    /// Prefers sync_standby: if a sync_standby connects first, return immediately.
    async fn try_connect_candidates(
        &self,
        candidates: &[(String, u16, Role)],
        timeout: Duration,
    ) -> Result<(String, u16), String> {
        let mut handles = Vec::with_capacity(candidates.len());
        for (host, port, role) in candidates {
            let addr = format!("{}:{}", host, port);
            let host = host.clone();
            let port = *port;
            let role = role.clone();
            let handle = tokio::spawn(async move {
                match tokio::time::timeout(timeout, TcpStream::connect(&addr)).await {
                    Ok(Ok(_stream)) => {
                        debug!("failover: TCP connect ok to {} (role: {:?})", addr, role);
                        Some((host, port, role))
                    }
                    Ok(Err(e)) => {
                        warn!("failover: TCP connect failed to {}: {}", addr, e);
                        None
                    }
                    Err(_) => {
                        warn!("failover: TCP connect timeout to {}", addr);
                        None
                    }
                }
            });
            handles.push(handle);
        }

        // Collect results as they complete
        let mut best: Option<(String, u16)> = None;
        let mut remaining = handles;

        while !remaining.is_empty() {
            let (result, _idx, rest) = futures::future::select_all(remaining).await;

            match result {
                Ok(Some((host, port, role))) => {
                    if role == Role::SyncStandby {
                        // sync_standby is top priority — return immediately
                        return Ok((host, port));
                    }
                    if best.is_none() {
                        best = Some((host, port));
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("failover: connect task panicked: {}", e);
                }
            }

            remaining = rest;
        }

        best.ok_or_else(|| "all candidates unreachable".to_string())
    }
}

/// Filter and sort members from a /cluster response.
///
/// Only members with state "streaming" or "running" are eligible.
/// Sorted: sync_standby first, then replica, then others (including leader).
fn select_candidates(members: &[Member]) -> Vec<(String, u16, Role)> {
    let mut candidates: Vec<(String, u16, Role)> = members
        .iter()
        .filter(|m| m.state == "streaming" || m.state == "running")
        .map(|m| (m.host.clone(), m.port, m.role.clone()))
        .collect();

    candidates.sort_by_key(|(_, _, role)| role_priority(role));
    candidates
}

/// Lower value = higher priority.
fn role_priority(role: &Role) -> u8 {
    match role {
        Role::SyncStandby => 0,
        Role::Replica => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patroni::types::Member;
    use std::time::Duration;

    fn make_member(name: &str, role: Role, state: &str, host: &str, port: u16) -> Member {
        Member {
            name: name.to_string(),
            role,
            state: state.to_string(),
            host: host.to_string(),
            port,
            api_url: None,
            lag: None,
        }
    }

    #[test]
    fn blacklist_lifecycle() {
        let state = FailoverState::new(
            "test_pool".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        );

        // Initially not blacklisted
        assert!(!state.is_blacklisted());

        // After blacklist() — is blacklisted
        state.blacklist();
        assert!(state.is_blacklisted());

        // After clear() — not blacklisted, whitelist also cleared
        state.clear();
        assert!(!state.is_blacklisted());
    }

    #[test]
    fn select_candidates_ordering() {
        let members = vec![
            make_member("pg-leader", Role::Leader, "running", "10.0.0.1", 5432),
            make_member("pg-replica1", Role::Replica, "streaming", "10.0.0.2", 5432),
            make_member("pg-sync", Role::SyncStandby, "streaming", "10.0.0.3", 5432),
            make_member("pg-stopped", Role::Replica, "stopped", "10.0.0.4", 5432),
            make_member("pg-replica2", Role::Replica, "streaming", "10.0.0.5", 5432),
        ];

        let candidates = select_candidates(&members);

        // Stopped replica is excluded, leader included as last resort
        assert_eq!(candidates.len(), 4);

        // sync_standby comes first
        assert_eq!(candidates[0].0, "10.0.0.3");
        assert_eq!(candidates[0].2, Role::SyncStandby);

        // Replicas follow
        assert_eq!(candidates[1].2, Role::Replica);
        assert_eq!(candidates[2].2, Role::Replica);

        // Leader is last
        assert_eq!(candidates[3].0, "10.0.0.1");
        assert_eq!(candidates[3].2, Role::Leader);
    }

    #[test]
    fn select_candidates_empty() {
        let candidates = select_candidates(&[]);
        assert!(candidates.is_empty());

        // Only stopped replicas — no valid candidates
        let members = vec![
            make_member("pg-stopped1", Role::Replica, "stopped", "10.0.0.1", 5432),
            make_member("pg-stopped2", Role::Replica, "starting", "10.0.0.2", 5432),
        ];
        let candidates = select_candidates(&members);
        assert!(candidates.is_empty());
    }
}
