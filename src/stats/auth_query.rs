use std::sync::atomic::{AtomicU64, Ordering};

/// Source identifier counter shared with the Prometheus scrape path
/// to detect `AuthQueryStats` recreations on `RELOAD`. The first
/// `Default::default()` after process start gets `1`, the next one
/// `2`, and so on; `0` is reserved as the tracker's "never seen"
/// sentinel.
static AUTH_QUERY_STATS_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Returns a unique generation token for the next `AuthQueryStats`.
pub fn next_auth_query_stats_generation() -> u64 {
    AUTH_QUERY_STATS_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Per-pool auth_query metrics. Shared via Arc between AuthQueryState,
/// AuthQueryCache, and the admin/prometheus layers.
pub struct AuthQueryStats {
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub cache_refetches: AtomicU64,
    pub cache_rate_limited: AtomicU64,
    pub auth_success: AtomicU64,
    pub auth_failure: AtomicU64,
    pub executor_queries: AtomicU64,
    pub executor_errors: AtomicU64,
    pub dynamic_pools_created: AtomicU64,
    pub dynamic_pools_destroyed: AtomicU64,
    /// Process-unique source identifier. Set on construction and read
    /// by the scrape-side delta tracker to detect that a config
    /// reload minted a fresh `AuthQueryStats` whose counters start
    /// at zero — see `next_auth_query_stats_generation`.
    pub generation: u64,
}

impl Default for AuthQueryStats {
    fn default() -> Self {
        Self {
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            cache_refetches: AtomicU64::new(0),
            cache_rate_limited: AtomicU64::new(0),
            auth_success: AtomicU64::new(0),
            auth_failure: AtomicU64::new(0),
            executor_queries: AtomicU64::new(0),
            executor_errors: AtomicU64::new(0),
            dynamic_pools_created: AtomicU64::new(0),
            dynamic_pools_destroyed: AtomicU64::new(0),
            generation: next_auth_query_stats_generation(),
        }
    }
}

impl AuthQueryStats {
    /// Load all counters as a snapshot (for admin/prometheus).
    pub fn snapshot(&self) -> AuthQueryStatsSnapshot {
        AuthQueryStatsSnapshot {
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            cache_refetches: self.cache_refetches.load(Ordering::Relaxed),
            cache_rate_limited: self.cache_rate_limited.load(Ordering::Relaxed),
            auth_success: self.auth_success.load(Ordering::Relaxed),
            auth_failure: self.auth_failure.load(Ordering::Relaxed),
            executor_queries: self.executor_queries.load(Ordering::Relaxed),
            executor_errors: self.executor_errors.load(Ordering::Relaxed),
            dynamic_pools_created: self.dynamic_pools_created.load(Ordering::Relaxed),
            dynamic_pools_destroyed: self.dynamic_pools_destroyed.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time snapshot of all auth_query counters (non-atomic, copyable).
#[derive(Debug, Clone, Copy)]
pub struct AuthQueryStatsSnapshot {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_refetches: u64,
    pub cache_rate_limited: u64,
    pub auth_success: u64,
    pub auth_failure: u64,
    pub executor_queries: u64,
    pub executor_errors: u64,
    pub dynamic_pools_created: u64,
    pub dynamic_pools_destroyed: u64,
}
