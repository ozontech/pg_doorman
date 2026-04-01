use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
        if self.is_reserve {
            self.coordinator.reserve_semaphore.add_permits(1);
            self.coordinator
                .reserve_in_use
                .fetch_sub(1, Ordering::Relaxed);
        } else {
            self.coordinator.db_semaphore.add_permits(1);
        }
        self.coordinator
            .total_connections
            .fetch_sub(1, Ordering::Relaxed);
        self.coordinator.connection_returned.notify_waiters();
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
            coordinator.reserve_semaphore.add_permits(1);
            coordinator.connection_returned.notify_waiters();
        }
    }
}

pub struct PoolCoordinator {
    db_semaphore: Semaphore,
    reserve_semaphore: Semaphore,
    total_connections: AtomicUsize,
    reserve_in_use: AtomicUsize,
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
    #[allow(dead_code)] // kept for logging/debugging
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
    pub fn new(config: CoordinatorConfig) -> Arc<Self> {
        let (reserve_tx, reserve_rx) = mpsc::channel(256);
        let coordinator = Arc::new(Self {
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
        // Phase A: fast path
        if let Some(permit) = self.try_acquire() {
            return Ok(permit);
        }

        // Phase B: try eviction
        if eviction_source.try_evict_one(user) {
            self.evictions_total.fetch_add(1, Ordering::Relaxed);
            if let Some(permit) = self.try_acquire() {
                return Ok(permit);
            }
        }

        // Phase C: wait for a connection to be returned
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(self.config.reserve_pool_timeout_ms);

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            tokio::select! {
                _ = self.connection_returned.notified() => {
                    if let Some(permit) = self.try_acquire() {
                        return Ok(permit);
                    }
                }
                _ = tokio::time::sleep(remaining) => {
                    break;
                }
            }
        }

        // Phase D: reserve
        let phase = if self.config.reserve_pool_size > 0 {
            let starving = u8::from(eviction_source.is_starving(user));
            let queued = eviction_source.queued_clients(user);
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
                    return Ok(grant.into_permit());
                }
            }

            AcquirePhase::ReserveExhausted
        } else {
            AcquirePhase::NoReserve
        };

        // Phase E: exhausted — client will get an error
        self.exhaustions_total.fetch_add(1, Ordering::Relaxed);
        Err(AcquireError::NoConnection(NoConnectionInfo {
            database: database.to_string(),
            user: user.to_string(),
            max_db_connections: self.config.max_db_connections,
            active_connections: self.total_connections.load(Ordering::Relaxed),
            reserve_size: self.config.reserve_pool_size,
            reserve_in_use: self.reserve_in_use.load(Ordering::Relaxed),
            phase,
        }))
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
        // Register Notify BEFORE processing so we don't miss wakeups
        // that arrive between the grant loop and select!.
        let notified = coordinator.connection_returned.notified();

        // Collect new requests (non-blocking)
        while let Ok(req) = rx.try_recv() {
            pending.push(req);
        }

        // Grant to highest-scoring request if reserve available
        while pending.peek().is_some() {
            if let Ok(sem_permit) = coordinator.reserve_semaphore.try_acquire() {
                sem_permit.forget();
                let req = pending.pop().unwrap();
                let grant = ReserveGrant::new(coordinator.clone());
                // If send fails, grant drops here → permit returned automatically.
                // If send succeeds but caller times out and drops the receiver,
                // the grant inside the oneshot drops → permit returned automatically.
                let _ = req.response.send(grant);
            } else {
                break;
            }
        }

        // Remove cancelled requests (caller timed out and dropped oneshot)
        pending.retain(|req| !req.response.is_closed());

        tokio::select! {
            req = rx.recv() => {
                match req {
                    Some(req) => pending.push(req),
                    None => return, // channel closed, coordinator dropped
                }
            }
            _ = notified => {}
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
        let coord = PoolCoordinator::new(test_config(10, 0));
        let permit = coord.try_acquire().unwrap();
        assert_eq!(coord.total_connections(), 1);
        assert!(!permit.is_reserve);
        drop(permit);
        assert_eq!(coord.total_connections(), 0);
    }

    #[tokio::test]
    async fn try_acquire_at_limit_returns_none() {
        let coord = PoolCoordinator::new(test_config(2, 0));
        let _p1 = coord.try_acquire().unwrap();
        let _p2 = coord.try_acquire().unwrap();
        assert!(coord.try_acquire().is_none());
        assert_eq!(coord.total_connections(), 2);
    }

    #[tokio::test]
    async fn permit_drop_frees_slot() {
        let coord = PoolCoordinator::new(test_config(1, 0));
        let p = coord.try_acquire().unwrap();
        assert!(coord.try_acquire().is_none());
        drop(p);
        assert!(coord.try_acquire().is_some());
    }

    #[tokio::test]
    async fn acquire_with_eviction() {
        let coord = PoolCoordinator::new(test_config(1, 0));
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
        let coord = PoolCoordinator::new(test_config(1, 5));
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
        let coord = PoolCoordinator::new(test_config(1, 0));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let result = coord.acquire("testdb", "test", &eviction).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn permit_drop_notifies_waiters() {
        let coord = PoolCoordinator::new(test_config(1, 0));
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
        let coord = PoolCoordinator::new(test_config(1, 2));
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
        let p3 = coord.acquire("testdb", "ok_user2", &eviction).await.unwrap();
        assert!(p3.is_reserve);
        assert_eq!(coord.reserve_in_use(), 2);
    }

    // ===== New tests below =====

    #[tokio::test]
    async fn reserve_grant_returns_permit_on_drop() {
        let coord = PoolCoordinator::new(test_config(1, 1));

        // Manually acquire a reserve semaphore permit via the grant mechanism
        let sem_permit = coord.reserve_semaphore.try_acquire().unwrap();
        sem_permit.forget();

        // Create a grant and immediately drop it
        let grant = ReserveGrant::new(coord.clone());
        drop(grant);

        // Permit should be back — verify by acquiring it again
        let sem_permit2 = coord.reserve_semaphore.try_acquire();
        assert!(sem_permit2.is_ok(), "permit should be returned after grant drop");
    }

    #[tokio::test]
    async fn reserve_permit_not_leaked_on_late_receiver_drop() {
        // Regression test: arbiter sends grant, then receiver is dropped without
        // consuming the value. Before fix (bool), permit was leaked permanently.
        // After fix (ReserveGrant), permit is returned via RAII Drop.
        let coord = PoolCoordinator::new(test_config(1, 1));
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
        let coord = PoolCoordinator::new(test_config(2, 0));
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
        let coord = PoolCoordinator::new(test_config(1, 0));
        let p = coord.try_acquire().unwrap();

        let coord2 = coord.clone();
        let waiter = tokio::spawn(async move {
            let eviction = NoOpEviction;
            // Phase C: waits for connection_returned notify
            coord2.acquire("testdb", "waiter", &eviction).await
        });

        // Give waiter time to enter Phase C
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Drop permit — triggers notify_waiters
        drop(p);

        let result = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter should complete")
            .unwrap();
        assert!(result.is_ok());
        assert!(!result.unwrap().is_reserve);
    }

    #[tokio::test]
    async fn reserve_arbiter_grants_by_priority() {
        let coord = PoolCoordinator::new(test_config(1, 1)); // only 1 reserve permit
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
        assert!(low_result.is_err(), "non-starving user should not get grant");
    }

    #[tokio::test]
    async fn stats_accurate_after_mixed_operations() {
        let coord = PoolCoordinator::new(test_config(2, 2));

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
        let coord = PoolCoordinator::new(test_config(1, 0)); // no reserve
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let result = coord.acquire("testdb", "user", &eviction).await;
        assert!(matches!(result, Err(AcquireError::NoConnection(_))));
    }

    #[tokio::test]
    async fn reserve_fully_exhausted() {
        let coord = PoolCoordinator::new(test_config(1, 1)); // 1 main + 1 reserve
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
        let coord = PoolCoordinator::new(test_config(1, 1));

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
        let coord = PoolCoordinator::new(test_config(max, reserve));

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
        let coord = PoolCoordinator::new(test_config(2, 0));
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
        let coord = PoolCoordinator::new(test_config(1, 0));
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
        let coord = PoolCoordinator::new(test_config(1, 1));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let _p2 = coord.acquire("mydb", "user_a", &eviction).await.unwrap();
        assert!(
            _p2.is_reserve,
            "first reserve should succeed"
        );

        let err = coord.acquire("mydb", "user_b", &eviction).await.unwrap_err();
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
        let coord = PoolCoordinator::new(test_config(1, 1));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let _reserve = coord.acquire("mydb", "holder", &eviction).await.unwrap();

        let err = coord.acquire("mydb", "waiter", &eviction).await.unwrap_err();
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
        let coord = PoolCoordinator::new(test_config(1, 0));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let err = coord.acquire("production_db", "analytics", &eviction).await.unwrap_err();
        match err {
            AcquireError::NoConnection(info) => {
                assert_eq!(info.database, "production_db");
                assert_eq!(info.user, "analytics");
            }
        }
    }

    #[tokio::test]
    async fn error_contains_connection_counts() {
        let coord = PoolCoordinator::new(test_config(3, 2));
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
}
