use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// Result of a non-blocking acquire attempt.
#[derive(Debug, PartialEq)]
pub enum AcquireResult {
    /// Slot granted immediately.
    Granted,
    /// Slot granted after evicting a connection from another pool.
    GrantedAfterEviction { evicted_pool: String },
    /// Pool is full, no evictable connections. Caller should wait.
    WouldBlock,
    /// User is at their max_pool_size.
    DeniedUserMax,
    /// Pool not registered.
    DeniedUnknownPool,
}

/// Configuration for a pool participant in the budget.
#[derive(Debug, Clone, Copy)]
pub struct PoolBudgetConfig {
    pub guaranteed: u32,
    pub weight: u32,
    pub max_pool_size: u32,
}

pub(crate) struct BudgetState {
    pub pools: HashMap<String, PoolState>,
    pub total_held: u32,
    /// Deduplicated waiter set in insertion order.
    /// Each pool appears at most once; PoolState.waiting tracks the count.
    pub waiters: Vec<String>,
}

pub(crate) struct PoolState {
    pub config: PoolBudgetConfig,
    pub held: u32,
    pub waiting: u32,
    /// Creation timestamps of held connections, oldest first (front = oldest).
    pub connection_ages: VecDeque<Instant>,
}
