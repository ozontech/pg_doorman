use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use super::guard::AcquireGuard;
use super::metrics::BudgetMetrics;
use super::scheduling;
use super::types::{AcquireResult, BudgetState, PoolBudgetConfig, PoolState};

/// Instance-level budget controller for server connections.
///
/// Controls the total number of server connections to a PostgreSQL
/// server (host:port) across all pools. Each pool has:
/// - guaranteed: connections always available, opens immediately
/// - weight: priority for above-guarantee competition
/// - max_pool_size: per-pool hard cap
///
/// Eviction rules:
/// - Only above-guarantee connections can be evicted
/// - Only by a requester with strictly higher weight (or guaranteed request = weight ∞)
/// - Only if the connection is older than min_connection_lifetime
pub struct BudgetController {
    pub(crate) max_connections: AtomicU32,
    pub(crate) min_lifetime: Duration,
    pub(crate) state: Mutex<BudgetState>,
    pub(crate) metrics: BudgetMetrics,
}

impl BudgetController {
    pub fn new(max_connections: u32, min_lifetime: Duration) -> Self {
        Self {
            max_connections: AtomicU32::new(max_connections),
            min_lifetime,
            state: Mutex::new(BudgetState {
                pools: HashMap::new(),
                total_held: 0,
                waiters: Vec::new(),
            }),
            metrics: BudgetMetrics::new(),
        }
    }

    pub fn register_pool(&self, name: &str, config: PoolBudgetConfig) {
        let mut state = self.state.lock();
        state.pools.insert(
            name.to_string(),
            PoolState {
                config,
                held: 0,
                waiting: 0,
                connection_ages: VecDeque::new(),
            },
        );
    }

    pub fn unregister_pool(&self, name: &str, now: Instant) {
        let mut state = self.state.lock();
        if let Some(pool) = state.pools.remove(name) {
            state.total_held -= pool.held;
            state.waiters.retain(|w| w != name);

            let max = self.max_connections.load(Ordering::Relaxed);
            while scheduling::schedule(&mut state, max, self.min_lifetime, now).is_some() {}
        }
    }

    /// Check if the guaranteed budget invariant holds.
    pub fn validate_guarantees(&self) -> Result<(), String> {
        let state = self.state.lock();
        let sum: u32 = state.pools.values().map(|p| p.config.guaranteed).sum();
        let max = self.max_connections.load(Ordering::Relaxed);
        if sum > max {
            return Err(format!("sum(guaranteed)={} > max_connections={}", sum, max));
        }
        Ok(())
    }

    /// Non-blocking attempt to acquire a server connection slot.
    pub fn try_acquire(&self, pool: &str, now: Instant) -> AcquireResult {
        let mut state = self.state.lock();

        let pool_state = match state.pools.get(pool) {
            Some(p) => p,
            None => {
                self.metrics.inc(&self.metrics.denied_unknown);
                return AcquireResult::DeniedUnknownPool;
            }
        };

        let held = pool_state.held;
        let config = pool_state.config;

        if held >= config.max_pool_size {
            self.metrics.inc(&self.metrics.denied_user_max);
            return AcquireResult::DeniedUserMax;
        }

        let is_guaranteed = held < config.guaranteed;

        let max_conn = self.max_connections.load(Ordering::Relaxed);
        if state.total_held < max_conn {
            if !is_guaranteed && scheduling::has_higher_weight_waiter(&state, pool, config.weight) {
                scheduling::enqueue_waiter(&mut state, pool);
                self.metrics.inc(&self.metrics.would_block);
                return AcquireResult::WouldBlock;
            }
            scheduling::grant(&mut state, pool, now);
            if is_guaranteed {
                self.metrics.inc(&self.metrics.grants_guaranteed);
            } else {
                self.metrics.inc(&self.metrics.grants_above);
            }
            return AcquireResult::Granted;
        }

        let requester_weight = if is_guaranteed {
            u32::MAX
        } else {
            config.weight
        };
        let mut blocked = 0u32;
        if let Some(victim) = scheduling::find_evictable_with_blocked_count(
            &state,
            pool,
            requester_weight,
            now,
            self.min_lifetime,
            Some(&mut blocked),
        ) {
            let victim_name = victim.clone();
            scheduling::evict_one(&mut state, &victim_name, now, self.min_lifetime);
            self.metrics.inc(&self.metrics.evictions);
            scheduling::grant(&mut state, pool, now);
            self.metrics.inc(&self.metrics.grants_after_eviction);
            return AcquireResult::GrantedAfterEviction {
                evicted_pool: victim_name,
            };
        }

        if blocked > 0 {
            self.metrics
                .evictions_blocked
                .fetch_add(blocked as u64, Ordering::Relaxed);
        }

        scheduling::enqueue_waiter(&mut state, pool);
        self.metrics.inc(&self.metrics.would_block);
        AcquireResult::WouldBlock
    }

    /// Release a server connection slot. Triggers SCHEDULE.
    pub fn release(&self, pool: &str, now: Instant) -> Option<String> {
        let mut state = self.state.lock();

        if let Some(ps) = state.pools.get_mut(pool) {
            if ps.held > 0 {
                ps.held -= 1;
                ps.connection_ages.pop_front();
                state.total_held -= 1;
                self.metrics.inc(&self.metrics.releases);
            }
        }

        let max = self.max_connections.load(Ordering::Relaxed);
        scheduling::schedule(&mut state, max, self.min_lifetime, now)
    }

    /// Bulk reset after PostgreSQL failover (Contract 2).
    pub fn reset_all(&self, now: Instant) {
        let mut state = self.state.lock();

        for ps in state.pools.values_mut() {
            ps.held = 0;
            ps.connection_ages.clear();
        }
        state.total_held = 0;

        self.metrics.inc(&self.metrics.resets);

        let max = self.max_connections.load(Ordering::Relaxed);
        while scheduling::schedule(&mut state, max, self.min_lifetime, now).is_some() {}
    }

    /// Adjust held counter for a single pool to match reality (Contract 4).
    pub fn reconcile(&self, pool: &str, actual_held: u32, now: Instant) {
        let mut state = self.state.lock();

        let budget_held = match state.pools.get(pool) {
            Some(ps) => ps.held,
            None => return,
        };

        if budget_held == actual_held {
            return;
        }

        self.metrics.inc(&self.metrics.reconciliations);

        let diff = actual_held as i64 - budget_held as i64;
        state.total_held = (state.total_held as i64 + diff) as u32;

        let ps = state
            .pools
            .get_mut(pool)
            .expect("BUG: reconcile called for unregistered pool");
        ps.held = actual_held;
        ps.connection_ages.clear();
        for _ in 0..actual_held {
            ps.connection_ages.push_back(now);
        }

        if diff < 0 {
            let max = self.max_connections.load(Ordering::Relaxed);
            while scheduling::schedule(&mut state, max, self.min_lifetime, now).is_some() {}
        }
    }

    /// Cancel a pending wait for `pool` (FM-4 timeout support).
    pub fn cancel_wait(&self, pool: &str) {
        let mut state = self.state.lock();

        let Some(ps) = state.pools.get_mut(pool) else {
            return;
        };

        if ps.waiting == 0 {
            return;
        }

        ps.waiting -= 1;
        self.metrics.inc(&self.metrics.denied_timeout);

        if ps.waiting == 0 {
            state.waiters.retain(|w| w != pool);
        }
    }

    /// Read-only access to atomic metrics counters.
    pub fn metrics(&self) -> &BudgetMetrics {
        &self.metrics
    }

    // --- Getters ---

    pub fn held(&self, pool: &str) -> u32 {
        self.state.lock().pools.get(pool).map_or(0, |p| p.held)
    }

    pub fn total_held(&self) -> u32 {
        self.state.lock().total_held
    }

    pub fn waiting(&self, pool: &str) -> u32 {
        self.state.lock().pools.get(pool).map_or(0, |p| p.waiting)
    }

    pub fn max_connections(&self) -> u32 {
        self.max_connections.load(Ordering::Relaxed)
    }

    /// Change the global budget at runtime (for maintenance windows).
    pub fn set_max_connections(&self, new_max: u32, now: Instant) {
        let old = self.max_connections.swap(new_max, Ordering::Relaxed);
        if new_max > old {
            let mut state = self.state.lock();
            while scheduling::schedule(&mut state, new_max, self.min_lifetime, now).is_some() {}
        }
    }

    /// Acquire with RAII guard (Contract 3).
    pub fn try_acquire_guard(&self, pool: &str, now: Instant) -> Option<AcquireGuard<'_>> {
        match self.try_acquire(pool, now) {
            AcquireResult::Granted | AcquireResult::GrantedAfterEviction { .. } => {
                Some(AcquireGuard::new(self, pool, now))
            }
            _ => None,
        }
    }

    pub fn above_guarantee(&self, pool: &str) -> u32 {
        let state = self.state.lock();
        state
            .pools
            .get(pool)
            .map_or(0, |p| p.held.saturating_sub(p.config.guaranteed))
    }

    /// Inject connections with specific ages (for testing eviction scenarios).
    #[cfg(test)]
    pub fn set_held_with_age(&self, pool: &str, count: u32, created_at: Instant) {
        let mut state = self.state.lock();
        if let Some(ps) = state.pools.get_mut(pool) {
            let old_held = ps.held;
            ps.held = count;
            ps.connection_ages.clear();
            for _ in 0..count {
                ps.connection_ages.push_back(created_at);
            }
            state.total_held = state.total_held - old_held + count;
        }
    }
}
