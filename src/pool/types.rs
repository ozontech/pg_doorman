use std::time::{Duration, Instant};

/// Pool configuration.
#[derive(Clone, Copy, Debug)]
pub struct PoolConfig {
    /// Maximum size of the pool.
    pub max_size: usize,

    /// Timeouts of the pool.
    pub timeouts: Timeouts,

    /// Queue mode of the pool.
    /// Determines the order of objects being queued and dequeued.
    pub queue_mode: QueueMode,
}

impl PoolConfig {
    /// Creates a new PoolConfig without any timeouts and with the provided max_size.
    #[must_use]
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            timeouts: Timeouts::default(),
            queue_mode: QueueMode::default(),
        }
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self::new(num_cpus::get_physical() * 4)
    }
}

/// Timeouts when getting objects from a pool.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timeouts {
    /// Timeout when waiting for a slot to become available.
    pub wait: Option<Duration>,

    /// Timeout when creating a new object.
    pub create: Option<Duration>,

    /// Timeout when recycling an object.
    pub recycle: Option<Duration>,
}

impl Timeouts {
    /// Create an empty Timeouts config (no timeouts set).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Mode for dequeuing objects from a pool.
#[derive(Clone, Copy, Debug)]
pub enum QueueMode {
    /// Dequeue the object that was least recently added (first in first out).
    Fifo,
    /// Dequeue the object that was most recently added (last in first out).
    Lifo,
}

impl Default for QueueMode {
    fn default() -> Self {
        Self::Fifo
    }
}

/// The current pool status.
#[derive(Clone, Copy, Debug)]
pub struct Status {
    /// The maximum size of the pool.
    pub max_size: usize,

    /// The current size of the pool.
    pub size: usize,

    /// The number of available objects in the pool.
    pub available: usize,

    /// The number of futures waiting for an object.
    pub waiting: usize,
}

/// Statistics regarding an object returned by the pool.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct Metrics {
    /// The instant when this object was created.
    pub created: Instant,
    /// The instant when this object was last used.
    pub recycled: Option<Instant>,
    /// The number of times the object was recycled.
    pub recycle_count: usize,
}

impl Metrics {
    /// Access the age of this object.
    pub fn age(&self) -> Duration {
        self.created.elapsed()
    }

    /// Get the time elapsed when this object was last used.
    pub fn last_used(&self) -> Duration {
        self.recycled.unwrap_or(self.created).elapsed()
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            created: Instant::now(),
            recycled: None,
            recycle_count: 0,
        }
    }
}
