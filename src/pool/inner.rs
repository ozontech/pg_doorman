use std::{
    collections::VecDeque,
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
};

use log::debug;

use crate::utils::clock;

use parking_lot::Mutex;

use tokio::sync::{Semaphore, SemaphorePermit, TryAcquireError};

use super::errors::{PoolError, RecycleError, TimeoutType};
use super::pool_coordinator;
use super::types::{Metrics, PoolConfig, QueueMode, Status, Timeouts};
use super::ServerPool;
use crate::server::Server;

const MAX_FAST_RETRY: i32 = 10;

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
            return;
        }
        // Slow path: wait for lock
        let mut slots = self.slots.lock();
        match self.config.queue_mode {
            QueueMode::Fifo => slots.vec.push_back(inner),
            QueueMode::Lifo => slots.vec.push_front(inner),
        }
        drop(slots);
        self.semaphore.add_permits(1);
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

        // No connection available - check if we should use cooldown zone logic
        let should_use_cooldown = {
            let slots = self.inner.slots.lock();
            let warm_threshold = std::cmp::max(
                1,
                (slots.max_size as f32 * self.inner.config.scaling.warm_pool_ratio) as usize,
            );
            slots.size >= warm_threshold
        };

        if should_use_cooldown {
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

            let cooldown_sleep_ms = self.inner.config.scaling.cooldown_sleep_ms;
            if cooldown_sleep_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(cooldown_sleep_ms)).await;

                if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
                    permit.forget();
                    return Ok(Object {
                        inner: Some(*inner),
                        pool: Arc::downgrade(&self.inner),
                    });
                }
            }
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

        // Acquire coordinator permit before creating a new connection.
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
    pub fn retain(&self, f: impl Fn(&Server, Metrics) -> bool) {
        let mut guard = self.inner.slots.lock();
        let len_before = guard.vec.len();
        guard.vec.retain_mut(|obj| f(&obj.obj, obj.metrics));
        guard.size -= len_before - guard.vec.len();
    }

    /// Retains connections, closing oldest first when max limit is set.
    /// If max is 0, behaves like regular retain (closes all matching).
    /// If max > 0, closes at most `max` connections, prioritizing oldest by creation time.
    /// Returns the number of connections closed.
    pub fn retain_oldest_first(
        &self,
        should_close: impl Fn(&Server, &Metrics) -> bool,
        max_to_close: usize,
    ) -> usize {
        let mut guard = self.inner.slots.lock();

        if max_to_close == 0 {
            // Unlimited - close all matching connections
            let len_before = guard.vec.len();
            guard
                .vec
                .retain_mut(|obj| !should_close(&obj.obj, &obj.metrics));
            let closed = len_before - guard.vec.len();
            guard.size -= closed;
            return closed;
        }

        // Collect indices of connections that should be closed with their ages
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

        // Sort by age descending (oldest first - highest age value)
        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        // Take at most max_to_close oldest connections
        let to_close: std::collections::HashSet<usize> = candidates
            .into_iter()
            .take(max_to_close)
            .map(|(idx, _)| idx)
            .collect();

        // Remove selected connections by rebuilding the vec
        let len_before = guard.vec.len();
        let mut new_vec = VecDeque::with_capacity(guard.vec.capacity());
        for (idx, obj) in guard.vec.drain(..).enumerate() {
            if !to_close.contains(&idx) {
                new_vec.push_back(obj);
            }
        }
        guard.vec = new_vec;

        let closed = len_before - guard.vec.len();
        guard.size -= closed;
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
    pub fn close_idle_reserve_connections(&self, min_lifetime_ms: u64) -> usize {
        let mut guard = self.inner.slots.lock();
        let len_before = guard.vec.len();
        guard.vec.retain(|obj| {
            let is_reserve = obj
                .coordinator_permit
                .as_ref()
                .is_some_and(|p| p.is_reserve);
            if !is_reserve {
                return true;
            }
            // Close reserve connections idle longer than min_connection_lifetime
            let idle = obj.metrics.last_used().as_millis();
            idle < u128::from(min_lifetime_ms)
        });
        let closed = len_before - guard.vec.len();
        guard.size -= closed;
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

            // Acquire coordinator permit (non-blocking). If the coordinator
            // limit is reached, skip and retry on the next retain cycle.
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
