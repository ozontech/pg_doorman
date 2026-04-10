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
    /// Number of anticipation loop iterations where a real `idle_returned`
    /// signal woke the waiter. Incremented once per iteration that saw a
    /// notify, including iterations that then lost the post-await recycle
    /// race and looped back.
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
    /// Used by the Phase 4 anticipation loop in `timeout_get` to wait for a
    /// recyclable connection event-driven, and by peer coordinator waiters to
    /// re-attempt peer eviction after a return.
    idle_returned: Notify,
    /// Number of tasks currently awaiting `idle_returned.notified()`. Covers
    /// Phase B anticipation waiters AND burst gate waiters. Incremented before
    /// registering a `Notified` future, decremented on loop exit (scopeguard).
    /// `return_object` checks this before calling `idle_returned.notify_one()`
    /// to avoid waking a task that will race against the semaphore waiter and
    /// lose — at 10k clients this race doubles per-return CPU cost for no gain.
    idle_returned_listeners: AtomicUsize,
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

    /// Wake observers of an idle return: the same-pool Phase 4 anticipation
    /// waiter and any peer-pool Phase C waiter on the coordinator. Both fire
    /// on the same event (a connection landed in `slots.vec`) but consume by
    /// different waiters:
    /// - `idle_returned` is for callers in this pool's Phase 4 anticipation
    ///   loop or Phase 5 burst-gate wait, who will recycle the returned object.
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
        // Only wake anticipation / burst-gate waiters if at least one task
        // is parked on `idle_returned.notified()`. At 10k clients in steady
        // state the idle queue cycles fast and no one enters Phase B or the
        // burst gate — every return would wake a task that races the
        // semaphore waiter and loses, doubling per-return CPU cost for no
        // gain. The atomic load is ~3 ns vs ~104 ns for the notify + the
        // ~2-5 us wasted task wake on the losing side.
        //
        // Safety net: if a listener registers between our load and a missed
        // notify_one, it will still progress — Phase B's SLEEP_CAP (100 ms)
        // and the burst gate's BURST_BACKOFF (5 ms) guarantee an iteration
        // even without a signal.
        if self.idle_returned_listeners.load(Ordering::Acquire) > 0 {
            self.idle_returned.notify_one();
        }
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
                idle_returned_listeners: AtomicUsize::new(0),
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

        // Single deadline for the whole acquire path. Phase 1/2 semaphore wait
        // and Phase 4 anticipation loop both consume from this deadline so
        // their cumulative time cannot exceed the caller's `wait_timeout`.
        let start = tokio::time::Instant::now();

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

            // Capacity deficit: the pool has room to grow (size < max_size)
            // but the idle queue is empty. Phase B anticipation is futile
            // here — no amount of waiting for a recycle will help when the
            // pool genuinely needs a NEW backend connection. Waiting wastes
            // up to 500 ms of the client's budget on race-loss retries that
            // produce nothing; skipping straight to the create path lets the
            // pool recover from lifetime expiry or load spikes immediately.
            //
            // When size == max_size the pool is full and anticipation is the
            // right strategy: a return WILL arrive within the query latency
            // window and recycling avoids a wasted create that would exceed
            // the pool cap anyway.
            let capacity_deficit = {
                let slots = self.inner.slots.lock();
                slots.vec.is_empty() && slots.size < slots.max_size
            };

            // Phase B: event-driven anticipation loop. Wait on `idle_returned`
            // so a single `return_object` wakes exactly one queued task, then
            // retry recycle. Bounded by `timeouts.wait` (the caller's budget)
            // minus a small reserve for the create path. Falls through to the
            // create path only when the deadline is reached — not on a single
            // race loss.
            //
            // Skipped when `capacity_deficit` is true: the pool has room to
            // grow, so creating a new connection is cheaper than racing for
            // returns in the anticipation loop.
            //
            // Why a loop instead of one-shot wait-then-recycle:
            //   return_object() bumps both `idle_returned` AND `semaphore`
            //   permits. A waiter parked in Phase 1/2 blocking semaphore
            //   acquire wakes at the same instant as this Phase B waiter, and
            //   races into Phase 3's hot-path recycle. Under multi-threaded
            //   scheduling the fresh Phase 1/2 waiter frequently wins the race,
            //   popping the just-returned item from `slots.vec` before the
            //   Phase B waiter can reach its post-await recycle. Without the
            //   loop every such race loss became a wasted `server_pool.create()`
            //   even though another return would have arrived within the next
            //   few milliseconds. The loop retries until the caller's deadline.
            if !capacity_deficit && !non_blocking {
                const CREATE_RESERVE: Duration = Duration::from_millis(500);
                // Fallback budget used only when the caller passes no
                // `wait_timeout`. Stock pg_doorman always propagates
                // `query_wait_timeout` into `Timeouts.wait`, so this arm is
                // only reachable from direct API consumers. The constant
                // preserves the historical default so behavior on that path
                // is unchanged.
                const FALLBACK_BUDGET_MS: u64 = 100;

                // Remaining time from the caller's wait_timeout minus the
                // reserve we leave for the create path. `start` was captured
                // at the top of `timeout_get`, so whatever Phase 1/2 spent
                // waiting on the semaphore is already accounted for. The
                // anticipation loop cannot push the caller past its own
                // `wait_timeout` because we share the same deadline.
                let total_budget = match timeouts.wait {
                    Some(wait) => wait
                        .saturating_sub(start.elapsed())
                        .saturating_sub(CREATE_RESERVE),
                    None => Duration::from_millis(FALLBACK_BUDGET_MS),
                };

                if !total_budget.is_zero() {
                    // Register as an idle_returned listener so return_object
                    // knows to call notify_one. Decremented on any exit from
                    // the loop (break, return, or scope drop).
                    self.inner
                        .idle_returned_listeners
                        .fetch_add(1, Ordering::Release);
                    let _listener_guard = scopeguard::guard((), |_| {
                        self.inner
                            .idle_returned_listeners
                            .fetch_sub(1, Ordering::Release);
                    });

                    // Hard cap on Phase 4 wall clock. Independent of race
                    // losses and notify wake ordering, a caller must not
                    // spend more than PHASE_4_HARD_CAP in anticipation
                    // before falling through to the create path. This is
                    // the outer bound that protects tail latency under
                    // pathological wake distributions where a caller
                    // wakes on every notify but loses every post-wake
                    // race. Phase 5's burst gate still caps how many
                    // creates actually reach the backend.
                    const PHASE_4_HARD_CAP: Duration = Duration::from_millis(500);
                    let effective_budget = total_budget.min(PHASE_4_HARD_CAP);
                    let deadline = tokio::time::Instant::now() + effective_budget;
                    // Per-iteration sleep cap — bounds silent-wait time when
                    // nobody notifies. Without it a caller would sleep the
                    // full remaining budget on every race loss.
                    const SLEEP_CAP: Duration = Duration::from_millis(100);
                    // Race-loss cap — bounds how many post-wake recycle
                    // races this caller is allowed to lose in a row before
                    // giving up on anticipation. Tokio's Notify does not
                    // guarantee FIFO wake ordering, so under sustained load
                    // a single caller can wake hundreds of times on
                    // `idle_returned` and lose the post-wake pop race to a
                    // fresh Phase 1/2 waiter every time, never actually
                    // acquiring a connection.
                    const MAX_RACE_LOSSES: u32 = 20;
                    let mut race_losses: u32 = 0;

                    loop {
                        let remaining =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if remaining.is_zero() {
                            self.inner
                                .scaling_stats
                                .anticipation_wakes_timeout
                                .fetch_add(1, Ordering::Relaxed);
                            break;
                        }

                        if race_losses >= MAX_RACE_LOSSES {
                            self.inner
                                .scaling_stats
                                .anticipation_wakes_timeout
                                .fetch_add(1, Ordering::Relaxed);
                            break;
                        }

                        // Register the notification BEFORE re-checking the slots.
                        // If a return_object fires between the check and the await,
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

                        // Sleep up to SLEEP_CAP or until a peer notifies us,
                        // whichever comes first. A capped sleep that finishes
                        // mid-budget means "nothing happened in the last
                        // 100ms, loop back and try recycle again with fresh
                        // state". Only `remaining.is_zero()` at the top of
                        // the loop causes a real timeout exit.
                        let sleep_duration = remaining.min(SLEEP_CAP);
                        let woken_by_notify = tokio::select! {
                            _ = notified => true,
                            _ = tokio::time::sleep(sleep_duration) => false,
                        };
                        if woken_by_notify {
                            self.inner
                                .scaling_stats
                                .anticipation_wakes_notify
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

                        // Race loss or silent wake: loop back after bumping
                        // the counter. The race-loss cap at the top of the
                        // loop guarantees forward progress even when Notify
                        // wake ordering consistently favours peers.
                        race_losses += 1;
                    }
                }
            }

            // Anticipation either was skipped (non_blocking / zero budget) or
            // its loop exhausted the deadline without finding a recyclable
            // connection. Fall through to the create path.
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

        // Bounded burst gate: cap the number of concurrent server creates per
        // pool. Without this gate, N parallel callers that all miss the idle
        // pool each independently issue a backend connect, producing
        // thundering-herd bursts under load. With the cap, only `max_parallel_creates`
        // creates run concurrently; the rest wait on a Notify woken by either
        // an idle return or a peer create completion, then re-check recycle.
        //
        // The gate is taken BEFORE the coordinator permit (JIT permit
        // acquisition). Old ordering (coordinator → gate) caused phantom
        // permits: N callers each took a coordinator permit then queued
        // behind the gate, inflating `total_connections` far beyond the
        // actual connection count and triggering false reserve grants.
        //
        // New ordering (gate → coordinator) ensures permits are only held
        // during actual connection creation. Head-of-line blocking is
        // avoided by releasing the gate slot when the coordinator needs a
        // slow path (eviction/wait), then re-acquiring after the permit
        // arrives.
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
            //
            // Bump `idle_returned_listeners` so `return_object` knows to
            // call `notify_one` while we are parked on the select. The
            // guard decrements on every exit path (break via recycle,
            // loop-back, or early return).
            self.inner
                .idle_returned_listeners
                .fetch_add(1, Ordering::Release);
            let on_create = self.inner.create_done.notified();
            let on_idle = self.inner.idle_returned.notified();

            if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                self.inner
                    .idle_returned_listeners
                    .fetch_sub(1, Ordering::Release);
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
            self.inner
                .idle_returned_listeners
                .fetch_sub(1, Ordering::Release);

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

        // JIT coordinator permit: acquired AFTER the burst gate slot so
        // that waiters queued behind the gate do not inflate
        // `total_connections` with phantom permits. Only callers that
        // actually have a create slot (and will immediately connect) hold
        // a coordinator permit.
        //
        // Fast path (try_acquire): non-blocking CAS, succeeds when the
        // database has headroom. No burst-gate slot is wasted on a wait.
        //
        // Slow path: if the fast try fails, release the burst gate slot
        // so other callers can create while we wait on eviction / peer
        // return. After the coordinator grants the permit, re-acquire a
        // burst gate slot (loop back) — this is rare and acceptable
        // because it only happens under genuine cross-pool pressure.
        let coordinator_permit = if let Some(ref coordinator) = self.inner.coordinator {
            let eviction = super::PoolEvictionSource::new(&self.inner.pool_name);
            match coordinator.try_acquire() {
                Some(p) => {
                    debug!(
                        "[{}@{}] coordinator: permit via fast JIT path \
                         (permit_type=main)",
                        self.inner.username, self.inner.pool_name,
                    );
                    Some(p)
                }
                None => {
                    // Slow path: release burst gate slot to avoid head-of-line
                    // blocking, then wait on the coordinator (may evict / wait
                    // for a peer return), then loop back to re-acquire a gate
                    // slot. The `_create_gate` scopeguard fires on drop,
                    // decrementing `inflight_creates` and notifying a peer.
                    drop(_create_gate);
                    match coordinator
                        .acquire(&self.inner.pool_name, &self.inner.username, &eviction)
                        .await
                    {
                        Ok(p) => {
                            debug!(
                                "[{}@{}] coordinator: permit via slow JIT path \
                                 (permit_type={})",
                                self.inner.username,
                                self.inner.pool_name,
                                if p.is_reserve { "reserve" } else { "main" },
                            );
                            // Re-check idle queue: a sibling caller may have
                            // returned a connection while we waited on the
                            // coordinator. Reusing it is cheaper than a fresh
                            // connect and avoids wasting the gate slot.
                            if let RecycleOutcome::Reused(inner) =
                                self.inner.try_recycle_one(timeouts).await
                            {
                                permit.forget();
                                return Ok(Object {
                                    inner: Some(*inner),
                                    pool: Arc::downgrade(&self.inner),
                                });
                            }

                            // Re-acquire burst gate slot. The loop is the same
                            // as above but without the coordinator acquire
                            // inside — we already have the permit.
                            let max_parallel =
                                self.inner.config.scaling.max_parallel_creates as usize;
                            loop {
                                if try_take_burst_slot(&self.inner.inflight_creates, max_parallel) {
                                    self.inner
                                        .scaling_stats
                                        .creates_started
                                        .fetch_add(1, Ordering::Relaxed);
                                    // Shadow _create_gate with a new guard
                                    // that lives until the end of the function.
                                    // The old one was already dropped above.
                                    let _create_gate = scopeguard::guard((), |_| {
                                        self.inner.inflight_creates.fetch_sub(1, Ordering::Release);
                                        self.inner.create_done.notify_one();
                                    });

                                    // Proceed to create with both permits
                                    let obj = match timeouts.create {
                                        Some(duration) => {
                                            match tokio::time::timeout(
                                                duration,
                                                self.inner.server_pool.create(),
                                            )
                                            .await
                                            {
                                                Ok(Ok(obj)) => obj,
                                                Ok(Err(e)) => return Err(PoolError::Backend(e)),
                                                Err(_) => {
                                                    return Err(PoolError::Timeout(
                                                        TimeoutType::Create,
                                                    ))
                                                }
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
                                    return Ok(Object {
                                        inner: Some(ObjectInner {
                                            obj,
                                            metrics: Metrics::new(
                                                lifetime_ms,
                                                idle_timeout_ms,
                                                epoch,
                                            ),
                                            coordinator_permit: Some(p),
                                        }),
                                        pool: Arc::downgrade(&self.inner),
                                    });
                                }

                                // Burst gate full — wait and retry
                                let on_create = self.inner.create_done.notified();
                                if let RecycleOutcome::Reused(inner) =
                                    self.inner.try_recycle_one(timeouts).await
                                {
                                    permit.forget();
                                    return Ok(Object {
                                        inner: Some(*inner),
                                        pool: Arc::downgrade(&self.inner),
                                    });
                                }
                                tokio::select! {
                                    _ = on_create => {}
                                    _ = tokio::time::sleep(BURST_BACKOFF) => {}
                                }
                            }
                        }
                        Err(pool_coordinator::AcquireError::NoConnection(info)) => {
                            return Err(PoolError::DbLimitExhausted(info));
                        }
                    }
                }
            }
        } else {
            None
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        // Bump idle_returned_listeners so notify_return_observers fires.
        let inner = Arc::clone(&pool.inner);
        inner
            .idle_returned_listeners
            .fetch_add(1, Ordering::Relaxed);
        let idle_observer = tokio::spawn(async move {
            let fut = inner.idle_returned.notified();
            tokio::pin!(fut);
            fut.as_mut().enable();
            fut.await;
            inner
                .idle_returned_listeners
                .fetch_sub(1, Ordering::Relaxed);
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

        // Release one permit → pressure clears.
        held.pop();
        assert!(
            !pool.inner.under_pressure(),
            "releasing one permit must clear pressure",
        );
    }
}
