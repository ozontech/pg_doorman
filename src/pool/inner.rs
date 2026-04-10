use std::{
    collections::VecDeque,
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Weak,
    },
    time::Duration,
};

use log::debug;
use rand::Rng as _;

use crate::utils::clock;

use parking_lot::Mutex;

use tokio::sync::{oneshot, Notify, Semaphore, SemaphorePermit, TryAcquireError};

use super::errors::{PoolError, RecycleError, TimeoutType};
use super::pool_coordinator;
use super::types::{Metrics, PoolConfig, QueueMode, Status, Timeouts};
use super::ServerPool;
use crate::server::Server;

const MAX_FAST_RETRY: i32 = 10;

/// Fallback wake interval for tasks queued behind the bounded burst limiter.
/// Used as a safety net in case neither a direct-handoff delivery nor
/// `create_done` fires within the expected window — guarantees forward
/// progress without busy-spinning.
const BURST_BACKOFF: std::time::Duration = std::time::Duration::from_millis(5);

/// Internal object wrapper with metrics.
/// The `coordinator_permit` is held for the entire lifetime of the connection:
/// - Acquired when a NEW connection is created (timeout_get / replenish)
/// - Stays with the ObjectInner when returned to the idle pool (VecDeque)
/// - Dropped when the connection is destroyed → frees coordinator semaphore slot
/// - `None` when coordination is disabled (max_db_connections = 0)
#[derive(Debug)]
struct ObjectInner {
    obj: Server,
    metrics: Metrics,
    /// Held for RAII — dropped when connection is destroyed, freeing coordinator slot.
    #[allow(dead_code)]
    coordinator_permit: Option<pool_coordinator::CoordinatorPermit>,
}

/// Wrapper around the actual pooled object which implements Deref and DerefMut.
/// When dropped, the object is returned to the pool.
pub struct Object {
    inner: Option<ObjectInner>,
    pool: Weak<PoolInner>,
}

impl Drop for Object {
    fn drop(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            if let Some(pool) = self.pool.upgrade() {
                inner.metrics.recycled = Some(clock::now());
                inner.metrics.recycle_count += 1;
                pool.return_object(inner);
            }
        }
    }
}

impl Deref for Object {
    type Target = Server;
    fn deref(&self) -> &Self::Target {
        &self.inner.as_ref().unwrap().obj
    }
}

impl DerefMut for Object {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.as_mut().unwrap().obj
    }
}

impl AsRef<Server> for Object {
    fn as_ref(&self) -> &Server {
        self
    }
}

impl AsMut<Server> for Object {
    fn as_mut(&mut self) -> &mut Server {
        self
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Object")
            .field("inner", &self.inner.as_ref().map(|i| &i.obj))
            .finish()
    }
}

/// Internal slots storage.
struct Slots {
    vec: VecDeque<ObjectInner>,
    /// Direct-handoff queue: waiters blocked on a oneshot receiver.
    /// `return_object` pops the oldest sender and delivers the connection
    /// directly, bypassing the idle VecDeque entirely.
    waiters: VecDeque<oneshot::Sender<ObjectInner>>,
    size: usize,
    max_size: usize,
}

impl fmt::Debug for Slots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slots")
            .field("vec_len", &self.vec.len())
            .field("waiters_len", &self.waiters.len())
            .field("size", &self.size)
            .field("max_size", &self.max_size)
            .finish()
    }
}

/// Per-pool counters for the anticipation + bounded burst code path.
///
/// All fields are monotonic counters. They are read by the admin/prometheus
/// exporters and never reset; relative deltas between scrapes are what
/// operators tune against.
#[derive(Debug, Default)]
pub(crate) struct ScalingStats {
    /// Number of new connections that successfully took a burst slot and
    /// proceeded to `server_pool.create()`. Pairs with `burst_gate_waits`
    /// to compute the gate hit rate.
    pub(crate) creates_started: AtomicU64,
    /// Number of times a caller observed the burst gate at capacity and had
    /// to wait on a Notify (or backoff). High values indicate `max_parallel_creates`
    /// is too low for the offered load — or that creates are slow.
    pub(crate) burst_gate_waits: AtomicU64,
    /// Number of Phase B anticipation attempts where a direct-handoff
    /// delivery via oneshot channel succeeded. Incremented once per
    /// successful receive, before the recycle check.
    pub(crate) anticipation_wakes_notify: AtomicU64,
    /// Number of Phase 4 fall-throughs that gave up on anticipation:
    /// the deadline was exhausted, the per-caller race-loss cap was
    /// hit, or the wall-clock hard cap fired. Increments exactly once
    /// per Phase 4 exit without a recyclable connection.
    pub(crate) anticipation_wakes_timeout: AtomicU64,
    /// Number of times Phase 4 fell through without a recyclable connection
    /// and the caller had to call `server_pool.create()`. Steady-state
    /// should be near zero; a sustained non-zero rate means offered load
    /// exceeds what returns can serve within the caller's remaining wait
    /// budget (`query_wait_timeout` - 500 ms create reserve).
    pub(crate) create_fallback: AtomicU64,
    /// Number of times the background `replenish` task hit the burst cap
    /// and deferred its work to the next retain cycle. Persistent non-zero
    /// values indicate `min_pool_size` cannot be sustained under current load.
    pub(crate) replenish_deferred: AtomicU64,
    /// Number of pre-replacement connections created ahead of lifetime expiry.
    pub(crate) pre_replacements_triggered: AtomicU64,
    /// Number of pre-replacement attempts skipped (coordinator full, pressure,
    /// pool not tight, or another pre-replacement already in flight).
    pub(crate) pre_replacements_skipped: AtomicU64,
}

/// Snapshot of per-pool scaling counters, returned to admin/prometheus exporters.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScalingStatsSnapshot {
    pub creates_started: u64,
    pub burst_gate_waits: u64,
    pub anticipation_wakes_notify: u64,
    pub anticipation_wakes_timeout: u64,
    pub create_fallback: u64,
    pub replenish_deferred: u64,
    /// Current `inflight_creates` value (gauge, not a counter).
    pub inflight_creates: usize,
    pub pre_replacements_triggered: u64,
    pub pre_replacements_skipped: u64,
}

/// Internal pool state.
struct PoolInner {
    server_pool: ServerPool,
    slots: Mutex<Slots>,
    /// Number of users currently holding or waiting for objects.
    users: AtomicUsize,
    semaphore: Semaphore,
    config: PoolConfig,
    /// Database-level coordinator (None when max_db_connections = 0).
    coordinator: Option<Arc<pool_coordinator::PoolCoordinator>>,
    /// Pool name (database name in config), used in coordinator error messages.
    pool_name: String,
    /// Username for this pool, used in coordinator error messages.
    username: String,
    /// Number of server connection creates currently in-flight on this pool.
    /// This is NOT the count of currently-held connections — only those being
    /// established right now via `server_pool.create()`. Bounded by
    /// `config.scaling.max_parallel_creates` to suppress thundering herd when
    /// N parallel callers all miss the idle pool simultaneously.
    inflight_creates: AtomicUsize,
    /// Wake signal for tasks queued behind the bounded burst limiter.
    /// Notified once when an in-flight create completes (success or failure),
    /// so the next waiting task can attempt its own create or recycle.
    create_done: Notify,
    /// Counters exposed via SHOW POOLS and Prometheus for tuning the
    /// anticipation + bounded burst path.
    scaling_stats: ScalingStats,
    /// Number of pre-replacement tasks currently in flight. Capped at
    /// `MAX_CONCURRENT_PRE_REPLACEMENTS` to prevent a burst of expiring
    /// connections from spawning too many background creates at once.
    pre_replacements_in_flight: AtomicUsize,
}

enum RecycleOutcome {
    Reused(Box<ObjectInner>),
    Failed,
    Empty,
}

/// Minimum `server_lifetime` for pre-replacement to be worthwhile.
/// With shorter lifetimes the overlap window is too narrow for the
/// replacement to be ready in time.
const PRE_REPLACE_MIN_LIFETIME_MS: u64 = 60_000;

/// Pre-replacement threshold as a percentage of `metrics.lifetime_ms`.
/// At 95% of a 5-minute lifetime the overlap window is ~15 seconds —
/// 15 000x the ~1 ms Unix-socket connect time. For TCP deployments this
/// can be lowered to 85%.
const PRE_REPLACE_THRESHOLD_PCT: u64 = 95;

/// Maximum concurrent pre-replacement tasks per pool. With a 5-minute
/// lifetime and ±20% jitter, up to 3 connections can expire within
/// the same 15-second window. Allowing 3 concurrent pre-replacements
/// ensures each one gets a warm replacement without serialization.
const MAX_CONCURRENT_PRE_REPLACEMENTS: usize = 3;

/// Push a connection into the idle queue respecting the configured
/// queue mode (FIFO/LIFO). Caller must hold the slots lock.
#[inline(always)]
fn push_idle(queue_mode: QueueMode, vec: &mut VecDeque<ObjectInner>, inner: ObjectInner) {
    match queue_mode {
        QueueMode::Fifo => vec.push_back(inner),
        QueueMode::Lifo => vec.push_front(inner),
    }
}

impl PoolInner {
    /// Try to take a burst gate slot. On success, bumps `creates_started`
    /// and returns a guard that releases the slot on drop.
    fn try_acquire_burst_gate(&self) -> Option<BurstGateGuard<'_>> {
        let max = self.config.scaling.max_parallel_creates as usize;
        if try_take_burst_slot(&self.inflight_creates, max) {
            self.scaling_stats
                .creates_started
                .fetch_add(1, Ordering::Relaxed);
            Some(BurstGateGuard {
                inflight_creates: &self.inflight_creates,
                create_done: &self.create_done,
            })
        } else {
            None
        }
    }

    /// Build an ObjectInner from a freshly created Server connection,
    /// stamped with the current server_pool epoch and jittered timeouts.
    fn new_object_inner(
        &self,
        obj: Server,
        coordinator_permit: Option<pool_coordinator::CoordinatorPermit>,
    ) -> ObjectInner {
        ObjectInner {
            obj,
            metrics: Metrics::new(
                self.server_pool.lifetime_ms(),
                self.server_pool.idle_timeout_ms(),
                self.server_pool.current_epoch(),
            ),
            coordinator_permit,
        }
    }

    /// Background pre-replacement: create one connection ahead of lifetime
    /// expiry so the next checkout finds a warm replacement in the idle
    /// queue instead of paying for a fresh create.
    ///
    /// Called via `tokio::spawn` from `Pool::trigger_pre_replacement`.
    /// On success the pool temporarily holds `max_size + 1` connections
    /// until the old one dies during the next recycle.
    async fn pre_replace_one(&self) {
        // Coordinator permit — non-blocking, with headroom guard.
        let coordinator_permit = if let Some(ref coord) = self.coordinator {
            // Keep at least 2 permits free so a peer pool can still create
            // without being forced onto the slow eviction/reserve path.
            if coord.available_main_permits() < 2 {
                log::debug!(
                    "[{}@{}] pre-replace: skipped — coordinator headroom < 2",
                    self.username,
                    self.pool_name,
                );
                self.scaling_stats
                    .pre_replacements_skipped
                    .fetch_add(1, Ordering::Relaxed);
                return;
            }
            match coord.try_acquire() {
                Some(p) => Some(p),
                None => {
                    log::debug!(
                        "[{}@{}] pre-replace: skipped — coordinator full",
                        self.username,
                        self.pool_name,
                    );
                    self.scaling_stats
                        .pre_replacements_skipped
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        } else {
            None
        };

        // Burst gate — non-blocking, like replenish.
        let Some(_gate) = self.try_acquire_burst_gate() else {
            log::debug!(
                "[{}@{}] pre-replace: skipped — burst gate full",
                self.username,
                self.pool_name,
            );
            self.scaling_stats
                .pre_replacements_skipped
                .fetch_add(1, Ordering::Relaxed);
            return;
        };

        // Create the replacement connection.
        let obj = match self.server_pool.create().await {
            Ok(obj) => obj,
            Err(e) => {
                log::debug!(
                    "[{}@{}] pre-replace: create failed — {}",
                    self.username,
                    self.pool_name,
                    e,
                );
                self.scaling_stats
                    .pre_replacements_skipped
                    .fetch_add(1, Ordering::Relaxed);
                return;
            }
        };

        let inner = self.new_object_inner(obj, coordinator_permit);

        // Push to idle queue. Temporarily exceeds max_size by 1; returns
        // to max_size when the old connection fails recycle.
        {
            let mut slots = self.slots.lock();
            slots.size += 1;
            push_idle(self.config.queue_mode, &mut slots.vec, inner);
        }

        // Add a semaphore permit so a client can reach the pre-created
        // connection through the normal checkout path. The extra permit
        // is consumed when the old connection fails recycle: the caller
        // holds it and retries try_recycle_one which finds the replacement.
        self.semaphore.add_permits(1);

        self.scaling_stats
            .pre_replacements_triggered
            .fetch_add(1, Ordering::Relaxed);
        log::info!(
            "[{}@{}] pre-replace: replacement connection created ahead of lifetime expiry",
            self.username,
            self.pool_name,
        );
    }

    /// Create a new backend connection via `server_pool.create()`, respecting
    /// the caller's `create` timeout. On success, increments `slots.size` and
    /// returns the `ObjectInner` ready for wrapping into an `Object`.
    async fn create_connection(
        &self,
        timeouts: &Timeouts,
        coordinator_permit: Option<pool_coordinator::CoordinatorPermit>,
    ) -> Result<ObjectInner, PoolError> {
        let obj = match timeouts.create {
            Some(duration) => {
                match tokio::time::timeout(duration, self.server_pool.create()).await {
                    Ok(Ok(obj)) => obj,
                    Ok(Err(e)) => return Err(PoolError::Backend(e)),
                    Err(_) => return Err(PoolError::Timeout(TimeoutType::Create)),
                }
            }
            None => self
                .server_pool
                .create()
                .await
                .map_err(PoolError::Backend)?,
        };

        {
            let mut slots = self.slots.lock();
            slots.size += 1;
        }

        Ok(self.new_object_inner(obj, coordinator_permit))
    }

    /// Returns true when every permit is in use — clients are either holding
    /// connections or queued behind the semaphore. Used to suppress lifetime
    /// housekeeping (`recycle` lifetime expiry, retain-loop trimming) so we
    /// do not close working connections at the moment they are most needed.
    /// One atomic load on the semaphore — safe to call from the hot path.
    #[inline(always)]
    fn under_pressure(&self) -> bool {
        self.semaphore.available_permits() == 0
    }

    async fn try_recycle_one(&self, timeouts: &Timeouts) -> RecycleOutcome {
        let obj_inner = {
            let mut slots = self.slots.lock();
            slots.vec.pop_front()
        };

        let Some(mut inner) = obj_inner else {
            return RecycleOutcome::Empty;
        };

        let skip_lifetime = self.under_pressure();

        let recycle_result = match timeouts.recycle {
            Some(duration) => {
                match tokio::time::timeout(
                    duration,
                    self.server_pool
                        .recycle(&mut inner.obj, &inner.metrics, skip_lifetime),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(RecycleError::StaticMessage("Recycle timeout")),
                }
            }
            None => {
                self.server_pool
                    .recycle(&mut inner.obj, &inner.metrics, skip_lifetime)
                    .await
            }
        };

        match recycle_result {
            Ok(()) => RecycleOutcome::Reused(Box::new(inner)),
            Err(_) => {
                let mut slots = self.slots.lock();
                slots.size = slots.size.saturating_sub(1);
                RecycleOutcome::Failed
            }
        }
    }

    #[inline(always)]
    fn return_object(&self, mut inner: ObjectInner) {
        let mut slots = self.slots.lock();

        // Direct handoff: send to the oldest registered waiter.
        // Waiters whose receiver was dropped (timeout) are skipped.
        while let Some(sender) = slots.waiters.pop_front() {
            match sender.send(inner) {
                Ok(()) => {
                    drop(slots);
                    // No semaphore.add_permits — the waiter already holds
                    // a permit. No coordinator notify — the connection was
                    // handed off, not placed in the idle queue, so a peer
                    // eviction scan would find nothing.
                    return;
                }
                Err(returned_inner) => {
                    // Receiver dropped (timeout) — try the next waiter.
                    inner = returned_inner;
                }
            }
        }

        // No waiters — normal path.
        push_idle(self.config.queue_mode, &mut slots.vec, inner);
        drop(slots);
        self.semaphore.add_permits(1);
        self.notify_return_observers();
    }

    /// Wake peer-pool coordinator waiter after a connection lands in
    /// `slots.vec` (the no-waiter path of `return_object`). The coordinator
    /// Phase C waiter scans this pool's idle vec via `evict_one_idle` and
    /// drops the returned connection to free a coordinator slot.
    ///
    /// Same-pool waiters (Phase B anticipation, burst gate) now receive
    /// connections via the direct-handoff oneshot channel inside
    /// `return_object` and never park on a Notify.
    #[inline(always)]
    fn notify_return_observers(&self) {
        if let Some(coordinator) = self.coordinator.as_ref() {
            coordinator.notify_idle_returned();
        }
    }
}

impl fmt::Debug for PoolInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let slots = self.slots.lock();
        f.debug_struct("PoolInner")
            .field("server_pool", &self.server_pool)
            .field("slots_size", &slots.size)
            .field("slots_max_size", &slots.max_size)
            .field("users", &self.users)
            .field("config", &self.config)
            .finish()
    }
}

/// Connection pool for PostgreSQL server connections.
///
/// This struct can be cloned and transferred across thread boundaries and uses
/// reference counting for its internal state.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<PoolInner>,
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool").field("inner", &self.inner).finish()
    }
}

/// Outcome of the burst gate acquisition loop.
enum BurstGateOutcome<'a> {
    /// Slot acquired — caller proceeds to create a connection.
    Acquired(BurstGateGuard<'a>),
    /// A recycled connection was obtained while waiting for a slot.
    Recycled(Box<ObjectInner>),
    /// Non-blocking caller and gate is full — no connection available.
    Timeout,
}

/// Outcome of JIT coordinator permit acquisition.
enum CoordinatorJitResult<'a> {
    /// Permit acquired (or no coordinator configured) — caller creates.
    /// The gate guard is returned so the caller holds it until create
    /// completes.
    Create {
        permit: Option<pool_coordinator::CoordinatorPermit>,
        gate: BurstGateGuard<'a>,
    },
    /// A recycled connection was found during the slow-path wait.
    Recycled(Box<ObjectInner>),
}

impl Pool {
    /// Wrap a recycled/created ObjectInner into an Object, consuming
    /// the semaphore permit so it stays with the connection until
    /// Object::drop returns it.
    #[inline(always)]
    fn wrap_checkout(&self, inner: ObjectInner, permit: SemaphorePermit<'_>) -> Object {
        permit.forget();
        Object {
            inner: Some(inner),
            pool: Arc::downgrade(&self.inner),
        }
    }

    /// Acquire a burst gate slot, waiting if necessary. While waiting,
    /// attempts to recycle idle connections and registers as a
    /// direct-handoff waiter so a returning connection can be delivered
    /// without entering the idle queue.
    async fn acquire_burst_gate(
        &self,
        timeouts: &Timeouts,
        non_blocking: bool,
    ) -> BurstGateOutcome<'_> {
        loop {
            if let Some(guard) = self.inner.try_acquire_burst_gate() {
                return BurstGateOutcome::Acquired(guard);
            }

            self.inner
                .scaling_stats
                .burst_gate_waits
                .fetch_add(1, Ordering::Relaxed);

            if non_blocking {
                if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                    return BurstGateOutcome::Recycled(inner);
                }
                return BurstGateOutcome::Timeout;
            }

            // Try recycle BEFORE registering as a waiter to avoid
            // leaving dead senders in the queue on success.
            if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                return BurstGateOutcome::Recycled(inner);
            }

            // Register a direct-handoff waiter AND listen on create_done.
            let (tx, rx) = oneshot::channel();
            self.inner.slots.lock().waiters.push_back(tx);
            let on_create = self.inner.create_done.notified();

            tokio::select! {
                result = rx => {
                    if let Ok(inner) = result {
                        if let Ok(inner) = self.recycle_handoff(inner, timeouts).await {
                            return BurstGateOutcome::Recycled(Box::new(inner));
                        }
                    }
                }
                _ = on_create => {}
                _ = tokio::time::sleep(BURST_BACKOFF) => {}
            }

            // After wake — try recycle once before retrying the gate.
            if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                return BurstGateOutcome::Recycled(inner);
            }
        }
    }

    /// JIT coordinator permit acquisition. Takes the burst gate guard
    /// by value — on the slow path the gate is released while waiting
    /// on the coordinator, then re-acquired.
    ///
    /// Returns either a permit + gate (caller proceeds to create) or
    /// a recycled connection found during the slow-path wait.
    async fn acquire_coordinator_jit<'a>(
        &'a self,
        timeouts: &Timeouts,
        gate: BurstGateGuard<'a>,
    ) -> Result<CoordinatorJitResult<'a>, PoolError> {
        let Some(ref coordinator) = self.inner.coordinator else {
            return Ok(CoordinatorJitResult::Create { permit: None, gate });
        };

        // Fast path: non-blocking CAS.
        if let Some(p) = coordinator.try_acquire() {
            debug!(
                "[{}@{}] coordinator: permit via fast JIT path \
                 (permit_type=main)",
                self.inner.username, self.inner.pool_name,
            );
            return Ok(CoordinatorJitResult::Create {
                permit: Some(p),
                gate,
            });
        }

        // Slow path: release gate slot so peers can create while we wait.
        drop(gate);
        let eviction = super::PoolEvictionSource::new(&self.inner.pool_name);
        let p = match coordinator
            .acquire(&self.inner.pool_name, &self.inner.username, &eviction)
            .await
        {
            Ok(p) => p,
            Err(pool_coordinator::AcquireError::NoConnection(info)) => {
                return Err(PoolError::DbLimitExhausted(info));
            }
        };

        debug!(
            "[{}@{}] coordinator: permit via slow JIT path \
             (permit_type={})",
            self.inner.username,
            self.inner.pool_name,
            if p.is_reserve { "reserve" } else { "main" },
        );

        // Re-check idle: a sibling may have returned a connection
        // while we waited on the coordinator.
        if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
            return Ok(CoordinatorJitResult::Recycled(inner));
        }

        // Re-acquire burst gate slot.
        match self.acquire_burst_gate(timeouts, false).await {
            BurstGateOutcome::Acquired(new_gate) => Ok(CoordinatorJitResult::Create {
                permit: Some(p),
                gate: new_gate,
            }),
            BurstGateOutcome::Recycled(inner) => Ok(CoordinatorJitResult::Recycled(inner)),
            BurstGateOutcome::Timeout => unreachable!("non_blocking=false"),
        }
    }

    /// Block if the pool is paused, waiting for resume or timeout.
    ///
    /// IMPORTANT: `resume_notified()` must be called BEFORE `is_paused()`
    /// to avoid a race where RESUME fires between the two calls and the
    /// notification is lost.
    async fn wait_if_paused(&self, timeouts: &Timeouts) -> Result<(), PoolError> {
        let resume_notify = self.inner.server_pool.resume_notified();
        if self.inner.server_pool.is_paused() {
            match timeouts.wait {
                Some(duration) => {
                    if tokio::time::timeout(duration, resume_notify).await.is_err() {
                        return Err(PoolError::Timeout(TimeoutType::Wait));
                    }
                }
                None => resume_notify.await,
            }
        }
        Ok(())
    }

    /// Acquire a semaphore permit: fast spin path, then blocking fallback.
    async fn acquire_semaphore(
        &self,
        timeouts: &Timeouts,
    ) -> Result<SemaphorePermit<'_>, PoolError> {
        let mut try_fast = 0;
        loop {
            if try_fast < MAX_FAST_RETRY {
                if let Ok(p) = self.inner.semaphore.try_acquire() {
                    return Ok(p);
                }
                try_fast += 1;
                for _ in 0..4 {
                    std::hint::spin_loop();
                }
                tokio::task::yield_now().await;
                continue;
            }

            let non_blocking = timeouts.wait.is_some_and(|t| t.as_nanos() == 0);
            return if non_blocking {
                self.inner.semaphore.try_acquire().map_err(|e| match e {
                    TryAcquireError::Closed => PoolError::Closed,
                    TryAcquireError::NoPermits => PoolError::Timeout(TimeoutType::Wait),
                })
            } else {
                match timeouts.wait {
                    Some(duration) => {
                        match tokio::time::timeout(duration, self.inner.semaphore.acquire()).await {
                            Ok(Ok(p)) => Ok(p),
                            Ok(Err(_)) => Err(PoolError::Closed),
                            Err(_) => Err(PoolError::Timeout(TimeoutType::Wait)),
                        }
                    }
                    None => self
                        .inner
                        .semaphore
                        .acquire()
                        .await
                        .map_err(|_| PoolError::Closed),
                }
            };
        }
    }

    /// Anticipation zone: try to recycle a connection before creating
    /// a new one. Returns `Some(ObjectInner)` if a connection was
    /// obtained, `None` if the caller should proceed to the create path.
    async fn try_anticipate(
        &self,
        timeouts: &Timeouts,
        start: tokio::time::Instant,
    ) -> Option<ObjectInner> {
        let should_anticipate = {
            let slots = self.inner.slots.lock();
            let warm_threshold = std::cmp::max(
                1,
                (slots.max_size as f32 * self.inner.config.scaling.warm_pool_ratio) as usize,
            );
            slots.size >= warm_threshold
        };
        if !should_anticipate {
            return None;
        }

        let non_blocking = timeouts.wait.is_some_and(|t| t.as_nanos() == 0);

        // Phase A: yield_now spin — catches microsecond races without sleeping.
        let fast_retries = self.inner.config.scaling.fast_retries;
        for _ in 0..fast_retries {
            if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                return Some(*inner);
            }
            for _ in 0..4 {
                std::hint::spin_loop();
            }
            tokio::task::yield_now().await;
        }

        // Capacity deficit: pool has room to grow but idle queue is empty.
        // Skip anticipation — creating a new connection is cheaper.
        // Disabled when a coordinator is configured: anticipation acts as
        // a natural throttle preventing one pool from grabbing all permits.
        let capacity_deficit = self.inner.coordinator.is_none() && {
            let slots = self.inner.slots.lock();
            slots.vec.is_empty() && slots.size < slots.max_size
        };

        // Phase B: direct handoff via oneshot channel.
        if !capacity_deficit && !non_blocking {
            const CREATE_RESERVE: Duration = Duration::from_millis(500);
            const FALLBACK_BUDGET_MS: u64 = 100;

            let total_budget = match timeouts.wait {
                Some(wait) => wait
                    .saturating_sub(start.elapsed())
                    .saturating_sub(CREATE_RESERVE),
                None => Duration::from_millis(FALLBACK_BUDGET_MS),
            };

            if !total_budget.is_zero() {
                const PHASE_4_HARD_CAP_BASE_MS: u64 = 500;
                const PHASE_4_HARD_CAP_JITTER_MS: u64 = 200;
                let cap_ms = PHASE_4_HARD_CAP_BASE_MS
                    - rand::rng().random_range(0..=PHASE_4_HARD_CAP_JITTER_MS);
                let effective_budget = total_budget.min(Duration::from_millis(cap_ms));

                let (tx, rx) = oneshot::channel();
                self.inner.slots.lock().waiters.push_back(tx);

                match tokio::time::timeout(effective_budget, rx).await {
                    Ok(Ok(inner)) => {
                        self.inner
                            .scaling_stats
                            .anticipation_wakes_notify
                            .fetch_add(1, Ordering::Relaxed);
                        if let Ok(inner) = self.recycle_handoff(inner, timeouts).await {
                            return Some(inner);
                        }
                    }
                    _ => {
                        self.inner
                            .scaling_stats
                            .anticipation_wakes_timeout
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }

        // Anticipation either was skipped or timed out.
        self.inner
            .scaling_stats
            .create_fallback
            .fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Instantiates a builder for a new Pool.
    pub fn builder(server_pool: ServerPool) -> PoolBuilder {
        PoolBuilder::new(server_pool)
    }

    fn from_builder(builder: PoolBuilder) -> Self {
        Self {
            inner: Arc::new(PoolInner {
                server_pool: builder.server_pool,
                slots: Mutex::new(Slots {
                    vec: VecDeque::with_capacity(builder.config.max_size),
                    waiters: VecDeque::new(),
                    size: 0,
                    max_size: builder.config.max_size,
                }),
                users: AtomicUsize::new(0),
                semaphore: Semaphore::new(builder.config.max_size),
                config: builder.config,
                coordinator: builder.coordinator,
                pool_name: builder.pool_name,
                username: builder.username,
                inflight_creates: AtomicUsize::new(0),
                create_done: Notify::new(),
                scaling_stats: ScalingStats::default(),
                pre_replacements_in_flight: AtomicUsize::new(0),
            }),
        }
    }

    /// Retrieves an Object from this Pool or waits for one to become available.
    #[inline(always)]
    pub async fn get(&self) -> Result<Object, PoolError> {
        self.timeout_get(&self.timeouts()).await
    }

    /// Retrieves an Object from this Pool using a different timeout than the configured one.
    pub async fn timeout_get(&self, timeouts: &Timeouts) -> Result<Object, PoolError> {
        self.inner.users.fetch_add(1, Ordering::Relaxed);
        scopeguard::defer! {
            self.inner.users.fetch_sub(1, Ordering::Relaxed);
        }

        let start = tokio::time::Instant::now();

        // Phase 1: Wait for unpause.
        self.wait_if_paused(timeouts).await?;

        // Phase 2: Acquire semaphore permit.
        let permit = self.acquire_semaphore(timeouts).await?;

        // Phase 3: Hot-path recycle.
        if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
            self.maybe_trigger_pre_replacement(&inner.metrics);
            return Ok(self.wrap_checkout(*inner, permit));
        }

        // Phase 4: Anticipation (spin + direct handoff).
        if let Some(inner) = self.try_anticipate(timeouts, start).await {
            return Ok(self.wrap_checkout(inner, permit));
        }

        // Phase 5: Drain remaining recyclable connections.
        loop {
            match self.inner.try_recycle_one(timeouts).await {
                RecycleOutcome::Reused(inner) => {
                    return Ok(self.wrap_checkout(*inner, permit));
                }
                RecycleOutcome::Failed => continue,
                RecycleOutcome::Empty => break,
            }
        }

        // Phase 6: Burst gate + coordinator + create.
        let non_blocking = timeouts.wait.is_some_and(|t| t.as_nanos() == 0);
        let _create_gate = match self.acquire_burst_gate(timeouts, non_blocking).await {
            BurstGateOutcome::Acquired(guard) => guard,
            BurstGateOutcome::Recycled(inner) => {
                return Ok(self.wrap_checkout(*inner, permit));
            }
            BurstGateOutcome::Timeout => {
                return Err(PoolError::Timeout(TimeoutType::Wait));
            }
        };

        let (coordinator_permit, _gate) =
            match self.acquire_coordinator_jit(timeouts, _create_gate).await? {
                CoordinatorJitResult::Create {
                    permit: cp,
                    gate: g,
                } => (cp, g),
                CoordinatorJitResult::Recycled(inner) => {
                    return Ok(self.wrap_checkout(*inner, permit));
                }
            };

        let obj_inner = self
            .inner
            .create_connection(timeouts, coordinator_permit)
            .await?;
        Ok(self.wrap_checkout(obj_inner, permit))
    }

    /// Resizes the pool.
    pub fn resize(&self, max_size: usize) {
        let mut slots = self.inner.slots.lock();
        let old_max_size = slots.max_size;
        slots.max_size = max_size;

        // Shrink pool
        if max_size < old_max_size {
            while slots.vec.len() > max_size {
                if slots.vec.pop_back().is_some() {
                    slots.size = slots.size.saturating_sub(1);
                }
            }
            // Reduce semaphore permits
            let permits_to_remove = old_max_size - max_size;
            let _ = self
                .inner
                .semaphore
                .try_acquire_many(permits_to_remove as u32);
            // Reallocate vec
            let mut vec = VecDeque::with_capacity(max_size);
            for obj in slots.vec.drain(..) {
                vec.push_back(obj);
            }
            slots.vec = vec;
        }

        // Grow pool
        if max_size > old_max_size {
            let additional = max_size - old_max_size;
            slots.vec.reserve_exact(additional);
            self.inner.semaphore.add_permits(additional);
        }
    }

    /// Retains only the objects specified by the given function.
    ///
    /// Evicted `ObjectInner`s are extracted into a local Vec and dropped
    /// **after** `slots.lock()` is released. The drop chain on each evicted
    /// object runs `Server::drop` (a `Terminate` syscall to PG) plus
    /// `CoordinatorPermit::drop` (a tokio `Notify::notify_one` that itself
    /// briefly takes an internal mutex). Holding `slots.lock()` across these
    /// blocks any peer caller trying to recycle from the same pool.
    pub fn retain(&self, f: impl Fn(&Server, Metrics) -> bool) {
        let evicted: Vec<ObjectInner> = {
            let mut guard = self.inner.slots.lock();
            // Common case on a healthy retain cycle: nothing to evict.
            // Skip the partition + allocation pair entirely.
            if guard.vec.iter().all(|obj| f(&obj.obj, obj.metrics)) {
                return;
            }
            let mut keep = VecDeque::with_capacity(guard.vec.capacity());
            let mut evicted = Vec::new();
            for obj in guard.vec.drain(..) {
                if f(&obj.obj, obj.metrics) {
                    keep.push_back(obj);
                } else {
                    evicted.push(obj);
                }
            }
            guard.vec = keep;
            guard.size -= evicted.len();
            evicted
        };
        // Lock released here. Syscalls and notify_one fire below, off-lock.
        drop(evicted);
    }

    /// Retains connections, closing oldest first when max limit is set.
    /// If max is 0, behaves like regular retain (closes all matching).
    /// If max > 0, closes at most `max` connections, prioritizing oldest by creation time.
    /// Returns the number of connections closed.
    ///
    /// As with [`retain`], evicted objects are extracted under the lock and
    /// dropped only after the lock is released, so peer callers do not block
    /// on PG `Terminate` syscalls or coordinator wake-ups.
    pub fn retain_oldest_first(
        &self,
        should_close: impl Fn(&Server, &Metrics) -> bool,
        max_to_close: usize,
    ) -> usize {
        let evicted: Vec<ObjectInner> = {
            let mut guard = self.inner.slots.lock();

            if max_to_close == 0 {
                // Early exit when nothing matches — avoid the partition
                // allocation in the frequent "retain cycle sees no stale
                // connections" case.
                if !guard
                    .vec
                    .iter()
                    .any(|obj| should_close(&obj.obj, &obj.metrics))
                {
                    return 0;
                }
                // Unlimited — partition every matching object out of the vec.
                let mut keep = VecDeque::with_capacity(guard.vec.capacity());
                let mut evicted = Vec::new();
                for obj in guard.vec.drain(..) {
                    if should_close(&obj.obj, &obj.metrics) {
                        evicted.push(obj);
                    } else {
                        keep.push_back(obj);
                    }
                }
                guard.vec = keep;
                guard.size -= evicted.len();
                evicted
            } else {
                // Pre-walk to identify the oldest `max_to_close` candidates.
                // We do not extract here — only collect (index, age) pairs.
                let mut candidates: Vec<(usize, u128)> = guard
                    .vec
                    .iter()
                    .enumerate()
                    .filter(|(_, obj)| should_close(&obj.obj, &obj.metrics))
                    .map(|(idx, obj)| (idx, obj.metrics.age().as_millis()))
                    .collect();

                if candidates.is_empty() {
                    return 0;
                }

                // Sort by age descending (oldest first — highest age value)
                candidates.sort_by(|a, b| b.1.cmp(&a.1));

                let to_close: std::collections::HashSet<usize> = candidates
                    .into_iter()
                    .take(max_to_close)
                    .map(|(idx, _)| idx)
                    .collect();

                let mut keep = VecDeque::with_capacity(guard.vec.capacity());
                let mut evicted = Vec::with_capacity(to_close.len());
                for (idx, obj) in guard.vec.drain(..).enumerate() {
                    if to_close.contains(&idx) {
                        evicted.push(obj);
                    } else {
                        keep.push_back(obj);
                    }
                }
                guard.vec = keep;
                guard.size -= evicted.len();
                evicted
            }
        };
        let closed = evicted.len();
        // Lock released here. Drops below run off-lock.
        drop(evicted);
        closed
    }

    /// Evict the oldest idle connection whose age exceeds `min_lifetime_ms`.
    ///
    /// Used by the pool coordinator when it needs to free a connection slot
    /// for another user. The evicted connection's `CoordinatorPermit` is dropped
    /// synchronously, making the slot available immediately.
    ///
    /// Returns `true` if a connection was evicted.
    pub fn evict_one_idle(&self, min_lifetime_ms: u64) -> bool {
        self.retain_oldest_first(
            |_, metrics| metrics.age().as_millis() >= u128::from(min_lifetime_ms),
            1,
        ) > 0
    }

    /// Convert idle reserve connections into main connections when the
    /// coordinator's main semaphore has headroom. Run by the retain task —
    /// never on the hot checkout path — so contention on `slots.lock()`
    /// stays predictable.
    ///
    /// Reserve permits are supposed to be a burst buffer: a backend grabbed
    /// under peak pressure so the pool can push past `max_db_connections`
    /// for a moment. Once the peak is gone, the backend sits in
    /// `slots.vec` as an ordinary idle connection, but its permit still
    /// counts against `reserve_in_use`. Without an upgrade, the reserve
    /// pool shows as occupied even though the main semaphore has free
    /// slots — the next real burst can't tell the buffer is empty, and
    /// `SHOW POOL_COORDINATOR` reports `reserve_used` that doesn't match
    /// actual reserve availability.
    ///
    /// The upgrade itself is a book-keeping swap, not a reconnect: for
    /// each idle reserve backend we try to steal a `db_semaphore` permit
    /// (non-blocking), and on success flip `permit.is_reserve = false`.
    /// The backend stays alive; the reserve semaphore gains a slot.
    ///
    /// Returns the number of permits upgraded.
    pub fn upgrade_reserve_to_main(&self) -> usize {
        let coordinator = match self.inner.coordinator.as_ref() {
            Some(c) => c,
            None => return 0,
        };
        let mut upgraded = 0;
        let mut guard = self.inner.slots.lock();
        for obj in guard.vec.iter_mut() {
            let Some(permit) = obj.coordinator_permit.as_mut() else {
                continue;
            };
            if !permit.is_reserve {
                continue;
            }
            if coordinator.try_upgrade_reserve_to_main() {
                permit.is_reserve = false;
                upgraded += 1;
            } else {
                // Main is saturated too; no point walking the rest of the
                // vec looking for another reserve entry to upgrade.
                break;
            }
        }
        upgraded
    }

    /// Close idle reserve connections that have been idle longer than `min_lifetime_ms`.
    ///
    /// Reserve connections are temporary — created under coordinator pressure when the
    /// main `max_db_connections` limit is reached. They should be released back to the
    /// reserve pool ASAP once idle, not held until the regular `idle_timeout` fires.
    /// This runs as part of the retain cycle to gradually relieve reserve pressure.
    ///
    /// Returns the number of reserve connections closed.
    ///
    /// Same off-lock drop discipline as [`retain`] / [`retain_oldest_first`]:
    /// closed objects are extracted under the lock and dropped after the lock
    /// is released, so the peer pool's eviction syscalls and coordinator
    /// notifications do not stall concurrent recyclers.
    pub fn close_idle_reserve_connections(&self, min_lifetime_ms: u64) -> usize {
        let evicted: Vec<ObjectInner> = {
            let mut guard = self.inner.slots.lock();
            // Common case on pools with `reserve_pool_size = 0` or with
            // reserve connections still within `min_connection_lifetime`:
            // nothing to close. Skip the partition allocation.
            let has_stale_reserve = guard.vec.iter().any(|obj| {
                let is_reserve = obj
                    .coordinator_permit
                    .as_ref()
                    .is_some_and(|p| p.is_reserve);
                is_reserve && obj.metrics.last_used().as_millis() >= u128::from(min_lifetime_ms)
            });
            if !has_stale_reserve {
                return 0;
            }
            let mut keep = VecDeque::with_capacity(guard.vec.capacity());
            let mut evicted = Vec::new();
            for obj in guard.vec.drain(..) {
                let is_reserve = obj
                    .coordinator_permit
                    .as_ref()
                    .is_some_and(|p| p.is_reserve);
                if !is_reserve {
                    keep.push_back(obj);
                    continue;
                }
                // Close reserve connections idle longer than min_connection_lifetime
                let idle = obj.metrics.last_used().as_millis();
                if idle < u128::from(min_lifetime_ms) {
                    keep.push_back(obj);
                } else {
                    evicted.push(obj);
                }
            }
            guard.vec = keep;
            guard.size -= evicted.len();
            evicted
        };
        let closed = evicted.len();
        // Lock released here. Reserve permit drops fire below.
        drop(evicted);
        closed
    }

    /// Get current timeout configuration.
    #[inline(always)]
    pub fn timeouts(&self) -> Timeouts {
        self.inner.config.timeouts
    }

    /// Creates new connections to bring the pool up to the desired count.
    /// Returns the number of connections successfully created.
    /// Stops on the first creation failure to avoid hammering a failing server.
    pub async fn replenish(&self, count: usize) -> usize {
        let mut created = 0;
        for _ in 0..count {
            // Check if there's still room in the pool
            {
                let slots = self.inner.slots.lock();
                if slots.size >= slots.max_size {
                    break;
                }
            }

            // Acquire coordinator permit FIRST (non-blocking). Same ordering
            // rationale as `timeout_get`: a slow coordinator must not hold a
            // burst slot. If the coordinator limit is reached, skip — the
            // next retain cycle will retry.
            let coordinator_permit = if let Some(ref coordinator) = self.inner.coordinator {
                match coordinator.try_acquire() {
                    Some(permit) => Some(permit),
                    None => {
                        log::debug!(
                            "[{}@{}] coordinator limit reached, skipping replenish",
                            self.inner.username,
                            self.inner.pool_name
                        );
                        break;
                    }
                }
            } else {
                None
            };

            // Take the burst slot AFTER the coordinator permit. Replenish runs
            // in the background retain loop, so when client traffic is already
            // saturating the burst gate there is no value in queueing here —
            // defer the work to the next retain cycle and let `timeout_get`
            // callers own the budget. The dropped `coordinator_permit` returns
            // its slot to the cross-pool semaphore.
            let Some(_create_gate) = self.inner.try_acquire_burst_gate() else {
                self.inner
                    .scaling_stats
                    .replenish_deferred
                    .fetch_add(1, Ordering::Relaxed);
                log::debug!(
                    "[{}@{}] replenish: bounded burst at limit, deferring to next cycle",
                    self.inner.username,
                    self.inner.pool_name
                );
                break;
            };

            // Create a new connection
            let obj = match self.inner.server_pool.create().await {
                Ok(obj) => obj,
                Err(e) => {
                    log::debug!(
                        "[{}@{}] replenish: failed to create server: {}",
                        self.inner.username,
                        self.inner.pool_name,
                        e
                    );
                    break;
                }
            };

            let inner = self.inner.new_object_inner(obj, coordinator_permit);

            {
                let mut slots = self.inner.slots.lock();
                if slots.size >= slots.max_size {
                    break;
                }
                slots.size += 1;
                push_idle(self.inner.config.queue_mode, &mut slots.vec, inner);
            }

            created += 1;
        }
        created
    }

    /// Closes this Pool.
    pub fn close(&self) {
        self.resize(0);
        self.inner.semaphore.close();
    }

    /// Indicates whether this Pool has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.semaphore.is_closed()
    }

    /// Retrieves Status of this Pool.
    #[must_use]
    pub fn status(&self) -> Status {
        let slots = self.inner.slots.lock();
        let users = self.inner.users.load(Ordering::Relaxed);
        let (available, waiting) = if users < slots.size {
            (slots.size - users, 0)
        } else {
            (0, users - slots.size)
        };
        Status {
            max_size: slots.max_size,
            size: slots.size,
            available,
            waiting,
        }
    }

    /// Returns ServerPool of this Pool.
    #[must_use]
    pub fn server_pool(&self) -> &ServerPool {
        &self.inner.server_pool
    }

    /// True when every semaphore permit is in use — clients are either
    /// holding connections or queued behind it. Used by housekeeping
    /// (retain loop, lifetime expiration in `recycle()`) to back off and
    /// not close working connections at the moment of peak demand.
    #[must_use]
    pub fn under_pressure(&self) -> bool {
        self.inner.under_pressure()
    }

    /// Test-only handle on the inner semaphore. Used to model client
    /// pressure (drain all permits) in unit tests that exercise the
    /// `under_pressure()` housekeeping gate from peer modules.
    #[cfg(test)]
    pub(crate) fn semaphore(&self) -> &tokio::sync::Semaphore {
        &self.inner.semaphore
    }

    /// Pauses the pool — blocks new connection acquisition.
    pub fn pause(&self) {
        self.inner.server_pool.pause();
    }

    /// Resumes the pool — unblocks waiting clients.
    pub fn resume(&self) {
        self.inner.server_pool.resume();
    }

    /// Returns whether the pool is paused.
    pub fn is_paused(&self) -> bool {
        self.inner.server_pool.is_paused()
    }

    /// Bumps reconnect epoch and drains all idle connections.
    /// Returns the new epoch value.
    pub fn reconnect(&self) -> u32 {
        let new_epoch = self.inner.server_pool.bump_epoch();
        // Drain all idle connections — they have the old epoch
        self.retain(|_, _| false);
        new_epoch
    }

    /// Returns the current reconnect epoch.
    pub fn reconnect_epoch(&self) -> u32 {
        self.inner.server_pool.current_epoch()
    }

    /// Returns a snapshot of the per-pool scaling counters used for tuning
    /// the anticipation + bounded burst path. Cheap — six relaxed atomic
    /// loads. Safe to call from `SHOW POOLS` / Prometheus scrapes.
    pub fn scaling_stats(&self) -> ScalingStatsSnapshot {
        let s = &self.inner.scaling_stats;
        ScalingStatsSnapshot {
            creates_started: s.creates_started.load(Ordering::Relaxed),
            burst_gate_waits: s.burst_gate_waits.load(Ordering::Relaxed),
            anticipation_wakes_notify: s.anticipation_wakes_notify.load(Ordering::Relaxed),
            anticipation_wakes_timeout: s.anticipation_wakes_timeout.load(Ordering::Relaxed),
            create_fallback: s.create_fallback.load(Ordering::Relaxed),
            replenish_deferred: s.replenish_deferred.load(Ordering::Relaxed),
            inflight_creates: self.inner.inflight_creates.load(Ordering::Relaxed),
            pre_replacements_triggered: s.pre_replacements_triggered.load(Ordering::Relaxed),
            pre_replacements_skipped: s.pre_replacements_skipped.load(Ordering::Relaxed),
        }
    }

    /// Recycle a connection received via direct handoff. On success,
    /// returns `Ok(ObjectInner)` — the caller wraps it via
    /// `wrap_checkout`. On failure, decrements `slots.size` (the
    /// backend is gone) and returns `Err(())`.
    async fn recycle_handoff(
        &self,
        mut inner: ObjectInner,
        timeouts: &Timeouts,
    ) -> Result<ObjectInner, ()> {
        let skip_lifetime = self.inner.under_pressure();
        let recycle_result = match timeouts.recycle {
            Some(duration) => {
                match tokio::time::timeout(
                    duration,
                    self.inner
                        .server_pool
                        .recycle(&mut inner.obj, &inner.metrics, skip_lifetime),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(RecycleError::StaticMessage("Recycle timeout")),
                }
            }
            None => {
                self.inner
                    .server_pool
                    .recycle(&mut inner.obj, &inner.metrics, skip_lifetime)
                    .await
            }
        };
        match recycle_result {
            Ok(()) => {
                self.maybe_trigger_pre_replacement(&inner.metrics);
                Ok(inner)
            }
            Err(_) => {
                let mut slots = self.inner.slots.lock();
                slots.size = slots.size.saturating_sub(1);
                Err(())
            }
        }
    }

    /// Check if a connection approaching lifetime expiry should trigger
    /// a background pre-replacement, and spawn the task if so.
    fn maybe_trigger_pre_replacement(&self, metrics: &Metrics) {
        // Quick checks that don't need a lock.
        if metrics.lifetime_ms < PRE_REPLACE_MIN_LIFETIME_MS {
            return;
        }
        let age_ms = metrics.age().as_millis() as u64;
        let threshold = metrics.lifetime_ms * PRE_REPLACE_THRESHOLD_PCT / 100;
        if age_ms < threshold || age_ms >= metrics.lifetime_ms {
            return;
        }
        if self.inner.under_pressure() {
            return;
        }
        if self.inner.server_pool.is_paused() {
            return;
        }

        // Pool tightness + overshoot check under lock.
        {
            let slots = self.inner.slots.lock();
            // Allow overshoot up to max_size + MAX_CONCURRENT_PRE_REPLACEMENTS.
            let in_flight = self
                .inner
                .pre_replacements_in_flight
                .load(Ordering::Relaxed);
            if slots.size + in_flight > slots.max_size + MAX_CONCURRENT_PRE_REPLACEMENTS {
                return;
            }
            // Idle ratio: only pre-replace when < 25% of connections are idle.
            // If the pool has plenty of idle connections it can absorb the
            // loss of one to lifetime expiry without a spike.
            let idle_pct = if slots.size > 0 {
                slots.vec.len() * 100 / slots.size
            } else {
                100
            };
            if idle_pct >= 25 {
                return;
            }
        }

        // Cap concurrent pre-replacements.
        if !try_take_burst_slot(
            &self.inner.pre_replacements_in_flight,
            MAX_CONCURRENT_PRE_REPLACEMENTS,
        ) {
            return;
        }

        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            inner.pre_replace_one().await;
            inner
                .pre_replacements_in_flight
                .fetch_sub(1, Ordering::Release);
        });
    }
}

/// Builder for Pool.
pub struct PoolBuilder {
    server_pool: ServerPool,
    config: PoolConfig,
    coordinator: Option<Arc<pool_coordinator::PoolCoordinator>>,
    pool_name: String,
    username: String,
}

impl PoolBuilder {
    fn new(server_pool: ServerPool) -> Self {
        Self {
            server_pool,
            config: PoolConfig::default(),
            coordinator: None,
            pool_name: String::new(),
            username: String::new(),
        }
    }

    /// Sets the PoolConfig.
    pub fn config(mut self, config: PoolConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets the database-level coordinator (for max_db_connections enforcement).
    pub fn coordinator(
        mut self,
        coordinator: Option<Arc<pool_coordinator::PoolCoordinator>>,
    ) -> Self {
        self.coordinator = coordinator;
        self
    }

    /// Sets the pool name (database name), used in coordinator error messages.
    pub fn pool_name(mut self, name: String) -> Self {
        self.pool_name = name;
        self
    }

    /// Sets the username for this pool, used in coordinator error messages.
    pub fn username(mut self, name: String) -> Self {
        self.username = name;
        self
    }

    /// Builds the Pool.
    pub fn build(self) -> Pool {
        Pool::from_builder(self)
    }
}

impl fmt::Debug for PoolBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolBuilder")
            .field("config", &self.config)
            .finish()
    }
}

/// Try to take a slot from the bounded burst counter.
///
/// Optimistically increments the counter and validates it stayed below `max`.
/// If the slot is available, returns `true` and leaves the counter incremented
/// (caller is responsible for releasing it). If the cap was already reached,
/// rolls back the increment and returns `false`.
///
/// This intentionally tolerates brief over-shoot when many tasks race the
/// `fetch_add`: the next observation will reflect the corrected value once
/// rollback completes. The cap is a soft burst smoother, not a hard fence,
/// and a 1-2 transient excess is acceptable for this purpose.
#[inline]
fn try_take_burst_slot(counter: &AtomicUsize, max: usize) -> bool {
    let prev = counter.fetch_add(1, Ordering::AcqRel);
    if prev < max {
        return true;
    }
    counter.fetch_sub(1, Ordering::Release);
    false
}

/// RAII guard for a burst gate slot. Decrements `inflight_creates`
/// and wakes one burst-gate waiter on drop.
struct BurstGateGuard<'a> {
    inflight_creates: &'a AtomicUsize,
    create_done: &'a Notify,
}

impl Drop for BurstGateGuard<'_> {
    fn drop(&mut self) {
        self.inflight_creates.fetch_sub(1, Ordering::Release);
        self.create_done.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ------------------------------------------------------------------
    // BurstGateGuard — RAII burst gate slot
    // ------------------------------------------------------------------

    #[test]
    fn burst_gate_guard_decrements_on_drop() {
        let counter = AtomicUsize::new(1);
        let notify = Notify::new();
        {
            let _g = BurstGateGuard {
                inflight_creates: &counter,
                create_done: &notify,
            };
            assert_eq!(counter.load(Ordering::Acquire), 1);
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn burst_gate_guard_notifies_on_drop() {
        let counter = AtomicUsize::new(1);
        let notify = Notify::new();
        let fut = notify.notified();
        {
            let _g = BurstGateGuard {
                inflight_creates: &counter,
                create_done: &notify,
            };
        }
        tokio::time::timeout(Duration::from_millis(50), fut)
            .await
            .expect("drop must fire notify_one");
    }

    #[test]
    fn burst_gate_guard_no_decrement_on_forget() {
        let counter = AtomicUsize::new(1);
        let notify = Notify::new();
        let g = BurstGateGuard {
            inflight_creates: &counter,
            create_done: &notify,
        };
        std::mem::forget(g);
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }

    // ------------------------------------------------------------------
    // try_take_burst_slot — soft burst limiter
    // ------------------------------------------------------------------

    #[test]
    fn burst_slot_taken_when_under_cap() {
        let counter = AtomicUsize::new(0);
        assert!(try_take_burst_slot(&counter, 2));
        assert_eq!(counter.load(Ordering::Acquire), 1);
        assert!(try_take_burst_slot(&counter, 2));
        assert_eq!(counter.load(Ordering::Acquire), 2);
    }

    #[test]
    fn burst_slot_rejected_at_cap_and_counter_rolled_back() {
        let counter = AtomicUsize::new(2);
        assert!(!try_take_burst_slot(&counter, 2));
        // Roll-back must restore the counter exactly.
        assert_eq!(counter.load(Ordering::Acquire), 2);
    }

    #[test]
    fn burst_slot_rejected_when_already_above_cap() {
        // Brief transient over-shoot from a racing peer should also reject
        // and roll back, never grow further.
        let counter = AtomicUsize::new(5);
        assert!(!try_take_burst_slot(&counter, 2));
        assert_eq!(counter.load(Ordering::Acquire), 5);
    }

    #[test]
    fn burst_slot_zero_cap_always_rejects() {
        let counter = AtomicUsize::new(0);
        assert!(!try_take_burst_slot(&counter, 0));
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn burst_slot_concurrent_acquire_caps_within_one_of_max() {
        // Stress: many threads racing the gate must never end with more than
        // `max + (threads - max)` rolled-back observations. The gate is a
        // soft cap, so we tolerate up to `max` accepted slots; everyone else
        // must observe rejection and leave the counter at exactly `max`.
        use std::sync::Arc;
        use std::thread;

        const THREADS: usize = 32;
        const MAX: usize = 4;

        let counter = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            let counter = Arc::clone(&counter);
            let accepted = Arc::clone(&accepted);
            handles.push(thread::spawn(move || {
                if try_take_burst_slot(&counter, MAX) {
                    accepted.fetch_add(1, Ordering::Relaxed);
                    // Hold the slot briefly so peers race rejection.
                    thread::yield_now();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let final_count = counter.load(Ordering::Acquire);
        let final_accepted = accepted.load(Ordering::Acquire);
        // No leak: every accepted slot is still in the counter, every
        // rejected attempt rolled back.
        assert_eq!(final_count, final_accepted);
        // Hard upper bound — burst gate must never accept more than MAX.
        assert!(
            final_accepted <= MAX,
            "burst gate accepted {} > MAX {}",
            final_accepted,
            MAX
        );
        // Sanity — at least one thread must have made progress.
        assert!(final_accepted >= 1);
    }

    // ------------------------------------------------------------------
    // Notify register-before-check pattern
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn notify_one_buffered_when_registered_before_signal() {
        // The Phase 4 anticipation loop relies on this property: a
        // notified() registered before notify_one() must wake immediately,
        // even if the await happens after the signal fires.
        let notify = std::sync::Arc::new(Notify::new());
        let n2 = std::sync::Arc::clone(&notify);

        let notified = notify.notified();
        n2.notify_one();
        // notify happened before await — must still wake.
        tokio::time::timeout(Duration::from_millis(50), notified)
            .await
            .expect("notified() must resolve when notify_one fired before await");
    }

    #[tokio::test]
    async fn notify_one_wakes_exactly_one_waiter() {
        // Anti-thundering-herd guarantee: a single return_object must wake
        // exactly one Phase 4 anticipation waiter, not all of them.
        //
        // Synchronization is barrier-based, not sleep-based: each waiter
        // signals it has parked on `notified()` BEFORE awaiting, so the
        // test never races CI scheduling latency.
        use std::sync::Arc;
        use tokio::sync::Barrier;

        const WAITERS: usize = 5;

        let notify = Arc::new(Notify::new());
        let woken = Arc::new(AtomicUsize::new(0));
        // +1 for the test driver itself.
        let registered = Arc::new(Barrier::new(WAITERS + 1));

        let mut handles = Vec::with_capacity(WAITERS);
        for _ in 0..WAITERS {
            let n = Arc::clone(&notify);
            let w = Arc::clone(&woken);
            let r = Arc::clone(&registered);
            handles.push(tokio::spawn(async move {
                // Register the future BEFORE the barrier so the wait below
                // is on a future already attached to the Notify queue.
                let fut = n.notified();
                tokio::pin!(fut);
                fut.as_mut().enable();
                r.wait().await;
                fut.await;
                w.fetch_add(1, Ordering::Relaxed);
            }));
        }

        // All waiters have armed their `Notified` future and are about to await.
        registered.wait().await;
        // Yield once so the spawned tasks reach `fut.await` after the barrier.
        tokio::task::yield_now().await;

        notify.notify_one();

        // Wait for ANY one waiter to record its wake. We do this by polling
        // a counter with a tight yield loop, capped by a generous wall-clock
        // budget so a stuck test fails instead of hanging the suite.
        let started = std::time::Instant::now();
        loop {
            if woken.load(Ordering::Acquire) >= 1 {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "no waiter woke within 2s after notify_one"
            );
            tokio::task::yield_now().await;
        }

        // Strict invariant: only one waiter must be woken by one notify_one.
        // Give the runtime a few yields to surface any spurious extra wakes.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            woken.load(Ordering::Acquire),
            1,
            "exactly one waiter must wake per notify_one"
        );

        // Cleanup: wake the remaining waiters one by one so the spawned tasks
        // can finish and we do not leak them past the test.
        for _ in 0..(WAITERS - 1) {
            notify.notify_one();
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn missed_notify_when_check_precedes_registration() {
        // Negative regression test: this is what would break if a future
        // refactor moved `let notified = ...` AFTER the recycle check in the
        // anticipation phase. The notify fired between the check and the
        // registration is lost, the waiter sleeps until its wake source
        // arrives — proving why the register-before-check ordering matters.
        let notify = Arc::new(Notify::new());

        // Wrong order: signal fires BEFORE the waiter creates its `notified`.
        notify.notify_one();
        let notified = notify.notified();

        // Permit was buffered when no waiter was registered, so the next
        // `notified()` consumes it immediately.
        // (This is the documented tokio behavior we rely on for the
        // register-BEFORE-check pattern: the buffered permit goes to the
        // first future that registers AFTER the signal.)
        tokio::time::timeout(Duration::from_millis(50), notified)
            .await
            .expect("buffered permit must wake the next notified()");

        // Now demonstrate the failure mode: signal fires, the buffered
        // permit is consumed by an unrelated `notified()`, and a LATER
        // `notified()` does NOT see it.
        notify.notify_one();
        let consumer = notify.notified();
        tokio::time::timeout(Duration::from_millis(50), consumer)
            .await
            .expect("buffered permit goes to first future");

        let late = notify.notified();
        let result = tokio::time::timeout(Duration::from_millis(50), late).await;
        assert!(
            result.is_err(),
            "a Notified future created AFTER the buffered permit was consumed \
             must NOT wake without a fresh notify_one"
        );
    }

    // ------------------------------------------------------------------
    // notify_return_observers — covers both fast and slow return_object
    // ------------------------------------------------------------------

    /// Builds a `Pool` whose `ServerPool` is never asked to `create()`.
    /// Address/User defaults are fine because the test never opens a
    /// real backend connection — it only exercises the in-memory notify
    /// machinery on the resulting `PoolInner`.
    fn test_pool_with_coordinator(coord: Arc<pool_coordinator::PoolCoordinator>) -> Pool {
        use crate::config::{Address, User};
        use dashmap::DashMap;

        let server_pool = ServerPool::new(
            Address::default(),
            User::default(),
            "test_db",
            Arc::new(DashMap::new()),
            false,
            false,
            0,
            "test_app".to_string(),
            1,
            60_000,
            60_000,
            60_000,
            Duration::from_secs(5),
            false,
        );
        Pool::builder(server_pool)
            .coordinator(Some(coord))
            .pool_name("test_db".to_string())
            .username("test_user".to_string())
            .build()
    }

    /// `notify_return_observers` wakes the peer-pool coordinator Phase C
    /// waiter so eviction scans can find the just-returned connection.
    /// Same-pool waiters now use direct-handoff oneshot channels inside
    /// `return_object` and do not park on a Notify.
    #[tokio::test]
    async fn notify_return_observers_wakes_phase_c_waiter() {
        use std::sync::atomic::AtomicU64;
        use std::sync::atomic::Ordering as AOrdering;

        use pool_coordinator::{CoordinatorConfig, EvictionSource, PoolCoordinator};

        struct CountingEviction {
            calls: Arc<AtomicU64>,
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

        let coord = PoolCoordinator::new(
            "test_db".to_string(),
            CoordinatorConfig {
                max_db_connections: 1,
                min_connection_lifetime_ms: 5000,
                reserve_pool_size: 0,
                reserve_pool_timeout_ms: 2000,
            },
        );
        let _pinned = coord.try_acquire().expect("first slot is free");

        let pool = test_pool_with_coordinator(coord.clone());

        let coord_w = coord.clone();
        let calls = Arc::new(AtomicU64::new(0));
        let calls_w = Arc::clone(&calls);
        let phase_c_waiter = tokio::spawn(async move {
            let eviction = CountingEviction { calls: calls_w };
            coord_w.acquire("test_db", "u", &eviction).await
        });

        let parked = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if calls.load(AOrdering::Relaxed) >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(parked.is_ok(), "Phase C waiter never parked");
        let baseline = calls.load(AOrdering::Relaxed);
        assert_eq!(
            baseline, 2,
            "Phase B and the first Phase C iteration each call try_evict_one once",
        );

        pool.inner.notify_return_observers();

        let woke = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if calls.load(AOrdering::Relaxed) > baseline {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            woke.is_ok(),
            "Phase C waiter must wake on coordinator.notify_idle_returned",
        );
        assert_eq!(
            calls.load(AOrdering::Relaxed),
            baseline + 1,
            "exactly one Phase C wake -> exactly one extra try_evict_one",
        );

        phase_c_waiter.abort();
        let _ = phase_c_waiter.await;
    }

    // ------------------------------------------------------------------
    // upgrade_reserve_to_main — retain-time book-keeping swap
    // ------------------------------------------------------------------

    /// Smoke test for the retain-time helper: on an empty pool it must
    /// report zero upgrades and leave the coordinator state untouched.
    /// The real coverage of the upgrade arithmetic lives in
    /// `pool_coordinator::tests::reserve_to_main_upgrade_*`; this test
    /// pins the outer wrapper against a refactor that would accidentally
    /// touch coordinator counters on an empty slots vec.
    #[tokio::test]
    async fn upgrade_reserve_to_main_noop_on_empty_pool() {
        let coord = pool_coordinator::PoolCoordinator::new(
            "test_db".to_string(),
            pool_coordinator::CoordinatorConfig {
                max_db_connections: 4,
                min_connection_lifetime_ms: 5000,
                reserve_pool_size: 2,
                reserve_pool_timeout_ms: 100,
            },
        );
        let pool = test_pool_with_coordinator(coord.clone());
        assert_eq!(pool.upgrade_reserve_to_main(), 0);
        assert_eq!(coord.reserve_in_use(), 0);
        assert_eq!(coord.total_connections(), 0);
    }

    /// A pool without a coordinator (max_db_connections = 0) has no
    /// reserve concept at all — the helper must short-circuit and
    /// return 0 without locking `slots`.
    #[tokio::test]
    async fn upgrade_reserve_to_main_returns_zero_without_coordinator() {
        use crate::config::{Address, User};
        use dashmap::DashMap;

        let server_pool = ServerPool::new(
            Address::default(),
            User::default(),
            "test_db",
            Arc::new(DashMap::new()),
            false,
            false,
            0,
            "test_app".to_string(),
            1,
            60_000,
            60_000,
            60_000,
            Duration::from_secs(5),
            false,
        );
        let pool = Pool::builder(server_pool)
            .pool_name("test_db".to_string())
            .username("test_user".to_string())
            .build();
        assert_eq!(pool.upgrade_reserve_to_main(), 0);
    }

    // ------------------------------------------------------------------
    // under_pressure — predicate that gates lifetime housekeeping
    // ------------------------------------------------------------------

    /// `under_pressure` is the gate that decides whether `recycle()` and
    /// the retain loop close a working connection by `server_lifetime`.
    /// Wrong answer here means we either close connections mid-storm
    /// (false negative) or never refresh aged ones (false positive). The
    /// contract is "true iff every semaphore permit is in flight", so the
    /// test acquires all permits, asserts true, releases them, asserts
    /// false.
    #[tokio::test]
    async fn under_pressure_tracks_semaphore_exhaustion() {
        let coord = pool_coordinator::PoolCoordinator::new(
            "test_db".to_string(),
            pool_coordinator::CoordinatorConfig {
                max_db_connections: 0,
                min_connection_lifetime_ms: 0,
                reserve_pool_size: 0,
                reserve_pool_timeout_ms: 0,
            },
        );
        let pool = test_pool_with_coordinator(coord);

        // Builder default for tests is small but non-zero. Read the
        // current permit count so the test does not depend on it.
        let total_permits = pool.inner.semaphore.available_permits();
        assert!(
            total_permits > 0,
            "test pool must start with at least one permit"
        );

        // Empty pool with all permits free → no pressure.
        assert!(
            !pool.inner.under_pressure(),
            "fresh pool must report no pressure"
        );

        // Drain every permit. Holding them models clients holding
        // connections + clients queued behind the semaphore.
        let mut held = Vec::with_capacity(total_permits);
        for _ in 0..total_permits {
            held.push(pool.inner.semaphore.acquire().await.unwrap());
        }
        assert!(
            pool.inner.under_pressure(),
            "drained semaphore must report under_pressure",
        );

        // Release one permit -> pressure clears.
        held.pop();
        assert!(
            !pool.inner.under_pressure(),
            "releasing one permit must clear pressure",
        );
    }

    // ------------------------------------------------------------------
    // Direct handoff — oneshot channel mechanics
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn direct_handoff_delivers_to_oldest_waiter() {
        // Three waiters registered in order. A single send must deliver
        // to the first (oldest) waiter; the other two must not receive.
        let (tx1, rx1) = oneshot::channel::<u32>();
        let (tx2, rx2) = oneshot::channel::<u32>();
        let (tx3, rx3) = oneshot::channel::<u32>();

        let mut waiters = VecDeque::new();
        waiters.push_back(tx1);
        waiters.push_back(tx2);
        waiters.push_back(tx3);

        // Pop the oldest and send.
        let sender = waiters.pop_front().unwrap();
        sender.send(42).expect("receiver must be alive");

        assert_eq!(rx1.await.unwrap(), 42);
        // rx2 and rx3 must not have received anything.
        assert_eq!(waiters.len(), 2);

        // Verify the remaining senders are still pending (not resolved).
        let result = tokio::time::timeout(Duration::from_millis(10), rx2).await;
        assert!(result.is_err(), "second waiter must not receive");
        let result = tokio::time::timeout(Duration::from_millis(10), rx3).await;
        assert!(result.is_err(), "third waiter must not receive");
    }

    #[tokio::test]
    async fn direct_handoff_skips_dropped_receiver() {
        // Simulate a timed-out waiter: register a sender, drop the
        // receiver, then attempt send. The send must fail with the
        // value returned in Err, allowing the caller to try the next
        // waiter or fall back to the idle queue.
        let (tx1, rx1) = oneshot::channel::<u32>();
        let (tx2, rx2) = oneshot::channel::<u32>();

        let mut waiters = VecDeque::new();
        waiters.push_back(tx1);
        waiters.push_back(tx2);

        // Drop first receiver (simulates timeout).
        drop(rx1);

        // Walk the waiters like return_object does.
        let mut value = 99u32;
        while let Some(sender) = waiters.pop_front() {
            match sender.send(value) {
                Ok(()) => {
                    value = 0; // sentinel: delivered
                    break;
                }
                Err(returned) => {
                    value = returned;
                }
            }
        }
        assert_eq!(value, 0, "value must have been delivered to second waiter");
        assert_eq!(rx2.await.unwrap(), 99);
    }

    #[tokio::test]
    async fn direct_handoff_falls_back_when_no_waiters() {
        // With no waiters, there is nothing to pop. The value stays
        // with the caller (simulates the push-to-vec fallback path).
        let waiters: VecDeque<oneshot::Sender<u32>> = VecDeque::new();
        assert!(waiters.is_empty());
        // return_object would push to vec + add_permits here.
    }
}
