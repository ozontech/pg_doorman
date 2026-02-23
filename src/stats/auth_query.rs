use std::sync::atomic::{AtomicU64, Ordering};

/// Per-pool auth_query metrics. Shared via Arc between AuthQueryState,
/// AuthQueryCache, and the admin/prometheus layers.
#[derive(Default)]
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
