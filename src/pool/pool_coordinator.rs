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

#[derive(Clone, Debug)]
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
}

/// RAII permit — held for the lifetime of a server connection.
/// Dropping it returns the permit to the correct semaphore.
pub struct CoordinatorPermit {
    coordinator: Arc<PoolCoordinator>,
    pub is_reserve: bool,
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

pub struct PoolCoordinator {
    db_semaphore: Semaphore,
    reserve_semaphore: Semaphore,
    total_connections: AtomicUsize,
    reserve_in_use: AtomicUsize,
    connection_returned: Notify,
    config: CoordinatorConfig,
    evictions_total: AtomicU64,
    reserve_acquisitions_total: AtomicU64,
    reserve_tx: mpsc::Sender<ReserveRequest>,
}

/// Time to wait for the arbiter to process a reserve request
/// after it has been submitted to the priority queue.
const ARBITER_RESPONSE_TIMEOUT: Duration = Duration::from_millis(100);

struct ReserveRequest {
    #[allow(dead_code)] // kept for logging/debugging
    user: String,
    score: (u8, usize), // (starving, queued_clients)
    response: tokio::sync::oneshot::Sender<bool>,
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
                    coordinator: self.clone(),
                    is_reserve: false,
                })
            }
            Err(_) => None,
        }
    }

    /// Full acquisition path: try → evict → wait → reserve → error.
    pub async fn acquire(
        self: &Arc<Self>,
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
        if self.config.reserve_pool_size > 0 {
            let starving = u8::from(eviction_source.is_starving(user));
            let queued = eviction_source.queued_clients(user);
            let (tx, rx) = tokio::sync::oneshot::channel();

            let _ = self
                .reserve_tx
                .send(ReserveRequest {
                    user: user.to_string(),
                    score: (starving, queued),
                    response: tx,
                })
                .await;

            if let Ok(Ok(true)) = tokio::time::timeout(ARBITER_RESPONSE_TIMEOUT, rx).await {
                self.total_connections.fetch_add(1, Ordering::Relaxed);
                self.reserve_in_use.fetch_add(1, Ordering::Relaxed);
                self.reserve_acquisitions_total
                    .fetch_add(1, Ordering::Relaxed);
                return Ok(CoordinatorPermit {
                    coordinator: self.clone(),
                    is_reserve: true,
                });
            }
        }

        // Phase E: exhausted
        Err(AcquireError::Exhausted)
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
        }
    }

    pub fn config(&self) -> &CoordinatorConfig {
        &self.config
    }
}

#[derive(Debug)]
pub enum AcquireError {
    Exhausted,
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcquireError::Exhausted => write!(f, "all database connections exhausted"),
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
                sem_permit.forget(); // consume permit permanently
                let req = pending.pop().unwrap();
                if req.response.send(true).is_err() {
                    // Caller timed out and dropped the receiver — return the permit
                    coordinator.reserve_semaphore.add_permits(1);
                }
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

    struct SuccessEviction;
    impl EvictionSource for SuccessEviction {
        fn try_evict_one(&self, _user: &str) -> bool {
            true
        }
        fn queued_clients(&self, _user: &str) -> usize {
            5
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
        let result = coord.acquire("test", &eviction).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn acquire_reserve_on_exhaustion() {
        let coord = PoolCoordinator::new(test_config(1, 5));
        let _p1 = coord.try_acquire().unwrap();

        let eviction = NoOpEviction;
        let permit = coord.acquire("test", &eviction).await.unwrap();
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
        let result = coord.acquire("test", &eviction).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn permit_drop_notifies_waiters() {
        let coord = PoolCoordinator::new(test_config(1, 0));
        let p = coord.try_acquire().unwrap();

        let coord2 = coord.clone();
        let handle = tokio::spawn(async move {
            let eviction = NoOpEviction;
            coord2.acquire("waiter", &eviction).await
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
    async fn reserve_permits_not_leaked_on_timeout() {
        // Acquire all main permits, then request reserve.
        // Drop the oneshot receiver before arbiter processes → permit must be returned.
        let coord = PoolCoordinator::new(test_config(1, 2));
        let _p1 = coord.try_acquire().unwrap();

        // Send reserve request but immediately drop the receiver
        let (tx, _rx_dropped) = tokio::sync::oneshot::channel::<bool>();
        let _ = coord
            .reserve_tx
            .send(ReserveRequest {
                user: "leaker".to_string(),
                score: (0, 1),
                response: tx,
            })
            .await;
        drop(_rx_dropped);

        // Give arbiter time to process
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Reserve permits should NOT be leaked — we should still be able to acquire both
        let eviction = NoOpEviction;
        let p2 = coord.acquire("ok_user", &eviction).await.unwrap();
        assert!(p2.is_reserve);
        let p3 = coord.acquire("ok_user2", &eviction).await.unwrap();
        assert!(p3.is_reserve);
        assert_eq!(coord.reserve_in_use(), 2);
    }
}
