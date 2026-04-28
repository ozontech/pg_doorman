use std::collections::HashMap;
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

/// Fallback host chosen via Patroni `/cluster`.
pub struct FallbackTarget {
    pub host: String,
    pub port: u16,
    pub role: Role,
    pub lifetime_ms: u64,
}

/// Three-way blacklist state. `JustExpired` exists so callers can drain stale
/// fallback connections on natural expiry.
#[derive(Debug, PartialEq)]
pub enum BlacklistCheck {
    NotBlacklisted,
    Active,
    /// Blacklist just expired on this check. Caller should bump epoch to drain
    /// stale fallback connections.
    JustExpired,
}

pub struct FallbackState {
    /// When `Some` and in the future, local host is considered down.
    blacklisted_until: Mutex<Option<Instant>>,
    /// Last successful fallback. Reused while blacklist is active.
    whitelisted_host: Mutex<Option<(String, u16, Role)>>,
    /// Shared inflight `/cluster` request for coalescing.
    /// Stores creation timestamp alongside the future to detect staleness.
    inflight: tokio::sync::Mutex<Option<(Instant, SharedClusterFuture)>>,

    /// Suppresses repeat blacklist log lines.
    blacklist_logged: AtomicBool,

    /// Per-candidate cooldown set after a failed startup. Each entry maps
    /// `(host, port) -> cooldown_until`. While the entry is in the future,
    /// `select_candidates_filtered` skips this candidate, so a recovering
    /// postgres on a target node does not get hammered on every client request.
    unhealthy_candidates: Mutex<HashMap<(String, u16), Instant>>,

    pool_name: String,
    discovery_urls: Vec<String>,
    blacklist_duration: Duration,
    connect_timeout: Duration,
    server_lifetime_ms: u64,
    patroni_client: PatroniClient,
}

/// Coalesced `/cluster` requests older than this are replaced.
/// 1 second balances coalescing with not stalling on a hung request.
const INFLIGHT_STALENESS: Duration = Duration::from_secs(1);

impl FallbackState {
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
            unhealthy_candidates: Mutex::new(HashMap::new()),
            pool_name,
            discovery_urls,
            blacklist_duration,
            connect_timeout,
            server_lifetime_ms,
            patroni_client,
        })
    }

    /// Check blacklist state. On natural expiry, also clears whitelist,
    /// the prometheus gauge, and the log-suppression flag.
    pub fn check_blacklist(&self) -> BlacklistCheck {
        let mut guard = self.blacklisted_until.lock();
        match *guard {
            Some(until) if Instant::now() < until => BlacklistCheck::Active,
            Some(_) => {
                *guard = None;
                drop(guard);

                crate::prometheus::FALLBACK_ACTIVE
                    .with_label_values(&[&self.pool_name])
                    .set(0.0);
                self.blacklist_logged.store(false, Ordering::Relaxed);

                // Read whitelist before clearing so we can remove the exact metric labels
                let old_host = {
                    let mut wl = self.whitelisted_host.lock();
                    wl.take()
                };
                if let Some((host, port, _)) = old_host {
                    let _ = crate::prometheus::FALLBACK_HOST.remove_label_values(&[
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

    /// True only on the first call after a fresh blacklist; rate-limits log lines.
    pub fn should_log_blacklist(&self) -> bool {
        !self.blacklist_logged.swap(true, Ordering::Relaxed)
    }

    /// Reset blacklist, whitelist, candidate cooldowns, and metrics.
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
            let _ = crate::prometheus::FALLBACK_HOST.remove_label_values(&[
                &self.pool_name,
                &host,
                &port.to_string(),
            ]);
        }
        self.unhealthy_candidates.lock().clear();
        self.blacklist_logged.store(false, Ordering::Relaxed);
        crate::prometheus::FALLBACK_ACTIVE
            .with_label_values(&[&self.pool_name])
            .set(0.0);
    }

    /// True iff a whitelisted host is currently cached. Lets the fallback
    /// caller distinguish "whitelist round failed" (which warrants a single
    /// retry with fresh discovery) from "discovery round failed" (where a
    /// retry would just repeat the same query).
    pub fn is_whitelisted(&self) -> bool {
        self.whitelisted_host.lock().is_some()
    }

    /// `fallback_connect_timeout` — used both as the per-startup deadline on
    /// fallback connections and as the per-candidate cooldown window after a
    /// failed startup. Kept as one parameter for now: same scale, same
    /// "candidate looks unresponsive" semantics.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Clear whitelist cache so the next `get_fallback_targets` re-runs discovery.
    pub fn clear_whitelist(&self) {
        let old = {
            let mut guard = self.whitelisted_host.lock();
            guard.take()
        };
        if let Some((host, port, _)) = old {
            let _ = crate::prometheus::FALLBACK_HOST.remove_label_values(&[
                &self.pool_name,
                &host,
                &port.to_string(),
            ]);
        }
    }

    /// Mark a candidate unhealthy for `connect_timeout` window. The next
    /// `get_fallback_targets` call will skip it. Used after a failed startup
    /// (auth error, `database is starting up`, timeout, etc.) so that a
    /// recovering candidate is not retried on every client request.
    pub fn mark_unhealthy(&self, host: &str, port: u16) {
        let until = Instant::now() + self.connect_timeout;
        let mut guard = self.unhealthy_candidates.lock();
        guard.insert((host.to_string(), port), until);
    }

    /// True if `(host, port)` is currently within its cooldown window.
    /// Performs lazy cleanup of an expired entry on miss.
    pub fn is_unhealthy(&self, host: &str, port: u16) -> bool {
        let mut guard = self.unhealthy_candidates.lock();
        let key = (host.to_string(), port);
        match guard.get(&key) {
            Some(until) if Instant::now() < *until => true,
            Some(_) => {
                guard.remove(&key);
                false
            }
            None => false,
        }
    }

    /// Build the candidate list the caller must iterate when establishing a
    /// fallback connection.
    ///
    /// On whitelist hit returns a single-element vector with the cached host
    /// (skipping discovery) — unless that host is currently in cooldown, in
    /// which case the whitelist is bypassed and full discovery runs.
    ///
    /// Otherwise: fetch `/cluster`, drop unhealthy candidates, run parallel
    /// TCP-probe, and return all alive candidates ordered by priority
    /// (sync_standby > replica > leader). Caller iterates the list and is
    /// responsible for `set_whitelisted` on the first successful startup and
    /// `mark_unhealthy` on each failure.
    pub async fn get_fallback_targets(&self) -> Result<Vec<FallbackTarget>, String> {
        // 1. Check whitelist — return cached host immediately, unless it just
        // got marked unhealthy (in which case we skip the cache and rediscover).
        {
            let guard = self.whitelisted_host.lock();
            if let Some((ref host, port, ref role)) = *guard {
                let host_owned = host.clone();
                let role_owned = role.clone();
                drop(guard);
                if !self.is_unhealthy(&host_owned, port) {
                    debug!(
                        "[pool: {}] fallback: returning whitelisted host {}:{}",
                        self.pool_name, host_owned, port
                    );
                    crate::prometheus::FALLBACK_CACHE_HITS_TOTAL
                        .with_label_values(&[&self.pool_name])
                        .inc();
                    return Ok(vec![FallbackTarget {
                        host: host_owned,
                        port,
                        role: role_owned,
                        lifetime_ms: self.server_lifetime_ms,
                    }]);
                }
            }
        }

        // 2. Fetch /cluster via coalesced request.
        let cluster = self.fetch_cluster_coalesced().await?;

        // 3. Filter and sort candidates, then drop those in cooldown.
        let raw_candidates = select_candidates(&cluster.members);
        let candidates: Vec<(String, u16, Role)> = raw_candidates
            .into_iter()
            .filter(|(host, port, _)| !self.is_unhealthy(host, *port))
            .collect();
        info!(
            "[pool: {}] fallback: discovered {} members, {} candidates: {}",
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

        // 4. Parallel TCP-probe; collect every alive candidate sorted by priority.
        let alive = self
            .try_connect_candidates(&candidates, self.connect_timeout)
            .await;
        if alive.is_empty() {
            warn!(
                "[pool: {}] fallback: all {} candidates unreachable",
                self.pool_name,
                candidates.len()
            );
            return Err("all candidates unreachable".to_string());
        }

        Ok(alive
            .into_iter()
            .map(|(host, port, role)| FallbackTarget {
                host,
                port,
                role,
                lifetime_ms: self.server_lifetime_ms,
            })
            .collect())
    }

    /// Record a successful fallback host so subsequent calls hit
    /// `get_fallback_targets`'s whitelist branch and skip discovery.
    /// Called from `server_pool` after `Server::startup` returns Ok.
    pub fn set_whitelisted(&self, host: String, port: u16, role: Role) {
        {
            let mut guard = self.whitelisted_host.lock();
            *guard = Some((host.clone(), port, role.clone()));
        }
        info!(
            "[pool: {}] fallback: whitelisted {}:{} (role: {:?})",
            self.pool_name, host, port, role
        );
        crate::prometheus::FALLBACK_HOST
            .with_label_values(&[&self.pool_name, &host, &port.to_string()])
            .set(1.0);
    }

    async fn fetch_cluster_coalesced(&self) -> Result<ClusterResponse, String> {
        let (shared, is_creator) = {
            let mut guard = self.inflight.lock().await;

            if let Some((created_at, ref shared)) = *guard {
                if created_at.elapsed() < INFLIGHT_STALENESS {
                    (shared.clone(), false)
                } else {
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

        if is_creator {
            crate::prometheus::PATRONI_API_REQUESTS_TOTAL
                .with_label_values(&[&self.pool_name])
                .inc();
        }

        let start = Instant::now();
        let result = shared.await;

        // Joiners measure wait time, not discovery time — only the creator records it.
        if is_creator {
            crate::prometheus::PATRONI_API_DURATION
                .with_label_values(&[&self.pool_name])
                .observe(start.elapsed().as_secs_f64());
        }

        // On creator-side error: drop the cached future so the next caller starts
        // fresh instead of inheriting this failure for INFLIGHT_STALENESS.
        //
        // Joiners must NOT clear: every failed joiner would race for the slot
        // and could erase a fresh inflight installed by another task. The
        // creator-only guard bounds this to one race per failure — if the
        // creator's await outran INFLIGHT_STALENESS, this clear may wipe a
        // fresh inflight. Worst case: one redundant /cluster fetch.
        if is_creator && result.is_err() {
            let mut guard = self.inflight.lock().await;
            *guard = None;
        }

        result
    }

    fn create_inflight(&self) -> SharedClusterFuture {
        let urls = self.discovery_urls.clone();
        let client = self.patroni_client.clone();
        let fut = async move { client.fetch_cluster(&urls).await.map_err(|e| e.to_string()) };
        fut.boxed().shared()
    }

    /// Parallel TCP-probe across all candidates. Returns every alive one
    /// sorted by priority (sync_standby > replica > leader); empty vec means
    /// nothing answered.
    ///
    /// We wait for all probes to complete instead of returning on first
    /// sync_standby: `create_fallback_connection` iterates this list when a
    /// startup fails on the best candidate, so it needs the lower-priority
    /// alternatives ready.
    async fn try_connect_candidates(
        &self,
        candidates: &[(String, u16, Role)],
        timeout: Duration,
    ) -> Vec<(String, u16, Role)> {
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
                            "[pool: {}] fallback: TCP connect ok to {} (role: {:?})",
                            pn, addr, role
                        );
                        Some((host, port, role))
                    }
                    Ok(Err(e)) => {
                        warn!(
                            "[pool: {}] fallback: TCP connect failed to {}: {}",
                            pn, addr, e
                        );
                        None
                    }
                    Err(_) => {
                        warn!("[pool: {}] fallback: TCP connect timeout to {}", pn, addr);
                        None
                    }
                }
            });
            futs.push(fut);
        }

        let results = futures::future::join_all(futs).await;
        let mut alive: Vec<(String, u16, Role)> = results.into_iter().flatten().collect();
        alive.sort_by_key(|(_, _, role)| role_priority(role));
        alive
    }
}

/// Filter and sort members from a /cluster response.
///
/// Excluded: non-running, `noloadbalance`, `nofailover`, `archive`.
/// Kept: `nostream` — cascade replicas serve reads, just with higher lag.
///
/// Sort: sync_standby > replica > everything else (including leader).
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
        let state = FallbackState::new(
            "test_pool".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);

        state.blacklist();
        assert_eq!(state.check_blacklist(), BlacklistCheck::Active);

        state.clear();
        assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);
    }

    #[test]
    fn mark_unhealthy_lifecycle() {
        let state = FallbackState::new(
            "test_pool_unhealthy_lifecycle".to_string(),
            vec![],
            Duration::from_secs(10),
            // Short cooldown so the test does not stall.
            Duration::from_millis(50),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        assert!(!state.is_unhealthy("10.0.0.1", 5432));
        state.mark_unhealthy("10.0.0.1", 5432);
        assert!(state.is_unhealthy("10.0.0.1", 5432));

        // Wait past the cooldown window; the next call must report healthy
        // again and lazy-clean the entry.
        std::thread::sleep(Duration::from_millis(70));
        assert!(!state.is_unhealthy("10.0.0.1", 5432));

        // Marking the same host again restarts the window.
        state.mark_unhealthy("10.0.0.1", 5432);
        assert!(state.is_unhealthy("10.0.0.1", 5432));
    }

    #[test]
    fn mark_unhealthy_is_per_host_port() {
        let state = FallbackState::new(
            "test_pool_unhealthy_per_host".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(60),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.mark_unhealthy("10.0.0.1", 5432);
        assert!(state.is_unhealthy("10.0.0.1", 5432));
        // Different host — still healthy.
        assert!(!state.is_unhealthy("10.0.0.2", 5432));
        // Same host, different port — also independent.
        assert!(!state.is_unhealthy("10.0.0.1", 5433));
    }

    #[test]
    fn clear_drops_unhealthy_entries() {
        let state = FallbackState::new(
            "test_pool_unhealthy_clear".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(60),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.mark_unhealthy("10.0.0.1", 5432);
        state.mark_unhealthy("10.0.0.2", 5432);
        state.clear();
        assert!(!state.is_unhealthy("10.0.0.1", 5432));
        assert!(!state.is_unhealthy("10.0.0.2", 5432));
    }

    #[test]
    fn is_whitelisted_reflects_state() {
        let state = FallbackState::new(
            "test_pool_is_whitelisted".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        assert!(!state.is_whitelisted());
        state.set_whitelisted("10.0.0.1".to_string(), 5432, Role::SyncStandby);
        assert!(state.is_whitelisted());
        state.clear_whitelist();
        assert!(!state.is_whitelisted());
    }

    #[test]
    fn blacklist_expiry_returns_just_expired() {
        let state = FallbackState::new(
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

        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(state.check_blacklist(), BlacklistCheck::JustExpired);
        assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);
    }

    #[test]
    fn blacklist_expiry_clears_whitelist() {
        let state = FallbackState::new(
            "test_pool".to_string(),
            vec![],
            Duration::from_millis(1),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        {
            let mut guard = state.whitelisted_host.lock();
            *guard = Some(("10.0.0.5".to_string(), 5432, Role::Replica));
        }

        state.blacklist();
        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(state.check_blacklist(), BlacklistCheck::JustExpired);

        let guard = state.whitelisted_host.lock();
        assert!(guard.is_none(), "whitelist must be cleared on expiry");
    }

    #[test]
    fn should_log_blacklist_only_first_time() {
        let state = FallbackState::new(
            "test_pool".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.blacklist();

        assert!(state.should_log_blacklist());
        assert!(!state.should_log_blacklist());
        assert!(!state.should_log_blacklist());

        state.clear();
        state.blacklist();
        assert!(state.should_log_blacklist());
    }

    #[test]
    fn clear_whitelist_removes_cached_host() {
        let state = FallbackState::new(
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

        assert_eq!(candidates[0].0, "10.0.0.3");
        assert_eq!(candidates[0].2, Role::SyncStandby);

        assert_eq!(candidates[1].2, Role::Replica);
        assert_eq!(candidates[2].2, Role::Replica);

        assert_eq!(candidates[3].0, "10.0.0.1");
        assert_eq!(candidates[3].2, Role::Leader);
    }

    #[test]
    fn select_candidates_empty() {
        let candidates = select_candidates(&[]);
        assert!(candidates.is_empty());

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

    #[test]
    fn select_candidates_keeps_nostream() {
        let mut cascade = make_member("pg-cascade", Role::Replica, "streaming", "10.0.0.1", 5432);
        cascade.tags.nostream = true;
        let normal = make_member("pg-normal", Role::Replica, "streaming", "10.0.0.2", 5432);
        let candidates = select_candidates(&[cascade, normal]);
        assert_eq!(candidates.len(), 2);
    }

    #[tokio::test]
    async fn failed_inflight_does_not_poison_next_caller() {
        let state = FallbackState::new(
            "test_pool_inflight_fail".to_string(),
            vec!["http://127.0.0.1:1/cluster".to_string()],
            Duration::from_secs(10),
            Duration::from_millis(100),
            Duration::from_millis(200),
            30_000,
        )
        .unwrap();

        let before = crate::prometheus::PATRONI_API_REQUESTS_TOTAL
            .with_label_values(&["test_pool_inflight_fail"])
            .get();
        let _ = state.fetch_cluster_coalesced().await;
        let _ = state.fetch_cluster_coalesced().await;
        let after = crate::prometheus::PATRONI_API_REQUESTS_TOTAL
            .with_label_values(&["test_pool_inflight_fail"])
            .get();

        assert_eq!(after - before, 2);
    }

    /// Mock HTTP/1.1 server replying with `response_body` to every request.
    /// Lives until the tokio runtime shuts down.
    async fn start_mock_patroni_success(response_body: String) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let body = response_body.clone();
                tokio::spawn(async move {
                    // Buffer for a typical HTTP/1.1 request; not parsed.
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        port
    }

    /// Bind a TCP listener that just accepts and drops connections, simulating
    /// a postgres TCP listener that's alive at L4. Lives until the runtime
    /// shuts down.
    async fn start_alive_listener() -> u16 {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while listener.accept().await.is_ok() {
                // accept and drop — only the L4 handshake matters for probes
            }
        });
        port
    }

    #[tokio::test]
    async fn try_connect_candidates_returns_full_sorted_list() {
        // Three alive TCP listeners — order in `candidates` is intentionally
        // not by priority, so the result must be sorted by `role_priority`.
        let port_a = start_alive_listener().await;
        let port_b = start_alive_listener().await;
        let port_c = start_alive_listener().await;

        let state = FallbackState::new(
            "test_pool_full_sorted".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_millis(500),
            Duration::from_millis(500),
            30_000,
        )
        .unwrap();

        let candidates = vec![
            ("127.0.0.1".to_string(), port_a, Role::Leader),
            ("127.0.0.1".to_string(), port_b, Role::Replica),
            ("127.0.0.1".to_string(), port_c, Role::SyncStandby),
        ];

        let alive = state
            .try_connect_candidates(&candidates, Duration::from_millis(500))
            .await;
        assert_eq!(alive.len(), 3);
        assert_eq!(alive[0].2, Role::SyncStandby);
        assert_eq!(alive[1].2, Role::Replica);
        assert_eq!(alive[2].2, Role::Leader);
    }

    #[tokio::test]
    async fn get_fallback_targets_skips_unhealthy_in_discovery() {
        let port_alive = start_alive_listener().await;
        let port_marked = start_alive_listener().await;

        let body = format!(
            r#"{{"members":[
                {{"name":"a","host":"127.0.0.1","port":{port_marked},"role":"replica","state":"streaming"}},
                {{"name":"b","host":"127.0.0.1","port":{port_alive},"role":"replica","state":"streaming"}}
            ]}}"#
        );
        let api_port = start_mock_patroni_success(body).await;

        let state = FallbackState::new(
            "test_pool_skip_unhealthy".to_string(),
            vec![format!("http://127.0.0.1:{api_port}/cluster")],
            Duration::from_secs(10),
            Duration::from_millis(500),
            Duration::from_millis(500),
            30_000,
        )
        .unwrap();

        // Mark one member's host:port as unhealthy. Discovery must drop it.
        state.mark_unhealthy("127.0.0.1", port_marked);

        let targets = state.get_fallback_targets().await.unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].port, port_alive);
    }

    #[tokio::test]
    async fn successful_inflight_is_coalesced_in_subsequent_call() {
        let body = r#"{"members":[{"name":"n","host":"127.0.0.1","port":5432,"role":"replica","state":"streaming"}]}"#.to_string();
        let port = start_mock_patroni_success(body).await;

        let state = FallbackState::new(
            "test_pool_inflight_ok_coalesce".to_string(),
            vec![format!("http://127.0.0.1:{}/cluster", port)],
            Duration::from_secs(10),
            Duration::from_millis(500),
            Duration::from_millis(500),
            30_000,
        )
        .unwrap();

        let before = crate::prometheus::PATRONI_API_REQUESTS_TOTAL
            .with_label_values(&["test_pool_inflight_ok_coalesce"])
            .get();
        let r1 = state.fetch_cluster_coalesced().await;
        let r2 = state.fetch_cluster_coalesced().await;
        let after = crate::prometheus::PATRONI_API_REQUESTS_TOTAL
            .with_label_values(&["test_pool_inflight_ok_coalesce"])
            .get();

        assert!(
            r1.is_ok(),
            "first call must succeed against the mock server"
        );
        assert!(r2.is_ok(), "second call must succeed via coalesced cache");
        assert_eq!(
            after - before,
            1,
            "second call must coalesce on cached success, not start fresh discovery"
        );
    }
}
