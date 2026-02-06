use std::{
    collections::VecDeque,
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
};

use crate::utils::clock;

use parking_lot::Mutex;

use tokio::sync::{Semaphore, SemaphorePermit, TryAcquireError};

use super::errors::{PoolError, RecycleError, TimeoutType};
use super::types::{Metrics, PoolConfig, QueueMode, Status, Timeouts};
use super::ServerPool;
use crate::server::Server;

const MAX_FAST_RETRY: i32 = 10;

/// Internal object wrapper with metrics.
#[derive(Debug)]
struct ObjectInner {
    obj: Server,
    metrics: Metrics,
}

/// Wrapper around the actual pooled object which implements Deref and DerefMut.
/// When dropped, the object is returned to the pool.
pub struct Object {
    inner: Option<ObjectInner>,
    pool: Weak<PoolInner>,
}

impl Object {
    /// Takes the object from this wrapper leaving behind an empty wrapper.
    /// This is useful when you want to take ownership of the object.
    #[allow(dead_code)]
    pub fn take(mut this: Self) -> Server {
        let inner = this.inner.take().unwrap();
        inner.obj
    }
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
}

impl PoolInner {
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

        // First, try to get an existing connection (hot path - no cooldown check)
        let obj_inner = {
            let mut slots = self.inner.slots.lock();
            slots.vec.pop_front()
        };

        // If we got a connection, try to recycle it (hot path)
        if let Some(mut inner) = obj_inner {
            let recycle_result = match timeouts.recycle {
                Some(duration) => {
                    match tokio::time::timeout(
                        duration,
                        self.inner
                            .server_pool
                            .recycle(&mut inner.obj, &inner.metrics),
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
                        .recycle(&mut inner.obj, &inner.metrics)
                        .await
                }
            };

            match recycle_result {
                Ok(()) => {
                    permit.forget();
                    return Ok(Object {
                        inner: Some(inner),
                        pool: Arc::downgrade(&self.inner),
                    });
                }
                Err(_) => {
                    let mut slots = self.inner.slots.lock();
                    slots.size = slots.size.saturating_sub(1);
                    // Continue to cooldown logic below
                }
            }
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

        // If in cooldown zone, try to wait for a free connection before creating new one
        if should_use_cooldown {
            // Phase 1: Fast retries with yield_now (low latency, ~10-50μs)
            let fast_retries = self.inner.config.scaling.fast_retries;
            for _ in 0..fast_retries {
                let obj_inner = {
                    let mut slots = self.inner.slots.lock();
                    slots.vec.pop_front()
                };

                if let Some(mut inner) = obj_inner {
                    let recycle_result = match timeouts.recycle {
                        Some(duration) => {
                            match tokio::time::timeout(
                                duration,
                                self.inner
                                    .server_pool
                                    .recycle(&mut inner.obj, &inner.metrics),
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
                                .recycle(&mut inner.obj, &inner.metrics)
                                .await
                        }
                    };

                    match recycle_result {
                        Ok(()) => {
                            permit.forget();
                            return Ok(Object {
                                inner: Some(inner),
                                pool: Arc::downgrade(&self.inner),
                            });
                        }
                        Err(_) => {
                            let mut slots = self.inner.slots.lock();
                            slots.size = slots.size.saturating_sub(1);
                            continue;
                        }
                    }
                }

                // No connection available, short spin + yield before next retry
                for _ in 0..4 {
                    std::hint::spin_loop();
                }
                tokio::task::yield_now().await;
            }

            // Phase 2: Single sleep retry if fast retries didn't help
            let cooldown_sleep_ms = self.inner.config.scaling.cooldown_sleep_ms;
            if cooldown_sleep_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(cooldown_sleep_ms)).await;

                let obj_inner = {
                    let mut slots = self.inner.slots.lock();
                    slots.vec.pop_front()
                };

                if let Some(mut inner) = obj_inner {
                    let recycle_result = match timeouts.recycle {
                        Some(duration) => {
                            match tokio::time::timeout(
                                duration,
                                self.inner
                                    .server_pool
                                    .recycle(&mut inner.obj, &inner.metrics),
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
                                .recycle(&mut inner.obj, &inner.metrics)
                                .await
                        }
                    };

                    match recycle_result {
                        Ok(()) => {
                            permit.forget();
                            return Ok(Object {
                                inner: Some(inner),
                                pool: Arc::downgrade(&self.inner),
                            });
                        }
                        Err(_) => {
                            let mut slots = self.inner.slots.lock();
                            slots.size = slots.size.saturating_sub(1);
                        }
                    }
                }
            }
        }

        // Try to get an existing object from the pool (fast path for non-cooldown or after cooldown retries)
        loop {
            let obj_inner = {
                let mut slots = self.inner.slots.lock();
                slots.vec.pop_front()
            };

            match obj_inner {
                Some(mut inner) => {
                    let recycle_result = match timeouts.recycle {
                        Some(duration) => {
                            match tokio::time::timeout(
                                duration,
                                self.inner
                                    .server_pool
                                    .recycle(&mut inner.obj, &inner.metrics),
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
                                .recycle(&mut inner.obj, &inner.metrics)
                                .await
                        }
                    };

                    match recycle_result {
                        Ok(()) => {
                            permit.forget();
                            return Ok(Object {
                                inner: Some(inner),
                                pool: Arc::downgrade(&self.inner),
                            });
                        }
                        Err(_) => {
                            let mut slots = self.inner.slots.lock();
                            slots.size = slots.size.saturating_sub(1);
                            continue;
                        }
                    }
                }
                None => {
                    // No object available, create a new one
                    break;
                }
            }
        }

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
        Ok(Object {
            inner: Some(ObjectInner {
                obj,
                metrics: Metrics::default(),
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

    /// Get current timeout configuration.
    #[inline(always)]
    pub fn timeouts(&self) -> Timeouts {
        self.inner.config.timeouts
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
}

/// Builder for Pool.
pub struct PoolBuilder {
    server_pool: ServerPool,
    config: PoolConfig,
}

impl PoolBuilder {
    fn new(server_pool: ServerPool) -> Self {
        Self {
            server_pool,
            config: PoolConfig::default(),
        }
    }

    /// Sets the PoolConfig.
    pub fn config(mut self, config: PoolConfig) -> Self {
        self.config = config;
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
