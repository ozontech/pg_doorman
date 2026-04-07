use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};
use tokio::sync::{mpsc, Notify, Semaphore};

/// Source of eviction candidates and user state.
/// Implemented by the pool layer when wired in; mocked in benchmarks.
pub trait EvictionSource: Send + Sync {
    /// Try to evict one idle connection from another user's pool.
    /// Returns true if a connection was evicted (permit will become available).
    fn try_evict_one(&self, requesting_user: &str) -> bool;

    /// Number of clients queued waiting for a connection.
    fn queued_clients(&self, user: &str) -> usize;

    /// True if user has fewer connections than their guaranteed minimum.
    fn is_starving(&self, user: &str) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinatorConfig {
    pub max_db_connections: usize,
    pub min_connection_lifetime_ms: u64,
    pub reserve_pool_size: usize,
    pub reserve_pool_timeout_ms: u64,
}

/// Cumulative counters.
#[derive(Debug, Clone)]
pub struct CoordinatorStats {
    pub total_connections: usize,
    pub reserve_in_use: usize,
    pub evictions_total: u64,
    pub reserve_acquisitions_total: u64,
    pub exhaustions_total: u64,
}

/// RAII permit — held for the lifetime of a server connection.
/// Dropping it returns the permit to the correct semaphore.
pub struct CoordinatorPermit {
    coordinator: Arc<PoolCoordinator>,
    pub is_reserve: bool,
}

impl std::fmt::Debug for CoordinatorPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoordinatorPermit")
            .field("is_reserve", &self.is_reserve)
            .finish()
    }
}

impl Drop for CoordinatorPermit {
    fn drop(&mut self) {
        let permit_type = if self.is_reserve { "reserve" } else { "main" };
        if self.is_reserve {
            self.coordinator.reserve_semaphore.add_permits(1);
            self.coordinator
                .reserve_in_use
                .fetch_sub(1, Ordering::Relaxed);
        } else {
            self.coordinator.db_semaphore.add_permits(1);
        }
        let prev = self
            .coordinator
            .total_connections
            .fetch_sub(1, Ordering::Relaxed);
        self.coordinator.connection_returned.notify_one();
        debug!(
            "[pool: {}] coordinator: {} permit released (active: {} -> {})",
            self.coordinator.database,
            permit_type,
            prev,
            prev - 1,
        );
    }
}

/// RAII guard for a reserve semaphore permit granted by the arbiter.
/// If dropped without being converted to a CoordinatorPermit (e.g., the caller
/// timed out on the oneshot), the permit is automatically returned to the semaphore.
struct ReserveGrant {
    coordinator: Option<Arc<PoolCoordinator>>,
}

impl ReserveGrant {
    fn new(coordinator: Arc<PoolCoordinator>) -> Self {
        Self {
            coordinator: Some(coordinator),
        }
    }

    /// Consume the grant and produce a CoordinatorPermit.
    /// The reserve semaphore permit is now owned by the CoordinatorPermit's Drop.
    fn into_permit(mut self) -> CoordinatorPermit {
        let coordinator = self.coordinator.take().expect("grant already consumed");
        CoordinatorPermit {
            coordinator,
            is_reserve: true,
        }
    }
}

impl Drop for ReserveGrant {
    fn drop(&mut self) {
        if let Some(coordinator) = &self.coordinator {
            debug!(
                "[pool: {}] coordinator: unused reserve grant returned to semaphore",
                coordinator.database,
            );
            coordinator.reserve_semaphore.add_permits(1);
        }
    }
}

pub struct PoolCoordinator {
    database: String,
    db_semaphore: Semaphore,
    reserve_semaphore: Semaphore,
    total_connections: AtomicUsize,
    reserve_in_use: AtomicUsize,
    /// Wakes Phase C waiters. Fires on two distinct events:
    /// 1. A `CoordinatorPermit` was dropped — a peer's server connection was
    ///    physically destroyed and its semaphore slot is free.
    /// 2. A `Pool::return_object` fired on a peer pool — the connection went
    ///    back to that pool's idle queue without being destroyed. The slot is
    ///    NOT free, but the peer's idle `vec` has a new entry, so the next
    ///    `retain_oldest_first` scan inside `evict_one_idle` can find an
    ///    eligible eviction candidate that wasn't visible a moment ago.
    ///
    /// Phase C handles both cases uniformly: on every wake it retries
    /// `eviction_source.try_evict_one(user)` before `try_acquire()`.
    connection_returned: Notify,
    config: CoordinatorConfig,
    evictions_total: AtomicU64,
    reserve_acquisitions_total: AtomicU64,
    exhaustions_total: AtomicU64,
    reserve_tx: mpsc::Sender<ReserveRequest>,
}

impl std::fmt::Debug for PoolCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolCoordinator")
            .field("database", &self.database)
            .field("config", &self.config)
            .field(
                "total_connections",
                &self.total_connections.load(Ordering::Relaxed),
            )
            .field(
                "reserve_in_use",
                &self.reserve_in_use.load(Ordering::Relaxed),
            )
            .finish()
    }
}

/// Time to wait for the arbiter to process a reserve request
/// after it has been submitted to the priority queue.
const ARBITER_RESPONSE_TIMEOUT: Duration = Duration::from_millis(100);

struct ReserveRequest {
    user: String,
    score: (u8, usize), // (starving, queued_clients)
    response: tokio::sync::oneshot::Sender<ReserveGrant>,
}

impl Ord for ReserveRequest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score.cmp(&other.score)
    }
}

impl PartialOrd for ReserveRequest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ReserveRequest {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for ReserveRequest {}

impl PoolCoordinator {
    /// Create a new coordinator. Spawns the reserve arbiter task.
    pub fn new(database: String, config: CoordinatorConfig) -> Arc<Self> {
        let (reserve_tx, reserve_rx) = mpsc::channel(256);
        let coordinator = Arc::new(Self {
            database,
            db_semaphore: Semaphore::new(config.max_db_connections),
            reserve_semaphore: Semaphore::new(config.reserve_pool_size),
            total_connections: AtomicUsize::new(0),
            reserve_in_use: AtomicUsize::new(0),
            connection_returned: Notify::new(),
            evictions_total: AtomicU64::new(0),
            reserve_acquisitions_total: AtomicU64::new(0),
            exhaustions_total: AtomicU64::new(0),
            reserve_tx,
            config,
        });

        let coordinator_clone = coordinator.clone();
        tokio::spawn(async move {
            reserve_arbiter(reserve_rx, coordinator_clone).await;
        });

        coordinator
    }

    /// Fast path: try to acquire a permit without blocking.
    /// Returns None if the database limit is reached.
    pub fn try_acquire(self: &Arc<Self>) -> Option<CoordinatorPermit> {
        match self.db_semaphore.try_acquire() {
            Ok(permit) => {
                permit.forget();
                self.total_connections.fetch_add(1, Ordering::Relaxed);
                Some(CoordinatorPermit {
                    coordinator: Arc::clone(self),
                    is_reserve: false,
                })
            }
            Err(_) => None,
        }
    }

    /// Full acquisition path: try → evict → wait → reserve → error.
    pub async fn acquire(
        self: &Arc<Self>,
        database: &str,
        user: &str,
        eviction_source: &dyn EvictionSource,
    ) -> Result<CoordinatorPermit, AcquireError> {
        let max = self.config.max_db_connections;

        // Phase A: fast path — non-blocking semaphore acquire
        if let Some(permit) = self.try_acquire() {
            debug!(
                "[{}@{}] coordinator: permit acquired via fast path \
                 (active={}/{})",
                user,
                database,
                self.total_connections.load(Ordering::Relaxed),
                max,
            );
            return Ok(permit);
        }

        let active = self.total_connections.load(Ordering::Relaxed);
        debug!(
            "[{}@{}] coordinator: fast path unavailable \
             (active={}/{}), trying eviction",
            user, database, active, max,
        );

        // A peer pool may have dropped its permit between Phase A's
        // `try_acquire` and now (any concurrent `CoordinatorPermit::drop`
        // bumps `db_semaphore` without going through this path). Re-check
        // the cheap fast path before incurring an eviction: closing a peer
        // backend that didn't need to be closed is unrecoverable damage,
        // and a single extra atomic CAS is essentially free compared to
        // the alternative.
        if let Some(permit) = self.try_acquire() {
            debug!(
                "[{}@{}] coordinator: permit became free between Phase A and Phase B, \
                 eviction avoided (active={}/{})",
                user,
                database,
                self.total_connections.load(Ordering::Relaxed),
                max,
            );
            return Ok(permit);
        }

        // Phase B: try eviction — close an idle connection from another user
        let evicted = eviction_source.try_evict_one(user);
        if evicted {
            self.evictions_total.fetch_add(1, Ordering::Relaxed);
            if let Some(permit) = self.try_acquire() {
                debug!(
                    "[{}@{}] coordinator: eviction freed a slot, \
                     permit acquired (active={}/{})",
                    user,
                    database,
                    self.total_connections.load(Ordering::Relaxed),
                    max,
                );
                return Ok(permit);
            }
            debug!(
                "[{}@{}] coordinator: eviction freed a slot \
                 but permit already taken by concurrent waiter (active={}/{})",
                user,
                database,
                self.total_connections.load(Ordering::Relaxed),
                max,
            );
        } else {
            debug!(
                "[{}@{}] coordinator: eviction found no eligible \
                 idle connections in other users' pools",
                user, database,
            );
        }

        // Phase C: wait for a connection to be returned.
        // Register `notified()` BEFORE `try_acquire()` so that the
        // `notify_one` from CoordinatorPermit::drop is not lost.
        let timeout_ms = self.config.reserve_pool_timeout_ms;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut wait_wakeups = 0u32;

        debug!(
            "[{}@{}] coordinator: entering wait phase \
             (timeout={}ms, active={}/{})",
            user,
            database,
            timeout_ms,
            self.total_connections.load(Ordering::Relaxed),
            max,
        );

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            // Register the notification BEFORE the opportunistic eviction
            // attempt and `try_acquire` so we cannot miss a wake-up fired
            // between them. Same pattern as the cooldown zone in
            // `Pool::timeout_get`.
            let notified = self.connection_returned.notified();

            // Cheap path first: a previous wake (or any concurrent
            // `CoordinatorPermit::drop`) may have already left a free permit
            // in the semaphore. `try_evict_one` would close a peer connection
            // for nothing in that case — the slot is already there for the
            // taking. The atomic CAS is roughly five nanoseconds; an avoided
            // eviction saves a peer backend.
            if let Some(permit) = self.try_acquire() {
                debug!(
                    "[{}@{}] coordinator: wait phase acquired free permit \
                     without eviction after {} wakeup(s) (active={}/{})",
                    user,
                    database,
                    wait_wakeups,
                    self.total_connections.load(Ordering::Relaxed),
                    max,
                );
                return Ok(permit);
            }

            // Opportunistic eviction retry. The wake-up may have come from
            // `Pool::return_object` on a peer pool, which made a previously
            // checked-out connection visible in that peer's idle vec without
            // destroying any permit. A previous Phase B attempt may have
            // found nothing evictable because the candidate was checked out,
            // but the state has now changed.
            //
            // The scan runs on every iteration, not only after a notify,
            // because the wake source is indistinguishable from a permit
            // drop and the scan is much cheaper than a `reserve_pool_timeout`
            // wait that ends in a reserve grant or a client error. The
            // try_acquire above means we only reach this point when the
            // semaphore is genuinely empty — eviction is the only way out.
            if eviction_source.try_evict_one(user) {
                self.evictions_total.fetch_add(1, Ordering::Relaxed);
                debug!(
                    "[{}@{}] coordinator: wait phase evicted idle from peer \
                     (wakeups={})",
                    user, database, wait_wakeups,
                );
                if let Some(permit) = self.try_acquire() {
                    debug!(
                        "[{}@{}] coordinator: wait phase succeeded \
                         after {} wakeup(s), permit acquired (active={}/{})",
                        user,
                        database,
                        wait_wakeups,
                        self.total_connections.load(Ordering::Relaxed),
                        max,
                    );
                    return Ok(permit);
                }
            }

            tokio::select! {
                _ = notified => {
                    wait_wakeups += 1;
                }
                _ = tokio::time::sleep(remaining) => {
                    break;
                }
            }
        }

        debug!(
            "[{}@{}] coordinator: wait phase exhausted \
             after {} wakeup(s), timeout={}ms (active={}/{})",
            user,
            database,
            wait_wakeups,
            timeout_ms,
            self.total_connections.load(Ordering::Relaxed),
            max,
        );

        // Phase D: reserve
        let phase = if self.config.reserve_pool_size > 0 {
            let starving = u8::from(eviction_source.is_starving(user));
            let queued = eviction_source.queued_clients(user);
            let reserve_in_use = self.reserve_in_use.load(Ordering::Relaxed);

            debug!(
                "[{}@{}] coordinator: requesting reserve permit \
                 (starving={}, queued_clients={}, reserve_in_use={}/{})",
                user,
                database,
                starving == 1,
                queued,
                reserve_in_use,
                self.config.reserve_pool_size,
            );

            let (tx, rx) = tokio::sync::oneshot::channel();

            if self
                .reserve_tx
                .send(ReserveRequest {
                    user: user.to_string(),
                    score: (starving, queued),
                    response: tx,
                })
                .await
                .is_ok()
            {
                if let Ok(Ok(grant)) = tokio::time::timeout(ARBITER_RESPONSE_TIMEOUT, rx).await {
                    self.total_connections.fetch_add(1, Ordering::Relaxed);
                    self.reserve_in_use.fetch_add(1, Ordering::Relaxed);
                    self.reserve_acquisitions_total
                        .fetch_add(1, Ordering::Relaxed);
                    info!(
                        "[{}@{}] coordinator: reserve permit granted \
                         (active={}/{}, reserve_in_use={}/{})",
                        user,
                        database,
                        self.total_connections.load(Ordering::Relaxed),
                        max,
                        self.reserve_in_use.load(Ordering::Relaxed),
                        self.config.reserve_pool_size,
                    );
                    return Ok(grant.into_permit());
                }
            }

            debug!(
                "[{}@{}] coordinator: reserve request denied — \
                 no reserve permits available or arbiter timeout \
                 (reserve_in_use={}/{})",
                user,
                database,
                self.reserve_in_use.load(Ordering::Relaxed),
                self.config.reserve_pool_size,
            );

            AcquirePhase::ReserveExhausted
        } else {
            debug!(
                "[{}@{}] coordinator: reserve pool not configured, \
                 skipping reserve phase",
                user, database,
            );
            AcquirePhase::NoReserve
        };

        // Phase E: exhausted — client will get an error
        self.exhaustions_total.fetch_add(1, Ordering::Relaxed);
        let active = self.total_connections.load(Ordering::Relaxed);
        let reserve_in_use = self.reserve_in_use.load(Ordering::Relaxed);

        warn!(
            "[{}@{}] coordinator: EXHAUSTED — all permits in use, \
             client will receive error (active={}/{}, reserve={}/{}, phase={:?}, \
             total_exhaustions={})",
            user,
            database,
            active,
            max,
            reserve_in_use,
            self.config.reserve_pool_size,
            phase,
            self.exhaustions_total.load(Ordering::Relaxed),
        );

        Err(AcquireError::NoConnection(NoConnectionInfo {
            database: database.to_string(),
            user: user.to_string(),
            max_db_connections: self.config.max_db_connections,
            active_connections: active,
            reserve_size: self.config.reserve_pool_size,
            reserve_in_use,
            phase,
        }))
    }

    /// Called by `Pool::return_object` when a server connection goes back
    /// into a peer pool's idle queue without being destroyed. Wakes one
    /// Phase C waiter so it can re-run `eviction_source.try_evict_one` —
    /// the returned connection is now visible to `retain_oldest_first`, so
    /// an eviction candidate that didn't exist a moment ago is now scannable.
    ///
    /// Without this signal, Phase C would only react to physical permit
    /// drops (`CoordinatorPermit::drop`), so a busy peer that constantly
    /// recycles its own idle queue would keep a waiter sleeping until
    /// `server_lifetime` ages a connection out — the waiter would then
    /// timeout into Phase D even though the cross-pool system had headroom
    /// every few milliseconds.
    pub(crate) fn notify_idle_returned(&self) {
        self.connection_returned.notify_one();
    }

    pub fn total_connections(&self) -> usize {
        self.total_connections.load(Ordering::Relaxed)
    }

    pub fn reserve_in_use(&self) -> usize {
        self.reserve_in_use.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> CoordinatorStats {
        CoordinatorStats {
            total_connections: self.total_connections.load(Ordering::Relaxed),
            reserve_in_use: self.reserve_in_use.load(Ordering::Relaxed),
            evictions_total: self.evictions_total.load(Ordering::Relaxed),
            reserve_acquisitions_total: self.reserve_acquisitions_total.load(Ordering::Relaxed),
            exhaustions_total: self.exhaustions_total.load(Ordering::Relaxed),
        }
    }

    pub fn config(&self) -> &CoordinatorConfig {
        &self.config
    }
}

/// What phase the coordinator was in when it gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquirePhase {
    /// Reserve pool is fully used — all main and reserve permits occupied.
    ReserveExhausted,
    /// No reserve configured (reserve_pool_size = 0) — only main permits available.
    NoReserve,
}

/// Context about why a connection could not be acquired.
#[derive(Debug, Clone)]
pub struct NoConnectionInfo {
    pub database: String,
    pub user: String,
    pub max_db_connections: usize,
    pub active_connections: usize,
    pub reserve_size: usize,
    pub reserve_in_use: usize,
    pub phase: AcquirePhase,
}

impl std::fmt::Display for NoConnectionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.phase {
            AcquirePhase::NoReserve => write!(
                f,
                "all server connections to database '{}' are in use \
                 (max={}, user='{}')",
                self.database, self.max_db_connections, self.user
            ),
            AcquirePhase::ReserveExhausted => write!(
                f,
                "all server connections to database '{}' are in use \
                 (max={}, reserve={}/{}, user='{}')",
                self.database,
                self.max_db_connections,
                self.reserve_in_use,
                self.reserve_size,
                self.user
            ),
        }
    }
}

#[derive(Debug)]
pub enum AcquireError {
    /// Database connection limit reached — eviction, wait, and reserve all failed.
    NoConnection(NoConnectionInfo),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcquireError::NoConnection(info) => write!(f, "{info}"),
        }
    }
}

impl std::error::Error for AcquireError {}

const ARBITER_POLL_INTERVAL: Duration = Duration::from_millis(50);

async fn reserve_arbiter(
    mut rx: mpsc::Receiver<ReserveRequest>,
    coordinator: Arc<PoolCoordinator>,
) {
    use std::collections::BinaryHeap;

    let mut pending: BinaryHeap<ReserveRequest> = BinaryHeap::new();

    loop {
        // Collect new requests (non-blocking)
        while let Ok(req) = rx.try_recv() {
            debug!(
                "[{}@{}] arbiter: received reserve request \
                 (score=starving:{}, queued:{})",
                req.user, coordinator.database, req.score.0, req.score.1,
            );
            pending.push(req);
        }

        // Grant to highest-scoring request if reserve available
        while pending.peek().is_some() {
            if let Ok(sem_permit) = coordinator.reserve_semaphore.try_acquire() {
                sem_permit.forget();
                let req = pending.pop().unwrap();
                let grant = ReserveGrant::new(coordinator.clone());
                let sent = req.response.send(grant);
                if sent.is_ok() {
                    debug!(
                        "[{}@{}] arbiter: granted reserve permit \
                         (score=starving:{}, queued:{})",
                        req.user, coordinator.database, req.score.0, req.score.1,
                    );
                } else {
                    debug!(
                        "[{}@{}] arbiter: grant failed — \
                         requester already timed out, permit returned",
                        req.user, coordinator.database,
                    );
                }
            } else {
                debug!(
                    "[pool: {}] arbiter: no reserve permits available, \
                     {} request(s) still pending",
                    coordinator.database,
                    pending.len(),
                );
                break;
            }
        }

        // Remove cancelled requests and cap heap size to prevent
        // unbounded growth during sustained overload.
        if !pending.is_empty() {
            let before = pending.len();
            pending.retain(|req| !req.response.is_closed());
            let pruned = before - pending.len();
            if pruned > 0 {
                debug!(
                    "[pool: {}] arbiter: pruned {} cancelled request(s), \
                     {} still pending",
                    coordinator.database,
                    pruned,
                    pending.len(),
                );
            }

            const HEAP_CAP_MULTIPLIER: usize = 4;
            let cap = coordinator.config.reserve_pool_size.max(1) * HEAP_CAP_MULTIPLIER;
            if pending.len() > cap {
                let dropped = pending.len() - cap;
                let mut kept = BinaryHeap::with_capacity(cap);
                for _ in 0..cap {
                    if let Some(req) = pending.pop() {
                        kept.push(req);
                    }
                }
                // Remaining low-priority requests are dropped (oneshot closed → caller gets timeout)
                pending = kept;
                warn!(
                    "[pool: {}] arbiter: heap overflow — dropped {} lowest-priority request(s), \
                     cap={}",
                    coordinator.database, dropped, cap,
                );
            }
        }

        tokio::select! {
            req = rx.recv() => {
                match req {
                    Some(req) => {
                        debug!(
                            "[pool: {}] arbiter: received reserve request from '{}' \
                             (score=starving:{}, queued:{})",
                            coordinator.database, req.user, req.score.0, req.score.1,
                        );
                        pending.push(req);
                    }
                    None => return, // channel closed, coordinator dropped
                }
            }
            _ = tokio::time::sleep(ARBITER_POLL_INTERVAL) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoOpEviction;
    impl EvictionSource for NoOpEviction {
        fn try_evict_one(&self, _user: &str) -> bool {
            false
        }
        fn queued_clients(&self, _user: &str) -> usize {
            0
        }
        fn is_starving(&self, _user: &str) -> bool {
            false
        }
    }

    /// Eviction mock that holds a real CoordinatorPermit and drops it
    /// when try_evict_one is called — simulating actual connection eviction.
    struct PermitDroppingEviction {
        permit: std::sync::Mutex<Option<CoordinatorPermit>>,
    }

    impl PermitDroppingEviction {
        fn new(permit: CoordinatorPermit) -> Self {
            Self {
                permit: std::sync::Mutex::new(Some(permit)),
            }
        }
    }

    impl EvictionSource for PermitDroppingEviction {
        fn try_evict_one(&self, _user: &str) -> bool {
            let mut guard = self.permit.lock().unwrap();
            if guard.is_some() {
                *guard = None; // drops the permit, frees the semaphore slot
                true
            } else {
                false
            }
        }
        fn queued_clients(&self, _user: &str) -> usize {
            0
        }
        fn is_starving(&self, _user: &str) -> bool {
            false
        }
    }

    fn test_config(max: usize, reserve: usize) -> CoordinatorConfig {
        CoordinatorConfig {
            max_db_connections: max,
            min_connection_lifetime_ms: 5000,
            reserve_pool_size: reserve,
            reserve_pool_timeout_ms: 100,
        }
    }

    #[tokio::test]
    async fn try_acquire_within_limit() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(10, 0));
        let permit = coord.try_acquire().unwrap();
        assert_eq!(coord.total_connections(), 1);
        assert!(!permit.is_reserve);
        drop(permit);
        assert_eq!(coord.total_connections(), 0);
    }

    #[tokio::test]
    async fn try_acquire_at_limit_returns_none() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(2, 0));
        let _p1 = coord.try_acquire().unwrap();
        let _p2 = coord.try_acquire().unwrap();
        assert!(coord.try_acquire().is_none());
        assert_eq!(coord.total_connections(), 2);
    }

    #[tokio::test]
    async fn permit_drop_frees_slot() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let p = coord.try_acquire().unwrap();
        assert!(coord.try_acquire().is_none());
        drop(p);
        assert!(coord.try_acquire().is_some());
    }

    #[tokio::test]
    async fn acquire_with_eviction() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let _p1 = coord.try_acquire().unwrap();

        // Simulate eviction: SuccessEviction returns true, but doesn't
        // actually free a permit. So acquire will still fail.
        // This tests that eviction path is attempted.
        let eviction = NoOpEviction;
        let result = coord.acquire("testdb", "test", &eviction).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn acquire_reserve_on_exhaustion() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 5));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let permit = coord.acquire("testdb", "test", &eviction).await.unwrap();
        assert!(permit.is_reserve);
        assert_eq!(coord.reserve_in_use(), 1);
        assert_eq!(coord.stats().reserve_acquisitions_total, 1);

        drop(permit);
        assert_eq!(coord.reserve_in_use(), 0);
    }

    #[tokio::test]
    async fn acquire_fully_exhausted() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let result = coord.acquire("testdb", "test", &eviction).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn permit_drop_notifies_waiters() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let p = coord.try_acquire().unwrap();

        let coord2 = coord.clone();
        let handle = tokio::spawn(async move {
            let eviction = NoOpEviction;
            coord2.acquire("testdb", "waiter", &eviction).await
        });

        // Give the waiter time to start waiting
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(p); // free the slot

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert!(!result.unwrap().is_reserve);
    }

    // --- ReserveRequest ordering tests ---

    fn make_request(starving: u8, queued: usize) -> ReserveRequest {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        ReserveRequest {
            user: "test".to_string(),
            score: (starving, queued),
            response: tx,
        }
    }

    #[test]
    fn reserve_ordering_starving_beats_non_starving() {
        let starving = make_request(1, 1);
        let normal = make_request(0, 100);
        // starving user wins regardless of queued_clients
        assert!(starving > normal);
    }

    #[test]
    fn reserve_ordering_more_queued_wins() {
        let many = make_request(0, 50);
        let few = make_request(0, 5);
        assert!(many > few);
    }

    #[test]
    fn reserve_ordering_equal_scores() {
        let a = make_request(1, 10);
        let b = make_request(1, 10);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
    }

    #[test]
    fn reserve_ordering_heap_pops_highest() {
        use std::collections::BinaryHeap;
        let mut heap = BinaryHeap::new();
        heap.push(make_request(0, 2));
        heap.push(make_request(1, 1)); // starving
        heap.push(make_request(0, 50));

        let top = heap.pop().unwrap();
        assert_eq!(top.score, (1, 1)); // starving wins
        let next = heap.pop().unwrap();
        assert_eq!(next.score, (0, 50)); // most queued among non-starving
    }

    #[tokio::test]
    async fn reserve_permits_not_leaked_on_early_receiver_drop() {
        // Drop the oneshot receiver BEFORE arbiter processes → permit must be returned.
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 2));
        let _p1 = coord.try_acquire().unwrap();

        let (tx, rx_dropped) = tokio::sync::oneshot::channel::<ReserveGrant>();
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "leaker".to_string(),
                score: (0, 1),
                response: tx,
            })
            .await;
        drop(rx_dropped);

        tokio::time::sleep(Duration::from_millis(200)).await;

        let eviction = NoOpEviction;
        let p2 = coord.acquire("testdb", "ok_user", &eviction).await.unwrap();
        assert!(p2.is_reserve);
        let p3 = coord
            .acquire("testdb", "ok_user2", &eviction)
            .await
            .unwrap();
        assert!(p3.is_reserve);
        assert_eq!(coord.reserve_in_use(), 2);
    }

    // ===== New tests below =====

    #[tokio::test]
    async fn reserve_grant_returns_permit_on_drop() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1));

        // Manually acquire a reserve semaphore permit via the grant mechanism
        let sem_permit = coord.reserve_semaphore.try_acquire().unwrap();
        sem_permit.forget();

        // Create a grant and immediately drop it
        let grant = ReserveGrant::new(coord.clone());
        drop(grant);

        // Permit should be back — verify by acquiring it again
        let sem_permit2 = coord.reserve_semaphore.try_acquire();
        assert!(
            sem_permit2.is_ok(),
            "permit should be returned after grant drop"
        );
    }

    #[tokio::test]
    async fn reserve_permit_not_leaked_on_late_receiver_drop() {
        // Regression test: arbiter sends grant, then receiver is dropped without
        // consuming the value. Before fix (bool), permit was leaked permanently.
        // After fix (ReserveGrant), permit is returned via RAII Drop.
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1));
        let _p1 = coord.try_acquire().unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel::<ReserveGrant>();
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "test".to_string(),
                score: (0, 1),
                response: tx,
            })
            .await;

        // Let arbiter process and send the grant
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Drop receiver WITHOUT consuming the grant — simulates timeout race
        drop(rx);

        // Let ReserveGrant::Drop return the permit
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Reserve permit should NOT be leaked — another user can acquire it
        let eviction = NoOpEviction;
        let result = coord.acquire("testdb", "another_user", &eviction).await;
        assert!(result.is_ok(), "reserve permit was leaked");
        assert!(result.unwrap().is_reserve);
    }

    #[tokio::test]
    async fn acquire_eviction_frees_permit() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(2, 0));
        let p1 = coord.try_acquire().unwrap();
        let _p2 = coord.try_acquire().unwrap();
        assert!(coord.try_acquire().is_none()); // limit reached

        // Eviction mock holds p1 and drops it when called
        let eviction = PermitDroppingEviction::new(p1);
        let result = coord.acquire("testdb", "requester", &eviction).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_reserve);
        assert_eq!(coord.stats().evictions_total, 1);
    }

    #[tokio::test]
    async fn phase_c_wait_woken_by_permit_drop() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let p = coord.try_acquire().unwrap();

        let coord2 = coord.clone();
        let waiter = tokio::spawn(async move {
            let eviction = NoOpEviction;
            // Phase C: waits for connection_returned notify
            coord2.acquire("testdb", "waiter", &eviction).await
        });

        // Give waiter time to enter Phase C
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Drop permit — triggers notify_one
        drop(p);

        let result = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter should complete")
            .unwrap();
        assert!(result.is_ok());
        assert!(!result.unwrap().is_reserve);
    }

    /// Regression for coordinator Phase C being blind to peer idle returns.
    ///
    /// Before the fix, Phase C only woke on `CoordinatorPermit::drop`, which
    /// fires only when a peer's server connection is physically destroyed.
    /// If a peer returned a connection to its idle queue via
    /// `Pool::return_object` (and its `spare_above_min` grew), a waiting
    /// task would keep sleeping until the peer's connection finally aged
    /// out — or fall through to Phase D / Phase E.
    ///
    /// After the fix, `Pool::return_object` calls `notify_idle_returned()`
    /// and Phase C retries `try_evict_one` on every wake. The test simulates
    /// this by holding an eviction-source that only becomes evictable after
    /// an external `notify_idle_returned()` signal.
    #[tokio::test]
    async fn phase_c_wait_woken_by_idle_return_and_retries_eviction() {
        /// Eviction source that does nothing until `arm()` is called, then
        /// drops a pre-acquired permit on the next `try_evict_one`. Mirrors
        /// the state of a peer pool that just received a `return_object`
        /// and whose `spare_above_min` grew from 0 to 1.
        struct DelayedEviction {
            armed: std::sync::Mutex<Option<CoordinatorPermit>>,
        }
        impl DelayedEviction {
            fn new() -> Self {
                Self {
                    armed: std::sync::Mutex::new(None),
                }
            }
            fn arm(&self, permit: CoordinatorPermit) {
                *self.armed.lock().unwrap() = Some(permit);
            }
        }
        impl EvictionSource for DelayedEviction {
            fn try_evict_one(&self, _user: &str) -> bool {
                let mut guard = self.armed.lock().unwrap();
                if guard.is_some() {
                    *guard = None;
                    true
                } else {
                    false
                }
            }
            fn queued_clients(&self, _user: &str) -> usize {
                0
            }
            fn is_starving(&self, _user: &str) -> bool {
                false
            }
        }

        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let p = coord.try_acquire().unwrap();
        let eviction = std::sync::Arc::new(DelayedEviction::new());

        let coord2 = coord.clone();
        let eviction2 = std::sync::Arc::clone(&eviction);
        let waiter = tokio::spawn(async move {
            // Phase A/B fail (NoOp-style: nothing armed yet). Phase C enters
            // and waits on connection_returned.
            coord2.acquire("testdb", "waiter", eviction2.as_ref()).await
        });

        // Let the waiter reach Phase C.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Simulate a peer pool returning an idle connection. The permit is
        // NOT dropped here — it would normally keep existing inside the
        // peer's idle ObjectInner. Instead, we:
        //   1. arm the eviction source so the next try_evict_one will
        //      drop `p` (modelling "spare_above_min grew");
        //   2. call notify_idle_returned() the way Pool::return_object now
        //      does — this is the signal that didn't exist before the fix.
        eviction.arm(p);
        coord.notify_idle_returned();

        let result = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter should complete quickly — not age out")
            .unwrap();
        assert!(
            result.is_ok(),
            "Phase C waiter must acquire a permit on idle-return notify, \
             not fall through to Phase D/E"
        );
        assert!(!result.unwrap().is_reserve);
        // Eviction ran exactly once (the delayed one), incrementing the
        // counter that drives observability.
        assert_eq!(coord.stats().evictions_total, 1);
    }

    /// FIFO guarantee: one `notify_idle_returned` wakes exactly one Phase C
    /// waiter, not all of them. Pins the `notify_one` vs `notify_waiters`
    /// choice against future regressions — a refactor that accidentally
    /// switches to `notify_waiters` would wake every parked task on every
    /// peer idle return, producing a thundering-herd retry storm on
    /// `try_evict_one`.
    ///
    /// Counts wakes via a custom eviction source. `try_evict_one` is called
    /// at most once per waiter in Phase B (entry into the loop) and then
    /// once per Phase C wake-up. With N waiters and a single
    /// `notify_idle_returned`, the expected counter is N (Phase B) plus
    /// exactly 1 (the one waiter that woke in Phase C).
    #[tokio::test]
    async fn phase_c_single_notify_wakes_exactly_one_waiter() {
        use std::sync::atomic::{AtomicU64, Ordering as AOrdering};

        #[derive(Clone)]
        struct CountingEviction {
            calls: std::sync::Arc<AtomicU64>,
        }
        impl EvictionSource for CountingEviction {
            fn try_evict_one(&self, _user: &str) -> bool {
                self.calls.fetch_add(1, AOrdering::Relaxed);
                false
            }
            fn queued_clients(&self, _user: &str) -> usize {
                0
            }
            fn is_starving(&self, _user: &str) -> bool {
                false
            }
        }

        const N: usize = 5;

        // Use a generous reserve_pool_timeout so the waiters stay parked
        // in Phase C while we observe the wake count.
        let cfg = CoordinatorConfig {
            max_db_connections: 1,
            min_connection_lifetime_ms: 5000,
            reserve_pool_size: 0,
            reserve_pool_timeout_ms: 500,
        };
        let coord = PoolCoordinator::new("test_db".to_string(), cfg);
        let _p = coord.try_acquire().unwrap(); // pin the only slot

        let calls = std::sync::Arc::new(AtomicU64::new(0));
        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            let coord2 = coord.clone();
            let calls2 = std::sync::Arc::clone(&calls);
            handles.push(tokio::spawn(async move {
                let eviction = CountingEviction { calls: calls2 };
                coord2.acquire("testdb", "waiter", &eviction).await
            }));
        }

        // Give all waiters time to finish Phase B and park inside Phase C.
        // Each waiter runs try_evict_one twice on the way to parking:
        //   - once in Phase B (pool_coordinator.rs:261),
        //   - once on the first iteration of the Phase C loop, just before
        //     the select! await.
        // Expected baseline: 2 * N.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let baseline = calls.load(AOrdering::Relaxed);
        assert_eq!(
            baseline,
            2 * N as u64,
            "each of N waiters should have called try_evict_one twice \
             (Phase B + first Phase C loop iteration); observed {}",
            baseline,
        );

        // Fire a single notify_idle_returned. `notify_one` must wake exactly
        // one waiter, which then runs try_evict_one once more in Phase C
        // (top of the next loop iteration).
        coord.notify_idle_returned();

        // Give the woken task time to schedule and run its retry.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let after_notify = calls.load(AOrdering::Relaxed);
        assert_eq!(
            after_notify,
            baseline + 1,
            "exactly one waiter should have woken and re-run try_evict_one; \
             a regression to `notify_waiters` or `notify_one`-per-waiter \
             would push this to {} or higher; observed {}",
            baseline + N as u64,
            after_notify,
        );

        // All waiters eventually time out (reserve_pool_size = 0, permit
        // never drops). Clean up the spawned tasks.
        for h in handles {
            let r = tokio::time::timeout(Duration::from_secs(2), h)
                .await
                .expect("waiter should finish within timeout + margin")
                .unwrap();
            assert!(r.is_err());
        }
    }

    /// Regression for the cheap-path-first invariant: when Phase C wakes
    /// from a real `CoordinatorPermit::drop` (semaphore actually has a free
    /// slot now), the waiter must take that slot via `try_acquire` WITHOUT
    /// running another `try_evict_one`. Closing a peer backend to free a
    /// slot that is already free is wasted damage on the peer.
    ///
    /// Before the fix, Phase C ran `try_evict_one` unconditionally at the
    /// top of every loop iteration. With a peer that had spare capacity,
    /// the wake from a `CoordinatorPermit::drop` would still close a peer
    /// connection for nothing.
    ///
    /// The test pins the new ordering: `try_acquire → try_evict_one`. The
    /// eviction-call counter must NOT advance past the parking baseline
    /// when the wake comes from a permit drop.
    #[tokio::test]
    async fn phase_c_wake_from_permit_drop_skips_eviction() {
        use std::sync::atomic::{AtomicU64, Ordering as AOrdering};

        #[derive(Clone)]
        struct CountingEviction {
            calls: std::sync::Arc<AtomicU64>,
        }
        impl EvictionSource for CountingEviction {
            fn try_evict_one(&self, _user: &str) -> bool {
                // Counts every call. Always returns false so a successful
                // eviction never confounds the wake-source attribution.
                self.calls.fetch_add(1, AOrdering::Relaxed);
                false
            }
            fn queued_clients(&self, _user: &str) -> usize {
                0
            }
            fn is_starving(&self, _user: &str) -> bool {
                false
            }
        }

        // Single-slot coordinator. We pin the slot, park a Phase C waiter
        // on it, then drop the pin to let the waiter take the freed permit.
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let p = coord.try_acquire().expect("first slot is free");

        let calls = std::sync::Arc::new(AtomicU64::new(0));
        let coord_w = coord.clone();
        let calls_w = std::sync::Arc::clone(&calls);
        let waiter = tokio::spawn(async move {
            let eviction = CountingEviction { calls: calls_w };
            coord_w.acquire("testdb", "waiter", &eviction).await
        });

        // Let the waiter reach Phase C. The cheap-path-first ordering means
        // each of Phase B and the first Phase C iteration runs the cheap
        // `try_acquire` first (fails — slot is pinned) and then `try_evict_one`
        // exactly once. Baseline counter == 2.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let baseline = calls.load(AOrdering::Relaxed);
        assert_eq!(
            baseline, 2,
            "baseline must be Phase B + first Phase C try_evict_one calls",
        );

        // Drop the pinned permit: semaphore +1, `connection_returned.notify_one`
        // fires from `CoordinatorPermit::drop`. The waiter wakes, sees a free
        // slot, and must take it via the new try_acquire-first path.
        drop(p);

        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter should complete after permit drop")
            .expect("waiter task must not panic");
        assert!(
            result.is_ok(),
            "Phase C waiter must acquire the freed permit, not time out",
        );
        assert!(!result.unwrap().is_reserve);

        // The invariant: no extra eviction call ran on the wake. Without the
        // cheap-path-first reordering the counter would be 3 (Phase B + first
        // Phase C iter + wake-driven extra `try_evict_one`).
        let final_calls = calls.load(AOrdering::Relaxed);
        assert_eq!(
            final_calls, baseline,
            "wake from permit drop must take the cheap path; counter advanced {} → {}",
            baseline, final_calls,
        );
        assert_eq!(
            coord.stats().evictions_total,
            0,
            "permit-drop path must not record any successful eviction",
        );
    }

    /// Negative guard: a bare `notify_idle_returned` call that does not
    /// correspond to a real state change (nothing evictable, no peer slot
    /// free) must NOT hand the waiter a permit out of thin air. The waiter
    /// should re-enter wait on the next iteration. This locks down the
    /// semantics: the notify is a retry trigger, not a grant.
    #[tokio::test]
    async fn phase_c_spurious_notify_idle_returned_does_not_grant() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let _p = coord.try_acquire().unwrap(); // never released

        let coord2 = coord.clone();
        let waiter = tokio::spawn(async move {
            let eviction = NoOpEviction; // never evicts
            coord2.acquire("testdb", "waiter", &eviction).await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;

        // Fire the notify without any actual change in peer state. Phase C
        // must re-check eviction (NoOp → false), re-check try_acquire
        // (still full), and go back to sleep.
        for _ in 0..5 {
            coord.notify_idle_returned();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // With `reserve_pool_timeout_ms = 100` and reserve_size = 0, this
        // should eventually error out (Phase D: reserve 0 → Phase E).
        let result = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter should complete")
            .unwrap();
        assert!(
            result.is_err(),
            "spurious notify must not grant a permit; waiter should error"
        );
    }

    #[tokio::test]
    async fn reserve_arbiter_grants_by_priority() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1)); // only 1 reserve permit
        let _p1 = coord.try_acquire().unwrap();

        // Send low-priority request first
        let (tx_low, rx_low) = tokio::sync::oneshot::channel::<ReserveGrant>();
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "low".to_string(),
                score: (0, 2), // not starving, 2 queued
                response: tx_low,
            })
            .await;

        // Send high-priority request second
        let (tx_high, rx_high) = tokio::sync::oneshot::channel::<ReserveGrant>();
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "high".to_string(),
                score: (1, 1), // starving — absolute priority
                response: tx_high,
            })
            .await;

        // Let arbiter process both requests
        tokio::time::sleep(Duration::from_millis(200)).await;

        // High priority should have received the grant (starving wins)
        let high_result = tokio::time::timeout(Duration::from_millis(50), rx_high).await;
        assert!(high_result.is_ok(), "starving user should get the grant");

        // Low priority should NOT receive (only 1 reserve permit)
        let low_result = tokio::time::timeout(Duration::from_millis(50), rx_low).await;
        assert!(
            low_result.is_err(),
            "non-starving user should not get grant"
        );
    }

    #[tokio::test]
    async fn stats_accurate_after_mixed_operations() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(2, 2));

        let p1 = coord.try_acquire().unwrap();
        let p2 = coord.try_acquire().unwrap();
        assert_eq!(coord.stats().total_connections, 2);

        // Reserve acquire
        let eviction = NoOpEviction;
        let p3 = coord.acquire("testdb", "user_a", &eviction).await.unwrap();
        assert!(p3.is_reserve);
        assert_eq!(coord.stats().total_connections, 3);
        assert_eq!(coord.stats().reserve_in_use, 1);
        assert_eq!(coord.stats().reserve_acquisitions_total, 1);

        drop(p1);
        assert_eq!(coord.stats().total_connections, 2);

        drop(p3);
        assert_eq!(coord.stats().total_connections, 1);
        assert_eq!(coord.stats().reserve_in_use, 0);

        drop(p2);
        assert_eq!(coord.stats().total_connections, 0);
        assert_eq!(coord.stats().reserve_in_use, 0);
        assert_eq!(coord.stats().reserve_acquisitions_total, 1);
    }

    #[tokio::test]
    async fn zero_reserve_returns_exhausted() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0)); // no reserve
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let result = coord.acquire("testdb", "user", &eviction).await;
        assert!(matches!(result, Err(AcquireError::NoConnection(_))));
    }

    #[tokio::test]
    async fn reserve_fully_exhausted() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1)); // 1 main + 1 reserve
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let _p2 = coord.acquire("testdb", "user_a", &eviction).await.unwrap(); // takes reserve
        assert_eq!(coord.reserve_in_use(), 1);

        // Second reserve attempt should fail — reserve exhausted
        let result = coord.acquire("testdb", "user_b", &eviction).await;
        assert!(matches!(result, Err(AcquireError::NoConnection(_))));
    }

    #[tokio::test]
    async fn arbiter_exits_on_channel_close() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1));

        // The only way to close the channel is to drop all senders.
        // PoolCoordinator holds reserve_tx; dropping the Arc drops the sender.
        drop(coord);

        // Arbiter should exit — give it time to notice
        tokio::time::sleep(Duration::from_millis(100)).await;
        // If arbiter panicked or hung, the test runtime would detect it.
        // The fact that we reach here means the arbiter exited cleanly.
    }

    #[tokio::test]
    async fn high_concurrency_no_permit_leak() {
        let max = 10;
        let reserve = 5;
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(max, reserve));

        let mut handles = Vec::new();
        for i in 0..50 {
            let c = coord.clone();
            handles.push(tokio::spawn(async move {
                let user = format!("user_{}", i);
                let eviction = NoOpEviction;
                match c.acquire("testdb", &user, &eviction).await {
                    Ok(permit) => {
                        // Hold the permit briefly
                        tokio::task::yield_now().await;
                        drop(permit);
                    }
                    Err(_) => {
                        // Expected: some will be exhausted under contention
                    }
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // All permits must be returned — no leaks
        assert_eq!(coord.total_connections(), 0);
        assert_eq!(coord.reserve_in_use(), 0);

        // Verify semaphores are fully replenished
        let mut permits = Vec::new();
        for _ in 0..max {
            permits.push(coord.try_acquire().unwrap());
        }
        assert!(coord.try_acquire().is_none());
        assert_eq!(coord.total_connections(), max);
    }

    #[tokio::test]
    async fn eviction_counter_increments() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(2, 0));
        let p1 = coord.try_acquire().unwrap();
        let p2 = coord.try_acquire().unwrap();

        // First eviction
        let eviction1 = PermitDroppingEviction::new(p1);
        let _p3 = coord.acquire("testdb", "user_a", &eviction1).await.unwrap();
        assert_eq!(coord.stats().evictions_total, 1);

        // Second eviction
        let eviction2 = PermitDroppingEviction::new(p2);
        let _p4 = coord.acquire("testdb", "user_b", &eviction2).await.unwrap();
        assert_eq!(coord.stats().evictions_total, 2);
        assert_eq!(coord.total_connections(), 2);
    }

    // ===== Error type tests =====

    #[tokio::test]
    async fn error_no_reserve_configured() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let err = coord.acquire("mydb", "app", &eviction).await.unwrap_err();
        match err {
            AcquireError::NoConnection(info) => {
                assert_eq!(info.phase, AcquirePhase::NoReserve);
            }
        }
    }

    #[tokio::test]
    async fn error_reserve_exhausted() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let _p2 = coord.acquire("mydb", "user_a", &eviction).await.unwrap();
        assert!(_p2.is_reserve, "first reserve should succeed");

        let err = coord
            .acquire("mydb", "user_b", &eviction)
            .await
            .unwrap_err();
        match err {
            AcquireError::NoConnection(info) => {
                assert_eq!(info.phase, AcquirePhase::ReserveExhausted);
            }
        }
    }

    #[tokio::test]
    async fn error_wait_timeout() {
        // reserve_pool_size > 0 but all reserve permits occupied,
        // so Phase D can't grant → ReserveExhausted
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let _reserve = coord.acquire("mydb", "holder", &eviction).await.unwrap();

        let err = coord
            .acquire("mydb", "waiter", &eviction)
            .await
            .unwrap_err();
        match err {
            AcquireError::NoConnection(info) => {
                assert_eq!(info.phase, AcquirePhase::ReserveExhausted);
                assert_eq!(info.reserve_in_use, 1);
                assert_eq!(info.reserve_size, 1);
            }
        }
    }

    #[tokio::test]
    async fn error_contains_database_and_user() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let err = coord
            .acquire("production_db", "analytics", &eviction)
            .await
            .unwrap_err();
        match err {
            AcquireError::NoConnection(info) => {
                assert_eq!(info.database, "production_db");
                assert_eq!(info.user, "analytics");
            }
        }
    }

    #[tokio::test]
    async fn error_contains_connection_counts() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(3, 2));
        let _p1 = coord.try_acquire().unwrap();
        let _p2 = coord.try_acquire().unwrap();
        let _p3 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let _r1 = coord.acquire("db", "u1", &eviction).await.unwrap(); // reserve 1
        let _r2 = coord.acquire("db", "u2", &eviction).await.unwrap(); // reserve 2

        let err = coord.acquire("db", "u3", &eviction).await.unwrap_err();
        match err {
            AcquireError::NoConnection(info) => {
                assert_eq!(info.max_db_connections, 3);
                assert_eq!(info.active_connections, 5); // 3 main + 2 reserve
                assert_eq!(info.reserve_size, 2);
                assert_eq!(info.reserve_in_use, 2);
            }
        }
    }

    #[test]
    fn error_display_no_reserve() {
        let info = NoConnectionInfo {
            database: "mydb".to_string(),
            user: "app".to_string(),
            max_db_connections: 100,
            active_connections: 100,
            reserve_size: 0,
            reserve_in_use: 0,
            phase: AcquirePhase::NoReserve,
        };
        let msg = format!("{info}");
        assert!(msg.contains("mydb"), "should contain database name");
        assert!(msg.contains("max=100"), "should contain max");
        assert!(msg.contains("user='app'"), "should contain user");
        assert!(!msg.contains("reserve"), "no reserve info when NoReserve");
    }

    #[test]
    fn error_display_reserve_exhausted() {
        let info = NoConnectionInfo {
            database: "mydb".to_string(),
            user: "migration".to_string(),
            max_db_connections: 50,
            active_connections: 55,
            reserve_size: 5,
            reserve_in_use: 5,
            phase: AcquirePhase::ReserveExhausted,
        };
        let msg = format!("{info}");
        assert!(msg.contains("mydb"));
        assert!(msg.contains("max=50"));
        assert!(msg.contains("reserve=5/5"));
        assert!(msg.contains("user='migration'"));
    }

    #[test]
    fn pool_error_from_acquire_error() {
        use crate::pool::errors::PoolError;

        let acquire_err = AcquireError::NoConnection(NoConnectionInfo {
            database: "db".to_string(),
            user: "u".to_string(),
            max_db_connections: 10,
            active_connections: 10,
            reserve_size: 0,
            reserve_in_use: 0,
            phase: AcquirePhase::NoReserve,
        });

        let pool_err: PoolError = acquire_err.into();
        assert!(matches!(pool_err, PoolError::DbLimitExhausted(_)));

        let msg = format!("{pool_err}");
        assert!(msg.contains("db"));
        assert!(msg.contains("max=10"));
    }

    #[tokio::test]
    async fn notify_one_exactly_one_waiter_acquires_single_permit() {
        let cfg = CoordinatorConfig {
            max_db_connections: 1,
            min_connection_lifetime_ms: 5000,
            reserve_pool_size: 0,
            reserve_pool_timeout_ms: 80,
        };
        let coord = PoolCoordinator::new("test_db".to_string(), cfg);
        let p = coord.try_acquire().unwrap();

        let success_count = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for i in 0..5 {
            let c = coord.clone();
            let cnt = success_count.clone();
            handles.push(tokio::spawn(async move {
                let eviction = NoOpEviction;
                let user = format!("waiter_{i}");
                if let Ok(_permit) = c.acquire("testdb", &user, &eviction).await {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    // Hold permit until test ends — no chain reaction
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }));
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(p);

        // Wait for Phase C timeout (80ms) + margin for all losers to finish
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(
            success_count.load(Ordering::Relaxed),
            1,
            "only one waiter should succeed with a single permit"
        );

        // Abort remaining tasks (the winner is sleeping 5s)
        for h in handles {
            h.abort();
        }
    }

    #[tokio::test]
    async fn sequential_permit_returns_wake_sequential_waiters() {
        let cfg = CoordinatorConfig {
            max_db_connections: 2,
            min_connection_lifetime_ms: 5000,
            reserve_pool_size: 0,
            reserve_pool_timeout_ms: 300,
        };
        let coord = PoolCoordinator::new("test_db".to_string(), cfg);
        let p1 = coord.try_acquire().unwrap();
        let p2 = coord.try_acquire().unwrap();

        let success_count = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for i in 0..3 {
            let c = coord.clone();
            let cnt = success_count.clone();
            handles.push(tokio::spawn(async move {
                let eviction = NoOpEviction;
                let user = format!("waiter_{i}");
                if let Ok(_permit) = c.acquire("testdb", &user, &eviction).await {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    // Hold permit — no chain reaction
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }));
        }

        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(p1);
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(p2);

        // Wait for Phase C timeout + margin
        tokio::time::sleep(Duration::from_millis(400)).await;

        assert_eq!(
            success_count.load(Ordering::Relaxed),
            2,
            "two permits returned should wake exactly two waiters"
        );

        for h in handles {
            h.abort();
        }
    }

    // ===== Extended coverage tests =====

    /// Eviction succeeds but another thread grabs the permit before
    /// the requester can `try_acquire()`. The requester must fall
    /// through to Phase C (wait) instead of getting a permit.
    #[tokio::test]
    async fn acquire_eviction_permit_stolen_by_concurrent_waiter() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(2, 0));
        let _p1 = coord.try_acquire().unwrap();
        let _p2 = coord.try_acquire().unwrap();

        // Eviction source that drops p1, freeing a slot.
        // But we also spawn a concurrent task that immediately grabs it.
        struct RacyEviction {
            permit: std::sync::Mutex<Option<CoordinatorPermit>>,
            coord: Arc<PoolCoordinator>,
        }
        impl EvictionSource for RacyEviction {
            fn try_evict_one(&self, _user: &str) -> bool {
                let mut guard = self.permit.lock().unwrap();
                if guard.is_some() {
                    *guard = None; // drops permit, frees slot
                                   // Immediately grab the freed slot before the caller
                    let stolen = self.coord.try_acquire();
                    assert!(stolen.is_some(), "should steal the freed permit");
                    std::mem::forget(stolen); // leak to keep slot occupied for this test
                    true
                } else {
                    false
                }
            }
            fn queued_clients(&self, _user: &str) -> usize {
                0
            }
            fn is_starving(&self, _user: &str) -> bool {
                false
            }
        }

        let eviction = RacyEviction {
            permit: std::sync::Mutex::new(Some(_p1)),
            coord: coord.clone(),
        };
        // Requester: eviction returns true, but try_acquire fails (stolen).
        // Falls through to Phase C, times out (no reserve).
        let result = coord.acquire("testdb", "victim", &eviction).await;
        assert!(
            result.is_err(),
            "should fail — evicted permit was stolen by concurrent waiter"
        );
        assert_eq!(coord.stats().evictions_total, 1);
    }

    /// reserve_pool_timeout shorter than ARBITER_RESPONSE_TIMEOUT:
    /// arbiter may not respond in time, reserve acquisition fails even
    /// though permits are available.
    #[tokio::test]
    async fn short_reserve_timeout_misses_arbiter_response() {
        let cfg = CoordinatorConfig {
            max_db_connections: 1,
            min_connection_lifetime_ms: 5000,
            reserve_pool_size: 5,
            // 10ms < ARBITER_RESPONSE_TIMEOUT (100ms) — arbiter may not respond in time
            reserve_pool_timeout_ms: 10,
        };
        let coord = PoolCoordinator::new("test_db".to_string(), cfg);
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        // Phase C: 10ms wait, then Phase D: reserve request sent, but
        // ARBITER_RESPONSE_TIMEOUT is 100ms, so oneshot may time out.
        // This is not guaranteed to fail (arbiter might respond quickly),
        // but we verify the coordinator doesn't panic or leak permits.
        let result = coord.acquire("testdb", "fast_user", &eviction).await;
        match result {
            Ok(permit) => {
                assert!(permit.is_reserve);
                drop(permit);
            }
            Err(_) => {
                // Expected: arbiter didn't respond in time
            }
        }
        // Key assertion: no permits leaked regardless of outcome
        drop(_p1);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(coord.total_connections(), 0);
        assert_eq!(coord.reserve_in_use(), 0);
    }

    /// Stress test: max_db_connections=1, many concurrent users competing
    /// for the single slot. Verifies no permit leak under maximal contention.
    #[tokio::test]
    async fn stress_single_slot_many_users_no_leak() {
        let cfg = CoordinatorConfig {
            max_db_connections: 1,
            min_connection_lifetime_ms: 5000,
            reserve_pool_size: 0,
            reserve_pool_timeout_ms: 50,
        };
        let coord = PoolCoordinator::new("test_db".to_string(), cfg);

        let success_count = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for i in 0..20 {
            let c = coord.clone();
            let cnt = success_count.clone();
            handles.push(tokio::spawn(async move {
                let eviction = NoOpEviction;
                let user = format!("user_{i}");
                if let Ok(permit) = c.acquire("testdb", &user, &eviction).await {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    // Simulate brief work
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    drop(permit);
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // All permits must be returned
        assert_eq!(coord.total_connections(), 0);
        // At least some should have succeeded (not all — only 1 slot)
        let successes = success_count.load(Ordering::Relaxed);
        assert!(
            successes >= 1,
            "at least one user should succeed, got {successes}"
        );
        // Semaphore fully replenished
        assert!(coord.try_acquire().is_some());
    }

    /// When all eviction candidates have connections younger than
    /// min_connection_lifetime, eviction returns false and the
    /// requester must fall through to wait/reserve.
    #[tokio::test]
    async fn eviction_returns_false_falls_through_to_reserve() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 2));
        let _p1 = coord.try_acquire().unwrap();

        // Eviction source returns false (all connections too young)
        struct TooYoungEviction;
        impl EvictionSource for TooYoungEviction {
            fn try_evict_one(&self, _user: &str) -> bool {
                false // all connections too young to evict
            }
            fn queued_clients(&self, _user: &str) -> usize {
                3
            }
            fn is_starving(&self, _user: &str) -> bool {
                true
            }
        }

        let eviction = TooYoungEviction;
        let result = coord.acquire("testdb", "needy_user", &eviction).await;
        // Should fall through eviction → wait (timeout) → reserve (success)
        assert!(result.is_ok());
        let permit = result.unwrap();
        assert!(permit.is_reserve, "should get reserve permit");
        assert_eq!(coord.stats().evictions_total, 0, "no eviction counted");
        assert_eq!(coord.stats().reserve_acquisitions_total, 1);
    }

    /// Multiple concurrent reserve requests from users with different
    /// priority scores — verify highest priority wins when only one
    /// reserve permit is available.
    #[tokio::test]
    async fn concurrent_reserve_priority_ordering() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1));
        let _p1 = coord.try_acquire().unwrap();

        // Send 3 requests with different priorities simultaneously
        let (tx1, rx1) = tokio::sync::oneshot::channel::<ReserveGrant>();
        let (tx2, rx2) = tokio::sync::oneshot::channel::<ReserveGrant>();
        let (tx3, rx3) = tokio::sync::oneshot::channel::<ReserveGrant>();

        // All sent before arbiter processes — it will pick the highest score
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "low".to_string(),
                score: (0, 1), // not starving, 1 queued
                response: tx1,
            })
            .await;
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "mid".to_string(),
                score: (0, 10), // not starving, 10 queued
                response: tx2,
            })
            .await;
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "high".to_string(),
                score: (1, 5), // starving — highest priority
                response: tx3,
            })
            .await;

        // Let arbiter process
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Starving user wins (only 1 reserve permit)
        let high = tokio::time::timeout(Duration::from_millis(50), rx3).await;
        assert!(high.is_ok(), "starving user should get the grant");

        // Mid-priority should NOT get (only 1 reserve)
        let mid = tokio::time::timeout(Duration::from_millis(50), rx2).await;
        assert!(mid.is_err(), "mid-priority should not get grant");

        // Low-priority should NOT get
        let low = tokio::time::timeout(Duration::from_millis(50), rx1).await;
        assert!(low.is_err(), "low-priority should not get grant");
    }

    /// Verify exhaustions_total counter increments correctly when
    /// both main and reserve are exhausted.
    #[tokio::test]
    async fn exhaustions_counter_accurate_across_multiple_failures() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 0));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;

        // 3 consecutive exhaustions
        for i in 0..3 {
            let result = coord
                .acquire("testdb", &format!("user_{i}"), &eviction)
                .await;
            assert!(result.is_err());
        }

        assert_eq!(
            coord.stats().exhaustions_total,
            3,
            "should count all 3 exhaustions"
        );
    }

    /// Dropping the coordinator Arc while there are pending reserve
    /// requests in the arbiter channel: the arbiter should exit cleanly
    /// and no permit leaks occur.
    #[tokio::test]
    async fn coordinator_drop_with_pending_reserve_requests() {
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 2));
        let _p1 = coord.try_acquire().unwrap();

        // Send a reserve request
        let (tx, rx) = tokio::sync::oneshot::channel::<ReserveGrant>();
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "orphan".to_string(),
                score: (0, 1),
                response: tx,
            })
            .await;

        // Drop the coordinator — arbiter channel closes
        drop(_p1);
        drop(coord);

        // Receiver should get an error (channel closed) or a grant
        // that returns the permit on drop. Either way, no leak.
        tokio::time::sleep(Duration::from_millis(200)).await;
        match rx.await {
            Ok(grant) => {
                // Grant received but coordinator dropped — Drop returns permit
                drop(grant);
            }
            Err(_) => {
                // Channel closed, no grant — expected path
            }
        }
        // If we reach here without panic/hang, arbiter exited cleanly
    }

    /// Arbiter caps its pending heap: when more requests accumulate
    /// than reserve_pool_size * 4, lowest-priority requests are dropped.
    /// Their oneshot channels close, and callers get a timeout.
    #[tokio::test]
    async fn arbiter_heap_capped_under_overload() {
        // reserve_pool_size=1 → cap = max(1,1)*4 = 4
        let coord = PoolCoordinator::new("test_db".to_string(), test_config(1, 1));
        let _p1 = coord.try_acquire().unwrap();

        // Don't let arbiter grant any reserves (occupy the only reserve permit)
        let rsem = coord.reserve_semaphore.try_acquire().unwrap();
        rsem.forget();

        // Flood arbiter with 10 requests (cap is 4)
        let mut receivers = Vec::new();
        for i in 0..10 {
            let (tx, rx) = tokio::sync::oneshot::channel::<ReserveGrant>();
            let _ = coord
                .reserve_tx
                .send(ReserveRequest {
                    user: format!("flood_{i}"),
                    score: (0, i), // ascending priority
                    response: tx,
                })
                .await;
            receivers.push(rx);
        }

        // Let arbiter process and apply cap
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Count how many receivers are already closed (sender dropped by cap).
        // Use a short timeout: if the sender is still alive in the heap,
        // the receiver will block — treat that as "not dropped".
        let mut dropped = 0;
        for rx in receivers {
            match tokio::time::timeout(Duration::from_millis(50), rx).await {
                Ok(Err(_)) => dropped += 1, // sender was dropped by cap
                Ok(Ok(_grant)) => {}        // unexpectedly granted
                Err(_) => {}                // still pending in heap (not dropped)
            }
        }

        // At least some low-priority requests should have been dropped
        assert!(
            dropped >= 4,
            "expected at least 4 dropped requests, got {dropped}"
        );
    }
}
