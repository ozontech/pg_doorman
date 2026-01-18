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
                inner.metrics.recycled = Some(clock::recent());
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
        let _ = self.inner.users.fetch_add(1, Ordering::Relaxed);
        let _users_guard = scopeguard::guard((), |_| {
            let _ = self.inner.users.fetch_sub(1, Ordering::Relaxed);
        });

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

        // Try to get an existing object from the pool
        loop {
            let obj_inner = {
                let mut slots = self.inner.slots.lock();
                slots.vec.pop_front()
            };

            match obj_inner {
                Some(mut inner) => {
                    // Recycle the object
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
                            // Object is bad, try again
                            let mut slots = self.inner.slots.lock();
                            slots.size = slots.size.saturating_sub(1);
                            continue;
                        }
                    }
                }
                None => {
                    // No existing object, create a new one
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
