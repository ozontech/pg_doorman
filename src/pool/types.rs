use std::time::Duration;

use crate::utils::clock;
use rand::Rng as _;

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
    pub const DEFAULT_WARM_POOL_RATIO: f32 = 0.2;
    pub const DEFAULT_FAST_RETRIES: u32 = 10;
    pub const DEFAULT_COOLDOWN_SLEEP_MS: u64 = 10;
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
    /// Individual lifetime for this connection in milliseconds (with jitter applied).
    pub lifetime_ms: u64,
    /// Individual idle timeout for this connection in milliseconds (with jitter applied).
    /// 0 means disabled (no idle timeout).
    pub idle_timeout_ms: u64,
    /// Reconnect epoch at which this connection was created.
    /// Connections with epoch < current pool epoch are rejected in recycle().
    pub epoch: u32,
}

impl Metrics {
    /// Jitter ratio for timeout randomization (±20%).
    const JITTER_RATIO: f64 = 0.2;

    /// Applies ±20% random jitter to a base timeout value.
    /// Returns 0 if the base value is 0 (meaning disabled).
    fn apply_jitter(base_ms: u64) -> u64 {
        if base_ms > 0 {
            let jitter_range = (base_ms as f64 * Self::JITTER_RATIO) as i64;
            let offset = rand::rng().random_range(-jitter_range..=jitter_range);
            (base_ms as i64 + offset).max(1) as u64
        } else {
            0
        }
    }

    /// Creates new Metrics with jitter applied to both lifetime and idle timeout.
    /// Applies ±20% random jitter to prevent mass connection closures
    /// when connections are created or become idle simultaneously.
    pub fn new(base_lifetime_ms: u64, base_idle_timeout_ms: u64, epoch: u32) -> Self {
        Self {
            created: clock::now(),
            recycled: None,
            recycle_count: 0,
            lifetime_ms: Self::apply_jitter(base_lifetime_ms),
            idle_timeout_ms: Self::apply_jitter(base_idle_timeout_ms),
            epoch,
        }
    }

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
            lifetime_ms: 0,
            idle_timeout_ms: 0,
            epoch: 0,
        }
    }
}
