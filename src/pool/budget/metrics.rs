use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters for budget observability.
///
/// All counters use relaxed ordering — they are monotonic
/// and do not synchronize other state. The Prometheus layer
/// reads them periodically; exact inter-counter consistency
/// is not required.
pub struct BudgetMetrics {
    pub grants_guaranteed: AtomicU64,
    pub grants_above: AtomicU64,
    pub grants_after_eviction: AtomicU64,
    pub denied_user_max: AtomicU64,
    pub denied_unknown: AtomicU64,
    pub would_block: AtomicU64,
    pub evictions: AtomicU64,
    pub evictions_blocked: AtomicU64,
    pub releases: AtomicU64,
    pub resets: AtomicU64,
    pub reconciliations: AtomicU64,
    pub denied_timeout: AtomicU64,
}

impl BudgetMetrics {
    pub(crate) fn new() -> Self {
        Self {
            grants_guaranteed: AtomicU64::new(0),
            grants_above: AtomicU64::new(0),
            grants_after_eviction: AtomicU64::new(0),
            denied_user_max: AtomicU64::new(0),
            denied_unknown: AtomicU64::new(0),
            would_block: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            evictions_blocked: AtomicU64::new(0),
            releases: AtomicU64::new(0),
            resets: AtomicU64::new(0),
            reconciliations: AtomicU64::new(0),
            denied_timeout: AtomicU64::new(0),
        }
    }

    pub(crate) fn inc(&self, counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}
