use std::time::Duration;

use crate::utils::clock;

/// Connection scaling configuration for gradual pool growth.
#[derive(Clone, Copy, Debug)]
pub struct ScalingConfig {
    /// Warm pool ratio (0.0-1.0). Connections below this threshold are created immediately.
    pub warm_pool_ratio: f32,

    /// Fast retry count with yield_now() for low latency waiting.
    pub fast_retries: u32,

    /// Sleep duration in ms after fast retries (0 = disabled).
    pub cooldown_sleep_ms: u64,
}

impl ScalingConfig {
    /// Default scaling configuration.
    /// - 20% warm pool (immediate creation)
    /// - 10 fast retries (~10-50μs)
    /// - 10ms sleep after fast retries
    const DEFAULT_WARM_POOL_RATIO: f32 = 0.2;
    const DEFAULT_FAST_RETRIES: u32 = 10;
    const DEFAULT_COOLDOWN_SLEEP_MS: u64 = 10;
}

impl Default for ScalingConfig {
    fn default() -> Self {
        Self {
            warm_pool_ratio: Self::DEFAULT_WARM_POOL_RATIO,
            fast_retries: Self::DEFAULT_FAST_RETRIES,
            cooldown_sleep_ms: Self::DEFAULT_COOLDOWN_SLEEP_MS,
        }
    }
}

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

    /// Scaling configuration for gradual pool growth.
    pub scaling: ScalingConfig,
}

impl PoolConfig {
    /// Creates a new PoolConfig without any timeouts and with the provided max_size.
    #[must_use]
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            timeouts: Timeouts::default(),
            queue_mode: QueueMode::default(),
            scaling: ScalingConfig::default(),
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
    pub created: quanta::Instant,
    /// The instant when this object was last used.
    pub recycled: Option<quanta::Instant>,
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
            created: clock::now(),
            recycled: None,
            recycle_count: 0,
        }
    }
}
