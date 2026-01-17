use std::{
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Weak,
    },
    time::Instant,
};

use crossbeam::queue::ArrayQueue;
use tokio::sync::Notify;

use super::errors::{PoolError, RecycleError, TimeoutType};
use super::types::{Metrics, PoolConfig, Status, Timeouts};
use super::ServerPool;
use crate::server::Server;

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
                inner.metrics.recycled = Some(Instant::now());
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

/// Internal pool state with lock-free queue.
struct PoolInner {
    server_pool: ServerPool,
    /// Lock-free queue for available connections.
    slots: ArrayQueue<ObjectInner>,
    /// Current number of created connections (may be less than max_size).
    size: AtomicUsize,
    /// Number of users currently holding or waiting for objects.
    users: AtomicUsize,
    /// Number of connections currently checked out (in use).
    in_use: AtomicUsize,
    /// Notification for waiters when a connection becomes available.
    notify: Notify,
    /// Whether the pool is closed.
    closed: AtomicBool,
    config: PoolConfig,
}

impl PoolInner {
    fn return_object(&self, inner: ObjectInner) {
        // Push back to queue (ArrayQueue is FIFO by default)
        // For LIFO we would need a different approach, but ArrayQueue doesn't support it directly
        // We'll handle queue_mode in a simplified way for now
        let _ = self.slots.push(inner);
        self.in_use.fetch_sub(1, Ordering::Release);
        // Notify one waiter that a connection is available
        self.notify.notify_one();
    }
}

impl fmt::Debug for PoolInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolInner")
            .field("server_pool", &self.server_pool)
            .field("slots_len", &self.slots.len())
            .field("size", &self.size.load(Ordering::Relaxed))
            .field("in_use", &self.in_use.load(Ordering::Relaxed))
            .field("users", &self.users.load(Ordering::Relaxed))
            .field("closed", &self.closed.load(Ordering::Relaxed))
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
                slots: ArrayQueue::new(builder.config.max_size),
                size: AtomicUsize::new(0),
                users: AtomicUsize::new(0),
                in_use: AtomicUsize::new(0),
                notify: Notify::new(),
                closed: AtomicBool::new(false),
                config: builder.config,
            }),
        }
    }

    /// Retrieves an Object from this Pool or waits for one to become available.
    pub async fn get(&self) -> Result<Object, PoolError> {
        self.timeout_get(&self.timeouts()).await
    }

    /// Retrieves an Object from this Pool using a different timeout than the configured one.
    pub async fn timeout_get(&self, timeouts: &Timeouts) -> Result<Object, PoolError> {
        // Check if pool is closed
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(PoolError::Closed);
        }

        let _ = self.inner.users.fetch_add(1, Ordering::Relaxed);
        let _users_guard = scopeguard::guard((), |_| {
            let _ = self.inner.users.fetch_sub(1, Ordering::Relaxed);
        });

        let non_blocking = match timeouts.wait {
            Some(t) => t.as_nanos() == 0,
            None => false,
        };

        let max_size = self.inner.config.max_size;
        let start = Instant::now();

        loop {
            // Check if pool is closed
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(PoolError::Closed);
            }

            // Fast path: try to get an existing object from the queue
            if let Some(mut inner) = self.inner.slots.pop() {
                // Recycle the object
                let recycle_result = match timeouts.recycle {
                    Some(duration) => {
                        match tokio::time::timeout(
                            duration,
                            self.inner.server_pool.recycle(&mut inner.obj, &inner.metrics),
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
                        self.inner.in_use.fetch_add(1, Ordering::Acquire);
                        return Ok(Object {
                            inner: Some(inner),
                            pool: Arc::downgrade(&self.inner),
                        });
                    }
                    Err(_) => {
                        // Object is bad, decrement size and try again
                        self.inner.size.fetch_sub(1, Ordering::Release);
                        continue;
                    }
                }
            }

            // Try to create a new connection if we haven't reached max_size
            let current_size = self.inner.size.load(Ordering::Acquire);
            
            if current_size < max_size {
                // Try to reserve a slot for new connection
                if self.inner.size.compare_exchange(
                    current_size,
                    current_size + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ).is_ok() {
                    // We reserved a slot, create new connection
                    let obj = match timeouts.create {
                        Some(duration) => {
                            match tokio::time::timeout(duration, self.inner.server_pool.create()).await {
                                Ok(Ok(obj)) => obj,
                                Ok(Err(e)) => {
                                    // Failed to create, release the slot
                                    self.inner.size.fetch_sub(1, Ordering::Release);
                                    return Err(PoolError::Backend(e));
                                }
                                Err(_) => {
                                    self.inner.size.fetch_sub(1, Ordering::Release);
                                    return Err(PoolError::Timeout(TimeoutType::Create));
                                }
                            }
                        }
                        None => match self.inner.server_pool.create().await {
                            Ok(obj) => obj,
                            Err(e) => {
                                self.inner.size.fetch_sub(1, Ordering::Release);
                                return Err(PoolError::Backend(e));
                            }
                        },
                    };

                    self.inner.in_use.fetch_add(1, Ordering::Acquire);
                    return Ok(Object {
                        inner: Some(ObjectInner {
                            obj,
                            metrics: Metrics::default(),
                        }),
                        pool: Arc::downgrade(&self.inner),
                    });
                }
                // CAS failed, another thread got the slot, retry
                continue;
            }

            // Pool is full, need to wait
            if non_blocking {
                return Err(PoolError::Timeout(TimeoutType::Wait));
            }

            // Check timeout
            if let Some(wait_duration) = timeouts.wait {
                if start.elapsed() >= wait_duration {
                    return Err(PoolError::Timeout(TimeoutType::Wait));
                }
                // Wait for notification with remaining timeout
                let remaining = wait_duration.saturating_sub(start.elapsed());
                match tokio::time::timeout(remaining, self.inner.notify.notified()).await {
                    Ok(_) => continue, // Got notification, retry
                    Err(_) => return Err(PoolError::Timeout(TimeoutType::Wait)),
                }
            } else {
                // No timeout, wait indefinitely
                self.inner.notify.notified().await;
            }
        }
    }

    /// Resizes the pool.
    /// Note: With lock-free ArrayQueue, resize is limited - we can only drain connections.
    /// Growing requires recreating the pool.
    pub fn resize(&self, max_size: usize) {
        // For shrinking, drain excess connections
        let current_size = self.inner.size.load(Ordering::Acquire);
        if max_size < current_size {
            let to_remove = current_size - max_size;
            for _ in 0..to_remove {
                if self.inner.slots.pop().is_some() {
                    self.inner.size.fetch_sub(1, Ordering::Release);
                }
            }
        }
        // Note: Growing is not supported with fixed-size ArrayQueue
        // The pool would need to be recreated for that
    }

    /// Retains only the objects specified by the given function.
    /// Note: With lock-free queue, this drains and re-adds matching objects.
    pub fn retain(&self, f: impl Fn(&Server, Metrics) -> bool) {
        // Drain all objects and re-add those that pass the filter
        let mut to_keep = Vec::new();
        while let Some(obj) = self.inner.slots.pop() {
            if f(&obj.obj, obj.metrics) {
                to_keep.push(obj);
            } else {
                self.inner.size.fetch_sub(1, Ordering::Release);
            }
        }
        // Re-add kept objects
        for obj in to_keep {
            let _ = self.inner.slots.push(obj);
        }
    }

    /// Get current timeout configuration.
    pub fn timeouts(&self) -> Timeouts {
        self.inner.config.timeouts
    }

    /// Closes this Pool.
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        // Drain all connections
        while self.inner.slots.pop().is_some() {
            self.inner.size.fetch_sub(1, Ordering::Release);
        }
        // Wake up all waiters so they can see the pool is closed
        self.inner.notify.notify_waiters();
    }

    /// Indicates whether this Pool has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// Retrieves Status of this Pool.
    #[must_use]
    pub fn status(&self) -> Status {
        let size = self.inner.size.load(Ordering::Relaxed);
        let users = self.inner.users.load(Ordering::Relaxed);
        let available = self.inner.slots.len();
        let waiting = users.saturating_sub(size);
        Status {
            max_size: self.inner.config.max_size,
            size,
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
