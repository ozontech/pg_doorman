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

use crate::errors::Error;
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

/// Where the candidate list returned by `get_fallback_targets` came from.
/// Lets the caller decide whether a failed round warrants one extra
/// discovery retry (whitelist hit may be stale) or not (discovery already
/// gave us the freshest list — exhaustion is final).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSource {
    /// Single cached host, no discovery performed.
    WhitelistCache,
    /// Full list freshly fetched from `/cluster`.
    Discovery,
}

/// Categorical reason for a fallback candidate failing a connection attempt.
/// Used both for the per-candidate Prometheus counter and for aggregating
/// the exhaustion error message so the operator sees `"3 startup_error,
/// 1 timeout"` rather than just the last error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureReason {
    /// Couldn't reach the candidate at the transport layer (TCP refuse,
    /// network unreachable, DNS).
    ConnectError,
    /// Server was reached but responded with FATAL during startup
    /// (auth, pg_hba, missing database).
    StartupError,
    /// Server responded `57P*` (admin shutdown, crash recovery, "database
    /// is starting up", database dropped).
    ServerUnavailable,
    /// `startup_with_timeout` deadline elapsed.
    Timeout,
    /// Anything else — should normally not happen on the fallback path.
    Other,
}

impl FailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureReason::ConnectError => "connect_error",
            FailureReason::StartupError => "startup_error",
            FailureReason::ServerUnavailable => "server_unavailable",
            FailureReason::Timeout => "timeout",
            FailureReason::Other => "other",
        }
    }
}

impl From<&Error> for FailureReason {
    fn from(err: &Error) -> Self {
        match err {
            // `startup_with_timeout` produces ConnectError with a literal
            // "startup timed out" prefix; promote it to its own bucket so
            // operators distinguish "couldn't connect" from "stuck postgres".
            Error::ConnectError(msg) if msg.starts_with("server startup timed out") => {
                FailureReason::Timeout
            }
            Error::ConnectError(_) => FailureReason::ConnectError,
            Error::ServerUnavailableError(_, _) => FailureReason::ServerUnavailable,
            Error::ServerStartupError(_, _) => FailureReason::StartupError,
            _ => FailureReason::Other,
        }
    }
}

/// Per-candidate cooldown entry. Tracks the active deadline, the
/// consecutive failure count for exponential backoff, and the last warn-log
/// timestamp so we can rate-limit per-candidate WARN spam under failure storm.
#[derive(Debug, Clone)]
struct CooldownEntry {
    until: Instant,
    attempts: u32,
    last_warn_at: Option<Instant>,
}

/// Cooldown grows as `base * 2^(attempts - 1)` capped at this value.
/// 60s is comfortably longer than typical Patroni `loop_wait` (10s) and
/// PostgreSQL restart cycles, but short enough that a candidate that
/// recovered isn't ignored for an absurd window.
const COOLDOWN_MAX: Duration = Duration::from_secs(60);

/// Hard cap on `unhealthy_candidates` size. When `mark_unhealthy` is about
/// to insert and we're at this cap, we prune expired entries first.
/// Realistic Patroni clusters have <10 members; this leaves headroom for
/// churn (autoscaling, k8s pod recreation) without unbounded growth from
/// stale entries that never get re-queried.
const COOLDOWN_MAX_ENTRIES: usize = 256;

/// Per-candidate WARN log rate-limit. Under a failure storm with N pools
/// and M clients each retrying, naive per-attempt logging spams the log
/// with thousands of identical lines per second; this throttles it to
/// roughly one line per candidate per 10s.
const COOLDOWN_LOG_RATE: Duration = Duration::from_secs(10);

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

    /// Per-candidate cooldown set after a failed startup. The entry tracks
    /// the active deadline, the consecutive-failure count for exponential
    /// backoff, and the last warn-log timestamp for log rate-limiting.
    /// Bounded by `COOLDOWN_MAX_ENTRIES` via lazy prune in `mark_unhealthy`.
    unhealthy_candidates: Mutex<HashMap<(String, u16), CooldownEntry>>,

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

    /// Mark a candidate unhealthy. The next `get_fallback_targets` call will
    /// skip this `(host, port)` until the cooldown window elapses.
    ///
    /// Cooldown grows exponentially on consecutive failures: base for the
    /// first miss, then doubles each subsequent failure while the cooldown
    /// is still active, capped at `COOLDOWN_MAX`. Once the cooldown expires
    /// naturally (entry is lazily cleaned on `is_unhealthy` miss), the
    /// counter resets — a candidate that came back healthy and failed again
    /// later starts fresh, not at its old penalty.
    ///
    /// Also records the failure reason in the per-pool Prometheus counter.
    /// When `unhealthy_candidates` is at `COOLDOWN_MAX_ENTRIES`, prunes
    /// expired entries one-shot before insert to keep the map bounded.
    pub fn mark_unhealthy(&self, host: &str, port: u16, reason: FailureReason) {
        crate::prometheus::FALLBACK_CANDIDATE_FAILURES_TOTAL
            .with_label_values(&[self.pool_name.as_str(), reason.as_str()])
            .inc();

        let now = Instant::now();
        let base = self.connect_timeout;
        let mut guard = self.unhealthy_candidates.lock();

        // Bounded growth: if at capacity, drop expired entries first.
        if guard.len() >= COOLDOWN_MAX_ENTRIES {
            guard.retain(|_, entry| entry.until > now);
        }

        let key = (host.to_string(), port);
        let entry = guard.entry(key).or_insert(CooldownEntry {
            until: now,
            attempts: 0,
            last_warn_at: None,
        });

        let still_active = entry.until > now;
        entry.attempts = if still_active {
            entry.attempts.saturating_add(1)
        } else {
            // Cooldown lapsed before we got around to noticing — start fresh.
            1
        };

        // 2^(attempts-1) * base, with overflow guard.
        let shift = entry.attempts.saturating_sub(1).min(20);
        let multiplier = 1u32 << shift;
        let next = base
            .checked_mul(multiplier)
            .unwrap_or(COOLDOWN_MAX)
            .min(COOLDOWN_MAX);
        entry.until = now + next;
    }

    /// True if `(host, port)` is currently within its cooldown window.
    /// Performs lazy cleanup of an expired entry on miss so the map does
    /// not retain dead members of churning clusters forever.
    pub fn is_unhealthy(&self, host: &str, port: u16) -> bool {
        let now = Instant::now();
        let mut guard = self.unhealthy_candidates.lock();
        let key = (host.to_string(), port);
        match guard.get(&key) {
            Some(entry) if entry.until > now => true,
            Some(_) => {
                guard.remove(&key);
                false
            }
            None => false,
        }
    }

    /// True if a WARN-level log line for `(host, port)` is allowed right now;
    /// returns false if we already emitted one within `COOLDOWN_LOG_RATE`.
    /// Side-effect: on a true return, records `now` so the next call within
    /// the window is suppressed. The candidate must already have a cooldown
    /// entry — call this immediately after `mark_unhealthy`, never before.
    pub fn should_log_unhealthy(&self, host: &str, port: u16) -> bool {
        let now = Instant::now();
        let mut guard = self.unhealthy_candidates.lock();
        let key = (host.to_string(), port);
        match guard.get_mut(&key) {
            Some(entry) => match entry.last_warn_at {
                Some(prev) if now.duration_since(prev) < COOLDOWN_LOG_RATE => false,
                _ => {
                    entry.last_warn_at = Some(now);
                    true
                }
            },
            None => true,
        }
    }

    /// Build the candidate list the caller must iterate when establishing a
    /// fallback connection. Returns the list together with its source so the
    /// caller can distinguish a (potentially stale) whitelist hit from a
    /// fresh discovery — only the former warrants a single retry round.
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
    pub async fn get_fallback_targets(
        &self,
    ) -> Result<(Vec<FallbackTarget>, TargetSource), String> {
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
                    return Ok((
                        vec![FallbackTarget {
                            host: host_owned,
                            port,
                            role: role_owned,
                            lifetime_ms: self.server_lifetime_ms,
                        }],
                        TargetSource::WhitelistCache,
                    ));
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

        Ok((
            alive
                .into_iter()
                .map(|(host, port, role)| FallbackTarget {
                    host,
                    port,
                    role,
                    lifetime_ms: self.server_lifetime_ms,
                })
                .collect(),
            TargetSource::Discovery,
        ))
    }

    /// Record a successful fallback host so subsequent calls hit
    /// `get_fallback_targets`'s whitelist branch and skip discovery.
    /// Called from `server_pool` after `Server::startup` returns Ok.
    ///
    /// If an older whitelist entry existed under a different `(host, port)`,
    /// its Prometheus `FALLBACK_HOST` label is removed atomically — without
    /// this, after a switchover both `(old, 1.0)` and `(new, 1.0)` would
    /// remain visible, suggesting fallback is active on two hosts at once.
    pub fn set_whitelisted(&self, host: String, port: u16, role: Role) {
        let old = {
            let mut guard = self.whitelisted_host.lock();
            let old = guard.take();
            *guard = Some((host.clone(), port, role.clone()));
            old
        };
        if let Some((old_host, old_port, _)) = old {
            if (old_host.as_str(), old_port) != (host.as_str(), port) {
                let _ = crate::prometheus::FALLBACK_HOST.remove_label_values(&[
                    &self.pool_name,
                    &old_host,
                    &old_port.to_string(),
                ]);
            }
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
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        assert!(state.is_unhealthy("10.0.0.1", 5432));

        // Wait past the cooldown window; the next call must report healthy
        // again and lazy-clean the entry.
        std::thread::sleep(Duration::from_millis(70));
        assert!(!state.is_unhealthy("10.0.0.1", 5432));

        // Marking the same host again restarts the window.
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
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

        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
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

        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        state.mark_unhealthy("10.0.0.2", 5432, FailureReason::Other);
        state.clear();
        assert!(!state.is_unhealthy("10.0.0.1", 5432));
        assert!(!state.is_unhealthy("10.0.0.2", 5432));
    }

    #[test]
    fn mark_unhealthy_exponential_backoff() {
        let base_ms = 50;
        let state = FallbackState::new(
            "test_pool_backoff".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_millis(base_ms),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        // First mark — base cooldown.
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        let attempts1 = {
            let g = state.unhealthy_candidates.lock();
            g.get(&("10.0.0.1".to_string(), 5432)).unwrap().attempts
        };
        assert_eq!(attempts1, 1);

        // Second mark while still active — attempts grows, cooldown extends.
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        let attempts2 = {
            let g = state.unhealthy_candidates.lock();
            g.get(&("10.0.0.1".to_string(), 5432)).unwrap().attempts
        };
        assert_eq!(attempts2, 2);

        // Third — attempts=3 → cooldown = base * 4. Verify > 2*base to confirm
        // doubling actually applied.
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        let entry = {
            let g = state.unhealthy_candidates.lock();
            g.get(&("10.0.0.1".to_string(), 5432)).unwrap().clone()
        };
        assert_eq!(entry.attempts, 3);
        let remaining = entry.until.saturating_duration_since(Instant::now());
        assert!(
            remaining > Duration::from_millis(2 * base_ms),
            "remaining {:?} should reflect 4x base ({}ms)",
            remaining,
            base_ms
        );
    }

    #[test]
    fn mark_unhealthy_caps_at_max() {
        // base = 1s; after enough doublings the result must clamp at COOLDOWN_MAX (60s).
        let state = FallbackState::new(
            "test_pool_cap".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        for _ in 0..20 {
            state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        }
        let until = {
            let g = state.unhealthy_candidates.lock();
            g.get(&("10.0.0.1".to_string(), 5432)).unwrap().until
        };
        let remaining = until.saturating_duration_since(Instant::now());
        assert!(
            remaining <= COOLDOWN_MAX + Duration::from_millis(50),
            "remaining {:?} must not exceed COOLDOWN_MAX={:?}",
            remaining,
            COOLDOWN_MAX
        );
    }

    #[test]
    fn mark_unhealthy_resets_after_lazy_clean() {
        // After cooldown expires, attempts must reset to 1 — a candidate
        // that recovered and failed again is at base, not at its old penalty.
        let state = FallbackState::new(
            "test_pool_reset".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_millis(20),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        std::thread::sleep(Duration::from_millis(100));
        // Trigger lazy clean.
        assert!(!state.is_unhealthy("10.0.0.1", 5432));
        // Fresh mark.
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        let attempts = {
            let g = state.unhealthy_candidates.lock();
            g.get(&("10.0.0.1".to_string(), 5432)).unwrap().attempts
        };
        assert_eq!(attempts, 1);
    }

    #[test]
    fn mark_unhealthy_prunes_when_at_capacity() {
        let state = FallbackState::new(
            "test_pool_prune".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_millis(20),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        // Fill past capacity with short-lived entries, then wait them out.
        for port in 0..(COOLDOWN_MAX_ENTRIES as u16) {
            state.mark_unhealthy("10.0.0.1", port, FailureReason::Other);
        }
        assert_eq!(
            state.unhealthy_candidates.lock().len(),
            COOLDOWN_MAX_ENTRIES
        );
        std::thread::sleep(Duration::from_millis(60));

        // Adding one more triggers prune of expired entries before insert.
        state.mark_unhealthy("10.0.0.2", 5432, FailureReason::Other);
        let len = state.unhealthy_candidates.lock().len();
        assert!(
            len < COOLDOWN_MAX_ENTRIES,
            "after prune len must drop below cap, got {len}"
        );
    }

    #[test]
    fn should_log_unhealthy_rate_limits_per_host_port() {
        let state = FallbackState::new(
            "test_pool_log_rate".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(60),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);
        // First call after mark — must be allowed.
        assert!(state.should_log_unhealthy("10.0.0.1", 5432));
        // Within rate-limit window — suppressed.
        assert!(!state.should_log_unhealthy("10.0.0.1", 5432));
        // Different host:port has its own counter.
        state.mark_unhealthy("10.0.0.2", 5432, FailureReason::Other);
        assert!(state.should_log_unhealthy("10.0.0.2", 5432));
    }

    #[test]
    fn failure_reason_from_error_maps_correctly() {
        use crate::errors::ServerIdentifier;
        let id = ServerIdentifier::new("u".to_string(), "d", "p");

        assert_eq!(
            FailureReason::from(&Error::ConnectError("tcp refused".into())),
            FailureReason::ConnectError
        );
        assert_eq!(
            FailureReason::from(&Error::ConnectError(
                "server startup timed out to 1.2.3.4:5432 after 3000ms".into()
            )),
            FailureReason::Timeout
        );
        assert_eq!(
            FailureReason::from(&Error::ServerStartupError("auth fail".into(), id.clone())),
            FailureReason::StartupError
        );
        assert_eq!(
            FailureReason::from(&Error::ServerUnavailableError(
                "starting up".into(),
                id.clone(),
            )),
            FailureReason::ServerUnavailable
        );
    }

    #[test]
    fn set_whitelisted_replaces_old_metric_label() {
        let pool = "test_pool_label_swap";
        let state = FallbackState::new(
            pool.to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
            30_000,
        )
        .unwrap();

        state.set_whitelisted("10.0.0.1".to_string(), 5432, Role::SyncStandby);
        let v1 = crate::prometheus::FALLBACK_HOST
            .with_label_values(&[pool, "10.0.0.1", "5432"])
            .get();
        assert_eq!(v1, 1.0);

        state.set_whitelisted("10.0.0.2".to_string(), 5432, Role::SyncStandby);
        // Old label cleared during overwrite.
        let v_old = crate::prometheus::FALLBACK_HOST
            .with_label_values(&[pool, "10.0.0.1", "5432"])
            .get();
        // remove_label_values drops the metric entirely; reading again
        // recreates it at default 0.0. Either way, never 1.0 here.
        assert_eq!(v_old, 0.0);
        let v_new = crate::prometheus::FALLBACK_HOST
            .with_label_values(&[pool, "10.0.0.2", "5432"])
            .get();
        assert_eq!(v_new, 1.0);
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
        state.mark_unhealthy("127.0.0.1", port_marked, FailureReason::Other);

        let (targets, source) = state.get_fallback_targets().await.unwrap();
        assert_eq!(source, TargetSource::Discovery);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].port, port_alive);
    }

    #[tokio::test]
    async fn get_fallback_targets_returns_whitelist_cache_source_when_set() {
        let state = FallbackState::new(
            "test_pool_whitelist_source".to_string(),
            vec![],
            Duration::from_secs(10),
            Duration::from_millis(500),
            Duration::from_millis(500),
            30_000,
        )
        .unwrap();

        state.set_whitelisted("10.0.0.1".to_string(), 5432, Role::SyncStandby);

        let (targets, source) = state.get_fallback_targets().await.unwrap();
        assert_eq!(source, TargetSource::WhitelistCache);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].host, "10.0.0.1");
        assert_eq!(targets[0].port, 5432);
    }

    #[tokio::test]
    async fn get_fallback_targets_bypasses_whitelist_when_unhealthy() {
        // If the whitelisted host has been marked unhealthy in the meantime,
        // the cache must be ignored and discovery re-run. Source must reflect
        // the actual fetch path.
        let port_alive = start_alive_listener().await;
        let body = format!(
            r#"{{"members":[{{"name":"a","host":"127.0.0.1","port":{port_alive},"role":"replica","state":"streaming"}}]}}"#
        );
        let api_port = start_mock_patroni_success(body).await;

        let state = FallbackState::new(
            "test_pool_whitelist_unhealthy".to_string(),
            vec![format!("http://127.0.0.1:{api_port}/cluster")],
            Duration::from_secs(10),
            Duration::from_millis(500),
            Duration::from_millis(500),
            30_000,
        )
        .unwrap();

        state.set_whitelisted("10.0.0.1".to_string(), 5432, Role::SyncStandby);
        state.mark_unhealthy("10.0.0.1", 5432, FailureReason::Other);

        let (targets, source) = state.get_fallback_targets().await.unwrap();
        assert_eq!(source, TargetSource::Discovery);
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
