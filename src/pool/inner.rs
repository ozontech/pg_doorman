use std::{
    collections::VecDeque,
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Weak,
    },
};

use log::debug;

use crate::utils::clock;

use parking_lot::Mutex;

use tokio::sync::{Notify, Semaphore, SemaphorePermit, TryAcquireError};

use super::errors::{PoolError, RecycleError, TimeoutType};
use super::pool_coordinator;
use super::types::{Metrics, PoolConfig, QueueMode, Status, Timeouts};
use super::ServerPool;
use crate::server::Server;

const MAX_FAST_RETRY: i32 = 10;

/// Fallback wake interval for tasks queued behind the bounded burst limiter.
/// Used as a safety net in case neither `idle_returned` nor `create_done`
/// fires within the expected window — guarantees forward progress without
/// busy-spinning.
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
#[derive(Debug)]
struct Slots {
    vec: VecDeque<ObjectInner>,
    size: usize,
    max_size: usize,
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
    /// Number of anticipation waits that woke on a real `idle_returned` signal
    /// (the optimistic case — anticipation paid off).
    pub(crate) anticipation_wakes_notify: AtomicU64,
    /// Number of anticipation waits that fell through on the budget timeout
    /// instead of catching a return. Ratio against `anticipation_wakes_notify`
    /// shows whether `max_anticipation_wait_ms` is well-calibrated for the
    /// pool's query latency distribution.
    pub(crate) anticipation_wakes_timeout: AtomicU64,
    /// Number of times anticipation completed but `try_recycle_one` still
    /// found the pool empty, forcing a fall-through to `server_pool.create()`.
    pub(crate) create_fallback: AtomicU64,
    /// Number of times the background `replenish` task hit the burst cap
    /// and deferred its work to the next retain cycle. Persistent non-zero
    /// values indicate `min_pool_size` cannot be sustained under current load.
    pub(crate) replenish_deferred: AtomicU64,
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
    /// Anticipation signal: woken when an Object is returned to the idle pool.
    /// Used by the cooldown zone in `timeout_get` to wait event-driven for a
    /// recyclable connection instead of polling after a blind sleep.
    idle_returned: Notify,
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
}

enum RecycleOutcome {
    Reused(Box<ObjectInner>),
    Failed,
    Empty,
}

impl PoolInner {
    async fn try_recycle_one(&self, timeouts: &Timeouts) -> RecycleOutcome {
        let obj_inner = {
            let mut slots = self.slots.lock();
            slots.vec.pop_front()
        };

        let Some(mut inner) = obj_inner else {
            return RecycleOutcome::Empty;
        };

        let recycle_result = match timeouts.recycle {
            Some(duration) => {
                match tokio::time::timeout(
                    duration,
                    self.server_pool.recycle(&mut inner.obj, &inner.metrics),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(RecycleError::StaticMessage("Recycle timeout")),
                }
            }
            None => {
                self.server_pool
                    .recycle(&mut inner.obj, &inner.metrics)
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
    fn return_object(&self, inner: ObjectInner) {
        // Fast path: try to acquire lock without blocking
        if let Some(mut slots) = self.slots.try_lock() {
            match self.config.queue_mode {
                QueueMode::Fifo => slots.vec.push_back(inner),
                QueueMode::Lifo => slots.vec.push_front(inner),
            }
            drop(slots);
            self.semaphore.add_permits(1);
            self.notify_return_observers();
            return;
        }
        // Slow path: wait for lock.
        let mut slots = self.slots.lock();
        match self.config.queue_mode {
            QueueMode::Fifo => slots.vec.push_back(inner),
            QueueMode::Lifo => slots.vec.push_front(inner),
        }
        drop(slots);
        self.semaphore.add_permits(1);
        self.notify_return_observers();
    }

    /// Wake observers of an idle return: the same-pool cooldown anticipation
    /// waiter and any peer-pool Phase C waiter on the coordinator. Both fire
    /// on the same event (a connection landed in `slots.vec`) but consume by
    /// different waiters:
    /// - `idle_returned` is for callers in this pool's bounded-burst /
    ///   cooldown anticipation zone, who will recycle the returned object.
    /// - `coordinator.notify_idle_returned()` is for callers in *peer* user
    ///   pools waiting on `PoolCoordinator` Phase C; they will scan this
    ///   pool's idle vec via `evict_one_idle` and drop the returned
    ///   connection to free a coordinator slot.
    ///
    /// Note: `spare_above_min` is NOT what changes here. It tracks
    /// `slots.size - effective_min`, and `slots.size` is the allocated count,
    /// not `vec.len()`; `return_object` leaves `slots.size` unchanged. What
    /// changes is the *evictable set* scanned by `retain_oldest_first` inside
    /// `evict_one_idle` — the returned connection is now visible there.
    #[inline(always)]
    fn notify_return_observers(&self) {
        self.idle_returned.notify_one();
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

impl Pool {
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
                    size: 0,
                    max_size: builder.config.max_size,
                }),
                users: AtomicUsize::new(0),
                semaphore: Semaphore::new(builder.config.max_size),
                config: builder.config,
                coordinator: builder.coordinator,
                pool_name: builder.pool_name,
                username: builder.username,
                idle_returned: Notify::new(),
                inflight_creates: AtomicUsize::new(0),
                create_done: Notify::new(),
                scaling_stats: ScalingStats::default(),
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

        // PAUSE check: wait for resume or timeout.
        // IMPORTANT: `resume_notified()` must be called BEFORE `is_paused()` to avoid
        // a race condition where RESUME fires between the two calls and the notification
        // is lost, causing the client to wait until query_wait_timeout (or forever).
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

        let mut try_fast = 0;
        let permit: SemaphorePermit<'_>;
        loop {
            if try_fast < MAX_FAST_RETRY {
                if let Ok(p) = self.inner.semaphore.try_acquire() {
                    permit = p;
                    break;
                }
                try_fast += 1;
                // Short spin before yielding - gives chance for permit
                // to be released on another hyperthread
                for _ in 0..4 {
                    std::hint::spin_loop();
                }
                tokio::task::yield_now().await;
                continue;
            }

            let non_blocking = timeouts.wait.is_some_and(|t| t.as_nanos() == 0);
            permit = if non_blocking {
                self.inner.semaphore.try_acquire().map_err(|e| match e {
                    TryAcquireError::Closed => PoolError::Closed,
                    TryAcquireError::NoPermits => PoolError::Timeout(TimeoutType::Wait),
                })?
            } else {
                match timeouts.wait {
                    Some(duration) => {
                        match tokio::time::timeout(duration, self.inner.semaphore.acquire()).await {
                            Ok(Ok(p)) => p,
                            Ok(Err(_)) => return Err(PoolError::Closed),
                            Err(_) => return Err(PoolError::Timeout(TimeoutType::Wait)),
                        }
                    }
                    None => self
                        .inner
                        .semaphore
                        .acquire()
                        .await
                        .map_err(|_| PoolError::Closed)?,
                }
            };
            break;
        }

        // Hot path: try to get an existing connection
        if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
            permit.forget();
            return Ok(Object {
                inner: Some(*inner),
                pool: Arc::downgrade(&self.inner),
            });
        }

        // No connection available - check if we should use the anticipation zone
        let should_anticipate = {
            let slots = self.inner.slots.lock();
            let warm_threshold = std::cmp::max(
                1,
                (slots.max_size as f32 * self.inner.config.scaling.warm_pool_ratio) as usize,
            );
            slots.size >= warm_threshold
        };

        // Non-blocking checkout (wait_timeout == 0) skips anticipation entirely:
        // the caller wants either an immediate idle hit or a fresh create, no waits.
        let non_blocking = timeouts.wait.is_some_and(|t| t.as_nanos() == 0);

        if should_anticipate {
            // Phase A: yield_now spin — catches microsecond races without sleeping.
            let fast_retries = self.inner.config.scaling.fast_retries;
            for _ in 0..fast_retries {
                if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                    permit.forget();
                    return Ok(Object {
                        inner: Some(*inner),
                        pool: Arc::downgrade(&self.inner),
                    });
                }

                for _ in 0..4 {
                    std::hint::spin_loop();
                }
                tokio::task::yield_now().await;
            }

            // Phase B: event-driven anticipation wait. Instead of a blind sleep,
            // wait on `idle_returned` so a single `return_object` wakes exactly
            // one queued task. Bounded by both the configured anticipation
            // window and the caller's remaining wait budget so the client never
            // blows past its own wait_timeout.
            if !non_blocking {
                let max_wait_ms = self.inner.config.scaling.max_anticipation_wait_ms;
                if max_wait_ms > 0 {
                    let budget = compute_anticipation_budget(timeouts.wait, max_wait_ms);
                    if !budget.is_zero() {
                        // Register the notification BEFORE re-checking the slots:
                        // if a return_object fires between the check and the await,
                        // the notification is buffered and `notified().await` returns
                        // immediately rather than missing the wake.
                        let notified = self.inner.idle_returned.notified();

                        if let RecycleOutcome::Reused(inner) =
                            self.inner.try_recycle_one(timeouts).await
                        {
                            permit.forget();
                            return Ok(Object {
                                inner: Some(*inner),
                                pool: Arc::downgrade(&self.inner),
                            });
                        }

                        let woken_by_notify = tokio::select! {
                            _ = notified => true,
                            _ = tokio::time::sleep(budget) => false,
                        };
                        if woken_by_notify {
                            self.inner
                                .scaling_stats
                                .anticipation_wakes_notify
                                .fetch_add(1, Ordering::Relaxed);
                        } else {
                            self.inner
                                .scaling_stats
                                .anticipation_wakes_timeout
                                .fetch_add(1, Ordering::Relaxed);
                        }

                        if let RecycleOutcome::Reused(inner) =
                            self.inner.try_recycle_one(timeouts).await
                        {
                            permit.forget();
                            return Ok(Object {
                                inner: Some(*inner),
                                pool: Arc::downgrade(&self.inner),
                            });
                        }
                    }
                }
            }

            // Anticipation either timed out or its wake-recycle missed.
            // Either way we are about to allocate a new backend connection.
            self.inner
                .scaling_stats
                .create_fallback
                .fetch_add(1, Ordering::Relaxed);
        }

        // Drain any remaining recyclable connections before creating a new one
        loop {
            match self.inner.try_recycle_one(timeouts).await {
                RecycleOutcome::Reused(inner) => {
                    permit.forget();
                    return Ok(Object {
                        inner: Some(*inner),
                        pool: Arc::downgrade(&self.inner),
                    });
                }
                RecycleOutcome::Failed => continue,
                RecycleOutcome::Empty => break,
            }
        }

        // Acquire coordinator permit FIRST, before taking the bounded burst
        // slot. The two limiters serve different roles and must be ordered
        // so that the slow one (coordinator, can wait up to wait_timeout for
        // eviction or a return in another pool) does not hold the fast one
        // (burst gate, per-pool, woken in milliseconds).
        //
        // Wrong order (gate → coordinator) causes head-of-line blocking
        // inside one pool: 2 callers grab the gate, both wait on coordinator
        // for seconds, the remaining N-2 callers in the pool starve waiting
        // for those 2 to finish, even though the pool itself has nothing to
        // do. Right order (coordinator → gate) makes the gate cap actual
        // backend connect() calls, not waiting time on a peer pool.
        //
        // Only the NEW CONNECTION path goes through the coordinator —
        // idle reuse above is unaffected (permit is already inside ObjectInner).
        let coordinator_permit = if let Some(ref coordinator) = self.inner.coordinator {
            let eviction = super::PoolEvictionSource::new(&self.inner.pool_name);
            match coordinator
                .acquire(&self.inner.pool_name, &self.inner.username, &eviction)
                .await
            {
                Ok(permit) => {
                    debug!(
                        "[{}@{}] coordinator: new connection authorized \
                         (permit_type={})",
                        self.inner.username,
                        self.inner.pool_name,
                        if permit.is_reserve { "reserve" } else { "main" },
                    );
                    Some(permit)
                }
                Err(pool_coordinator::AcquireError::NoConnection(info)) => {
                    return Err(PoolError::DbLimitExhausted(info));
                }
            }
        } else {
            None
        };

        // The coordinator wait above can run for up to `reserve_pool_timeout_ms`
        // (default 3000 ms). During that wait a sibling caller in this same
        // user pool may have finished a query and pushed its connection back
        // into `slots.vec`. The coordinator has no visibility into the local
        // pool — it could not have consumed that return on our behalf — so we
        // re-check the local idle vec here, before paying the cost of a fresh
        // backend connect. If we find a recyclable idle, drop the coordinator
        // permit (its slot returns to the cross-pool semaphore and any peer
        // Phase C waiter can take it) and hand the recycled object to the
        // caller. The connect cost is saved; an eviction the coordinator may
        // have already performed is unrecoverable, but the damage stops at
        // one peer backend instead of cascading into a wasted local create.
        if coordinator_permit.is_some() {
            if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                permit.forget();
                return Ok(Object {
                    inner: Some(*inner),
                    pool: Arc::downgrade(&self.inner),
                });
            }
        }

        // Bounded burst gate: cap the number of concurrent server creates per
        // pool. Without this gate, N parallel callers that all miss the idle
        // pool each independently issue a backend connect, producing
        // thundering-herd bursts under load. With the cap, only `max_parallel_creates`
        // creates run concurrently; the rest wait on a Notify woken by either
        // an idle return or a peer create completion, then re-check recycle.
        //
        // The gate is taken AFTER the coordinator permit so a slow coordinator
        // never holds the gate idle. See the ordering rationale above.
        let max_parallel = self.inner.config.scaling.max_parallel_creates as usize;
        let _create_gate = loop {
            if try_take_burst_slot(&self.inner.inflight_creates, max_parallel) {
                // Got a create slot — guard releases it on drop, no matter
                // whether create() succeeds, fails, or unwinds. Bump
                // `creates_started` here (slot acquisition), not at release,
                // so the counter tracks the in-flight intent rather than
                // post-hoc completions.
                self.inner
                    .scaling_stats
                    .creates_started
                    .fetch_add(1, Ordering::Relaxed);
                break scopeguard::guard((), |_| {
                    self.inner.inflight_creates.fetch_sub(1, Ordering::Release);
                    self.inner.create_done.notify_one();
                });
            }

            self.inner
                .scaling_stats
                .burst_gate_waits
                .fetch_add(1, Ordering::Relaxed);

            if non_blocking {
                // Non-blocking caller: one last recycle attempt, then fail.
                // The coordinator permit dropped here releases the cross-pool
                // slot we briefly held while attempting the gate.
                if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                    permit.forget();
                    return Ok(Object {
                        inner: Some(*inner),
                        pool: Arc::downgrade(&self.inner),
                    });
                }
                return Err(PoolError::Timeout(TimeoutType::Wait));
            }

            // Register both wake sources BEFORE re-checking recycle, so a
            // peer create finishing or an idle return between the check and
            // the await is captured rather than missed.
            let on_create = self.inner.create_done.notified();
            let on_idle = self.inner.idle_returned.notified();

            if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                permit.forget();
                return Ok(Object {
                    inner: Some(*inner),
                    pool: Arc::downgrade(&self.inner),
                });
            }

            tokio::select! {
                _ = on_create => {}
                _ = on_idle => {}
                _ = tokio::time::sleep(BURST_BACKOFF) => {}
            }

            // After wake — try recycle once before retrying the gate.
            if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                permit.forget();
                return Ok(Object {
                    inner: Some(*inner),
                    pool: Arc::downgrade(&self.inner),
                });
            }
            // Loop back to retry the gate.
        };

        // Create a new object
        let obj = match timeouts.create {
            Some(duration) => {
                match tokio::time::timeout(duration, self.inner.server_pool.create()).await {
                    Ok(Ok(obj)) => obj,
                    Ok(Err(e)) => return Err(PoolError::Backend(e)),
                    Err(_) => return Err(PoolError::Timeout(TimeoutType::Create)),
                }
            }
            None => self
                .inner
                .server_pool
                .create()
                .await
                .map_err(PoolError::Backend)?,
        };

        {
            let mut slots = self.inner.slots.lock();
            slots.size += 1;
        }

        permit.forget();
        let lifetime_ms = self.inner.server_pool.lifetime_ms();
        let idle_timeout_ms = self.inner.server_pool.idle_timeout_ms();
        let epoch = self.inner.server_pool.current_epoch();
        Ok(Object {
            inner: Some(ObjectInner {
                obj,
                metrics: Metrics::new(lifetime_ms, idle_timeout_ms, epoch),
                coordinator_permit,
            }),
            pool: Arc::downgrade(&self.inner),
        })
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
        let max_parallel = self.inner.config.scaling.max_parallel_creates as usize;
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
            if !try_take_burst_slot(&self.inner.inflight_creates, max_parallel) {
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
            }
            // Slot is taken — guard releases it on every exit path (success,
            // error, panic) and wakes any task waiting at the bounded burst
            // gate so it can retry recycle or take the slot.
            self.inner
                .scaling_stats
                .creates_started
                .fetch_add(1, Ordering::Relaxed);
            let _create_gate = scopeguard::guard((), |_| {
                self.inner.inflight_creates.fetch_sub(1, Ordering::Release);
                self.inner.create_done.notify_one();
            });

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

            let lifetime_ms = self.inner.server_pool.lifetime_ms();
            let idle_timeout_ms = self.inner.server_pool.idle_timeout_ms();
            let epoch = self.inner.server_pool.current_epoch();
            let inner = ObjectInner {
                obj,
                metrics: Metrics::new(lifetime_ms, idle_timeout_ms, epoch),
                coordinator_permit,
            };

            {
                let mut slots = self.inner.slots.lock();
                if slots.size >= slots.max_size {
                    break;
                }
                slots.size += 1;
                match self.inner.config.queue_mode {
                    QueueMode::Fifo => slots.vec.push_back(inner),
                    QueueMode::Lifo => slots.vec.push_front(inner),
                }
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
        }
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

/// Compute how long the anticipation phase may wait for an idle return.
///
/// The budget is bounded by two independent limits:
///
/// 1. `max_wait_ms` — the operator-configured upper bound on event-driven
///    waiting before falling through to creating a new connection.
/// 2. The caller's remaining `wait_timeout` — anticipation must never burn
///    the entire client budget, since after a miss the caller still needs
///    time to actually create a connection. Half of the remaining timeout
///    is reserved for the create path.
///
/// When `wait_timeout` is `None` the caller has no deadline at all and the
/// full `max_wait_ms` is used. A computed budget below 1ms is rounded up so
/// the wait actually has a chance to register a notification.
fn compute_anticipation_budget(
    wait_timeout: Option<std::time::Duration>,
    max_wait_ms: u64,
) -> std::time::Duration {
    let max = std::time::Duration::from_millis(max_wait_ms);
    let bounded = match wait_timeout {
        None => max,
        Some(remaining) => {
            // Reserve half the caller's budget for the create path.
            let half = remaining / 2;
            std::cmp::min(half, max)
        }
    };
    if bounded.is_zero() {
        std::time::Duration::ZERO
    } else {
        std::cmp::max(bounded, std::time::Duration::from_millis(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn anticipation_budget_uses_full_max_when_no_wait_timeout() {
        let budget = compute_anticipation_budget(None, 100);
        assert_eq!(budget, Duration::from_millis(100));
    }

    #[test]
    fn anticipation_budget_caps_at_half_of_wait_timeout() {
        // remaining wait = 50ms → half = 25ms; max = 100ms → result = 25ms
        let budget = compute_anticipation_budget(Some(Duration::from_millis(50)), 100);
        assert_eq!(budget, Duration::from_millis(25));
    }

    #[test]
    fn anticipation_budget_caps_at_max_when_half_is_larger() {
        // remaining wait = 1000ms → half = 500ms; max = 100ms → result = 100ms
        let budget = compute_anticipation_budget(Some(Duration::from_secs(1)), 100);
        assert_eq!(budget, Duration::from_millis(100));
    }

    #[test]
    fn anticipation_budget_returns_zero_for_non_blocking_caller() {
        // remaining wait = 0 → half = 0 → bounded = 0 → ZERO (do not wait)
        let budget = compute_anticipation_budget(Some(Duration::ZERO), 100);
        assert_eq!(budget, Duration::ZERO);
    }

    #[test]
    fn anticipation_budget_rounds_tiny_budget_up_to_one_ms() {
        // remaining wait = 1ms → half = 500us → bounded = 500us → rounded to 1ms
        let budget = compute_anticipation_budget(Some(Duration::from_millis(1)), 100);
        assert_eq!(budget, Duration::from_millis(1));
    }

    #[test]
    fn anticipation_budget_zero_max_yields_zero() {
        // max_wait_ms = 0 → operator disabled anticipation entirely
        let budget = compute_anticipation_budget(None, 0);
        assert_eq!(budget, Duration::ZERO);
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
        // The cooldown anticipation zone relies on this property:
        // notified() registered before notify_one() must wake immediately,
        // even if the await happens after the signal.
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
        // exactly one cooldown-zone waiter, not all of them.
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

    /// Both fast and slow paths of `return_object` funnel through
    /// `notify_return_observers`. This test pins the helper's contract:
    /// it wakes the same-pool `idle_returned` waiter AND the peer-pool
    /// coordinator Phase C waiter from a single call. A regression that
    /// drops one of the two notify calls inside the helper would fail
    /// this test even though `return_object` itself is not invoked,
    /// because the helper is the single point that both code paths share.
    #[tokio::test]
    async fn notify_return_observers_wakes_phase_c_waiter_and_idle_returned() {
        use std::sync::atomic::AtomicU64;
        use std::sync::atomic::Ordering as AOrdering;

        use pool_coordinator::{CoordinatorConfig, EvictionSource, PoolCoordinator};

        // Counts how many times Phase C asks for an eviction. Phase B
        // and the first iteration of Phase C each call try_evict_one
        // exactly once before parking, so the baseline before any wake
        // is exactly 2.
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

        // Single-slot coordinator, slot pinned so Phase C never finishes
        // via try_acquire — the only thing that can move the counter is
        // a fresh notify_one on `connection_returned`.
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

        // Park a Phase C waiter on the coordinator's connection_returned.
        let coord_w = coord.clone();
        let calls = Arc::new(AtomicU64::new(0));
        let calls_w = Arc::clone(&calls);
        let phase_c_waiter = tokio::spawn(async move {
            let eviction = CountingEviction { calls: calls_w };
            coord_w.acquire("test_db", "u", &eviction).await
        });

        // Park an idle-return observer on the same pool's idle_returned.
        // Pin + enable so the wake cannot race a late first poll.
        let inner = Arc::clone(&pool.inner);
        let idle_observer = tokio::spawn(async move {
            let fut = inner.idle_returned.notified();
            tokio::pin!(fut);
            fut.as_mut().enable();
            fut.await;
        });

        // Wait until both observers are parked. Phase C is observable
        // through its eviction-call counter (baseline = 2 calls). The
        // idle-return observer is observable indirectly: the test will
        // either wake it via notify_return_observers or hang.
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

        // Single helper call: must wake both observers from one event.
        pool.inner.notify_return_observers();

        // The idle-return observer wakes within a generous budget. If the
        // helper forgot the `idle_returned.notify_one()` call, this hangs
        // until the timeout fires and the test fails.
        tokio::time::timeout(Duration::from_secs(1), idle_observer)
            .await
            .expect("idle_returned waiter must wake from notify_return_observers")
            .expect("idle_returned task must not panic");

        // The Phase C waiter wakes, runs try_evict_one once more
        // (baseline + 1) and parks again. If the helper forgot the
        // `coordinator.notify_idle_returned()` call, the counter never
        // moves above the baseline.
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
            "exactly one Phase C wake → exactly one extra try_evict_one",
        );

        // Cleanup: let the Phase C waiter eventually time out so the
        // spawned task does not leak past the test.
        phase_c_waiter.abort();
        let _ = phase_c_waiter.await;
    }
}
