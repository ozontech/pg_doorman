use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::future::Shared;
use futures::FutureExt;
use log::{debug, info, warn};
use parking_lot::Mutex;
use tokio::net::TcpStream;

use crate::patroni::client::PatroniClient;
use crate::patroni::types::{ClusterResponse, Member, Role};

type SharedClusterFuture =
    Shared<Pin<Box<dyn Future<Output = Result<ClusterResponse, String>> + Send>>>;

/// A discovered fallback host to connect to when the primary is unavailable.
pub struct FallbackTarget {
    pub host: String,
    pub port: u16,
    pub role: Role,
    pub lifetime_ms: u64,
}

/// Result of checking blacklist state, distinguishing natural expiry
/// from active blacklist and no-blacklist states.
#[derive(Debug, PartialEq)]
pub enum BlacklistCheck {
    /// Host is not blacklisted and was not recently.
    NotBlacklisted,
    /// Blacklist is currently active (not yet expired).
    Active,
    /// Blacklist just expired on this check. Caller should
    /// bump epoch to drain stale fallback connections.
    JustExpired,
}

pub struct FailoverState {
    /// When Some and in the future, local host is considered down
    blacklisted_until: Mutex<Option<Instant>>,
    /// Cached fallback host and its role from last successful discovery
    whitelisted_host: Mutex<Option<(String, u16, Role)>>,
    /// Shared inflight /cluster request for coalescing.
    /// Stores creation timestamp alongside the future to detect staleness.
    inflight: tokio::sync::Mutex<Option<(Instant, SharedClusterFuture)>>,

    /// Suppresses repeated blacklist log messages after the first one.
    blacklist_logged: AtomicBool,

    pool_name: String,
    discovery_urls: Vec<String>,
    blacklist_duration: Duration,
    connect_timeout: Duration,
    server_lifetime_ms: u64,
    /// Reusable HTTP client for Patroni API calls.
    patroni_client: PatroniClient,
}

/// Age threshold for inflight coalesced requests. Futures older than
/// this are treated as stale and replaced with a fresh request.
const INFLIGHT_STALENESS: Duration = Duration::from_secs(1);

impl FailoverState {
    pub fn new(
        pool_name: String,
        discovery_urls: Vec<String>,
        blacklist_duration: Duration,
        connect_timeout: Duration,
        request_timeout: Duration,
        server_lifetime_ms: u64,
    ) -> Result<Self, String> {
        let patroni_client = PatroniClient::new(request_timeout, connect_timeout)
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self {
            blacklisted_until: Mutex::new(None),
            whitelisted_host: Mutex::new(None),
            inflight: tokio::sync::Mutex::new(None),
            blacklist_logged: AtomicBool::new(false),
            pool_name,
            discovery_urls,
            blacklist_duration,
            connect_timeout,
            server_lifetime_ms,
            patroni_client,
        })
    }

    /// Check the blacklist state, handling natural expiry.
    ///
    /// On `JustExpired`: clears the internal blacklist timer, resets the
    /// prometheus gauge, clears the whitelist cache, and resets the
    /// log-suppression flag so the next blacklist event is logged.
    pub fn check_blacklist(&self) -> BlacklistCheck {
        let mut guard = self.blacklisted_until.lock();
        match *guard {
            Some(until) if Instant::now() < until => BlacklistCheck::Active,
            Some(_) => {
                // Blacklist just expired — clean up
                *guard = None;
                drop(guard);

                crate::prometheus::FAILOVER_HOST_BLACKLISTED
                    .with_label_values(&[&self.pool_name])
                    .set(0.0);
                self.blacklist_logged.store(false, Ordering::Relaxed);

                // Read whitelist before clearing so we can remove the exact metric labels
                let old_host = {
                    let mut wl = self.whitelisted_host.lock();
                    wl.take()
                };
                if let Some((host, port, _)) = old_host {
                    let _ = crate::prometheus::FAILOVER_FALLBACK_HOST.remove_label_values(&[
                        &self.pool_name,
                        &host,
                        &port.to_string(),
                    ]);
                }

                BlacklistCheck::JustExpired
            }
            None => BlacklistCheck::NotBlacklisted,
        }
    }

    pub fn blacklist(&self) {
        let mut guard = self.blacklisted_until.lock();
        *guard = Some(Instant::now() + self.blacklist_duration);
    }

    /// Returns true only on the first call after a blacklist is set.
    /// Subsequent calls return false until the blacklist expires and
    /// is re-set, preventing log flooding.
    pub fn should_log_blacklist(&self) -> bool {
        !self.blacklist_logged.swap(true, Ordering::Relaxed)
    }

    /// Reset blacklist, whitelist, and metrics. Available for programmatic reset.
    pub fn clear(&self) {
        {
            let mut guard = self.blacklisted_until.lock();
            *guard = None;
        }
        let old_host = {
            let mut guard = self.whitelisted_host.lock();
            guard.take()
        };
        if let Some((host, port, _)) = old_host {
            let _ = crate::prometheus::FAILOVER_FALLBACK_HOST.remove_label_values(&[
                &self.pool_name,
                &host,
                &port.to_string(),
            ]);
        }
        self.blacklist_logged.store(false, Ordering::Relaxed);
        crate::prometheus::FAILOVER_HOST_BLACKLISTED
            .with_label_values(&[&self.pool_name])
            .set(0.0);
    }

    /// Clear whitelist cache so next `get_fallback_target` re-runs discovery.
    pub fn clear_whitelist(&self) {
        let old = {
            let mut guard = self.whitelisted_host.lock();
            guard.take()
        };
        if let Some((host, port, _)) = old {
            let _ = crate::prometheus::FAILOVER_FALLBACK_HOST.remove_label_values(&[
                &self.pool_name,
                &host,
                &port.to_string(),
            ]);
        }
    }

    pub async fn get_fallback_target(&self) -> Result<FallbackTarget, String> {
        // 1. Check whitelist — return cached host immediately
        {
            let guard = self.whitelisted_host.lock();
            if let Some((ref host, port, ref role)) = *guard {
                debug!(
                    "[pool: {}] failover: returning whitelisted host {}:{}",
                    self.pool_name, host, port
                );
                crate::prometheus::FAILOVER_WHITELIST_HITS_TOTAL
                    .with_label_values(&[&self.pool_name])
                    .inc();
                return Ok(FallbackTarget {
                    host: host.clone(),
                    port,
                    role: role.clone(),
                    lifetime_ms: self.server_lifetime_ms,
                });
            }
        }

        // 2. Fetch /cluster via coalesced request
        let cluster = self.fetch_cluster_coalesced().await?;

        // 3-4. Filter and sort candidates
        let candidates = select_candidates(&cluster.members);
        info!(
            "[pool: {}] failover: discovered {} members, {} candidates: {}",
            self.pool_name,
            cluster.members.len(),
            candidates.len(),
            candidates
                .iter()
                .map(|(h, p, r)| format!("{}:{}({:?})", h, p, r))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if candidates.is_empty() {
            return Err("no eligible members in /cluster response".to_string());
        }

        // 5-7. Parallel TCP connect to all candidates
        let timeout = self.connect_timeout;
        let (host, port, role) = self.try_connect_candidates(&candidates, timeout).await?;

        // 8. Whitelist the successful host
        {
            let mut guard = self.whitelisted_host.lock();
            *guard = Some((host.clone(), port, role.clone()));
        }
        info!(
            "[pool: {}] failover: whitelisted {}:{} (role: {:?})",
            self.pool_name, host, port, role
        );
        crate::prometheus::FAILOVER_FALLBACK_HOST
            .with_label_values(&[&self.pool_name, &host, &port.to_string()])
            .set(1.0);

        // 9. Return FallbackTarget
        Ok(FallbackTarget {
            host,
            port,
            role,
            lifetime_ms: self.server_lifetime_ms,
        })
    }

    async fn fetch_cluster_coalesced(&self) -> Result<ClusterResponse, String> {
        let (shared, is_creator) = {
            let mut guard = self.inflight.lock().await;

            // Reuse existing inflight if it was created recently
            if let Some((created_at, ref shared)) = *guard {
                if created_at.elapsed() < INFLIGHT_STALENESS {
                    (shared.clone(), false)
                } else {
                    // Stale — replace with a new request
                    let shared = self.create_inflight();
                    *guard = Some((Instant::now(), shared.clone()));
                    (shared, true)
                }
            } else {
                let shared = self.create_inflight();
                *guard = Some((Instant::now(), shared.clone()));
                (shared, true)
            }
        };

        // Only the creator increments the discovery counter
        if is_creator {
            crate::prometheus::FAILOVER_DISCOVERY_TOTAL
                .with_label_values(&[&self.pool_name])
                .inc();
        }

        let start = Instant::now();
        let result = shared.await;

        // Only the creator records duration — joiners measure wait time, not discovery time
        if is_creator {
            crate::prometheus::FAILOVER_DISCOVERY_DURATION
                .with_label_values(&[&self.pool_name])
                .observe(start.elapsed().as_secs_f64());
        }

        // No clearing of inflight here — staleness-based expiry handles it.
        // Clearing here would race: a new inflight created by another task
        // between our await and this point would be erroneously wiped.

        result
    }

    fn create_inflight(&self) -> SharedClusterFuture {
        let urls = self.discovery_urls.clone();
        let client = self.patroni_client.clone();
        let fut = async move { client.fetch_cluster(&urls).await.map_err(|e| e.to_string()) };
        fut.boxed().shared()
    }

    /// TCP-connect to candidates in parallel, returning the best that responds.
    /// Prefers sync_standby over replica over leader (by role_priority).
    /// Uses pinned futures instead of tokio::spawn to ensure cancellation
    /// when the remaining futures are dropped.
    async fn try_connect_candidates(
        &self,
        candidates: &[(String, u16, Role)],
        timeout: Duration,
    ) -> Result<(String, u16, Role), String> {
        type ConnectFuture = Pin<Box<dyn Future<Output = Option<(String, u16, Role)>> + Send>>;
        let mut futs: Vec<ConnectFuture> = Vec::with_capacity(candidates.len());

        let pool_name = self.pool_name.clone();
        for (host, port, role) in candidates {
            let addr = format!("{}:{}", host, port);
            let host = host.clone();
            let port = *port;
            let role = role.clone();
            let pn = pool_name.clone();
            let fut = Box::pin(async move {
                match tokio::time::timeout(timeout, TcpStream::connect(&addr)).await {
                    Ok(Ok(_stream)) => {
                        debug!(
                            "[pool: {}] failover: TCP connect ok to {} (role: {:?})",
                            pn, addr, role
                        );
                        Some((host, port, role))
                    }
                    Ok(Err(e)) => {
                        warn!(
                            "[pool: {}] failover: TCP connect failed to {}: {}",
                            pn, addr, e
                        );
                        None
                    }
                    Err(_) => {
                        warn!("[pool: {}] failover: TCP connect timeout to {}", pn, addr);
                        None
                    }
                }
            });
            futs.push(fut);
        }

        // Track best candidate seen so far, with its role priority.
        let mut best: Option<(String, u16, Role)> = None;
        let mut remaining = futs;

        while !remaining.is_empty() {
            let (result, _idx, rest) = futures::future::select_all(remaining).await;

            if let Some((host, port, role)) = result {
                let priority = role_priority(&role);
                if priority == 0 {
                    // Top priority (sync_standby) — return immediately,
                    // dropping `rest` cancels remaining futures.
                    return Ok((host, port, role));
                }
                let dominated = best
                    .as_ref()
                    .is_none_or(|(_, _, ref r)| priority < role_priority(r));
                if dominated {
                    best = Some((host, port, role));
                }
            }

            remaining = rest;
        }

        match best {
            Some((h, p, r)) => Ok((h, p, r)),
            None => {
                warn!(
                    "[pool: {}] failover: all {} candidates unreachable",
                    self.pool_name,
                    candidates.len()
                );
                Err("all candidates unreachable".to_string())
            }
        }
    }
}

/// Filter and sort members from a /cluster response.
///
/// Excluded: non-running members, noloadbalance, nofailover, archive replicas.
/// Sorted: sync_standby first, then replica, then others (including leader).
fn select_candidates(members: &[Member]) -> Vec<(String, u16, Role)> {
    let mut candidates: Vec<(String, u16, Role)> = members
        .iter()
        .filter(|m| {
            let alive = m.state == "streaming" || m.state == "running";
            alive && !m.tags.noloadbalance && !m.tags.nofailover && !m.tags.archive
        })
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
            tags: Default::default(),
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
        )
        .unwrap();

        // Initially not blacklisted
        assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);

        // After blacklist() — active
        state.blacklist();
        assert_eq!(state.check_blacklist(), BlacklistCheck::Active);

        // After clear() — not blacklisted
        state.clear();
        assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);
    }

    #[test]
    fn blacklist_expiry_returns_just_expired() {
        let state = FailoverState::new(
            "test_pool".to_string(),
            vec![],
            // Very short blacklist so it expires within the test
            Duration::from_millis(1),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.blacklist();

        // Wait for the blacklist to expire
        std::thread::sleep(Duration::from_millis(5));

        // First check after expiry returns JustExpired
        assert_eq!(state.check_blacklist(), BlacklistCheck::JustExpired);

        // Subsequent check returns NotBlacklisted (already cleaned up)
        assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);
    }

    #[test]
    fn blacklist_expiry_clears_whitelist() {
        let state = FailoverState::new(
            "test_pool".to_string(),
            vec![],
            Duration::from_millis(1),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        // Set a whitelist entry
        {
            let mut guard = state.whitelisted_host.lock();
            *guard = Some(("10.0.0.5".to_string(), 5432, Role::Replica));
        }

        state.blacklist();
        std::thread::sleep(Duration::from_millis(5));

        // JustExpired clears the whitelist
        assert_eq!(state.check_blacklist(), BlacklistCheck::JustExpired);

        let guard = state.whitelisted_host.lock();
        assert!(guard.is_none(), "whitelist must be cleared on expiry");
    }

    #[test]
    fn should_log_blacklist_only_first_time() {
        let state = FailoverState::new(
            "test_pool".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.blacklist();

        // First call returns true
        assert!(state.should_log_blacklist());
        // Subsequent calls return false
        assert!(!state.should_log_blacklist());
        assert!(!state.should_log_blacklist());

        // After clear, flag is reset
        state.clear();
        state.blacklist();
        assert!(state.should_log_blacklist());
    }

    #[test]
    fn clear_whitelist_removes_cached_host() {
        let state = FailoverState::new(
            "test_pool".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        {
            let mut guard = state.whitelisted_host.lock();
            *guard = Some(("10.0.0.1".to_string(), 5432, Role::SyncStandby));
        }

        state.clear_whitelist();

        let guard = state.whitelisted_host.lock();
        assert!(guard.is_none());
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

    #[test]
    fn select_candidates_filters_noloadbalance() {
        let mut nobalance = make_member("pg-nobal", Role::Replica, "streaming", "10.0.0.1", 5432);
        nobalance.tags.noloadbalance = true;

        let normal = make_member("pg-normal", Role::Replica, "streaming", "10.0.0.2", 5432);

        let candidates = select_candidates(&[nobalance, normal]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "10.0.0.2");
    }

    #[test]
    fn select_candidates_filters_nofailover() {
        let mut nofail = make_member("pg-nofail", Role::Replica, "streaming", "10.0.0.1", 5432);
        nofail.tags.nofailover = true;

        let normal = make_member("pg-normal", Role::Replica, "streaming", "10.0.0.2", 5432);

        let candidates = select_candidates(&[nofail, normal]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "10.0.0.2");
    }

    #[test]
    fn select_candidates_filters_archive() {
        let mut archive = make_member("pg-archive", Role::Replica, "streaming", "10.0.0.1", 5432);
        archive.tags.archive = true;

        let normal = make_member(
            "pg-normal",
            Role::SyncStandby,
            "streaming",
            "10.0.0.2",
            5432,
        );

        let candidates = select_candidates(&[archive, normal]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "10.0.0.2");
    }

    #[test]
    fn select_candidates_all_replicas_no_sync() {
        let members = vec![
            make_member("pg-replica1", Role::Replica, "streaming", "10.0.0.1", 5432),
            make_member("pg-replica2", Role::Replica, "streaming", "10.0.0.2", 5432),
            make_member("pg-leader", Role::Leader, "running", "10.0.0.3", 5432),
        ];

        let candidates = select_candidates(&members);
        assert_eq!(candidates.len(), 3);

        // Replicas come before leader even without sync_standby
        assert_eq!(candidates[0].2, Role::Replica);
        assert_eq!(candidates[1].2, Role::Replica);
        assert_eq!(candidates[2].2, Role::Leader);
    }

    #[test]
    fn select_candidates_only_leader() {
        let members = vec![make_member(
            "pg-leader",
            Role::Leader,
            "running",
            "10.0.0.1",
            5432,
        )];

        let candidates = select_candidates(&members);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "10.0.0.1");
        assert_eq!(candidates[0].2, Role::Leader);
    }
}
