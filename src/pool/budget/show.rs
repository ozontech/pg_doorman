use std::borrow::Cow;
use std::sync::atomic::Ordering;

use crate::messages::DataType;

use super::controller::BudgetController;

impl BudgetController {
    /// Column definitions for SHOW BUDGET.
    pub fn show_budget_header() -> Vec<(&'static str, DataType)> {
        vec![
            ("max_connections", DataType::Numeric),
            ("total_held", DataType::Numeric),
            ("total_waiting", DataType::Numeric),
            ("pools_registered", DataType::Numeric),
            ("min_lifetime_ms", DataType::Numeric),
            ("grants_guaranteed", DataType::Numeric),
            ("grants_above", DataType::Numeric),
            ("grants_after_eviction", DataType::Numeric),
            ("evictions", DataType::Numeric),
            ("evictions_blocked", DataType::Numeric),
            ("denied_user_max", DataType::Numeric),
            ("denied_timeout", DataType::Numeric),
            ("would_block", DataType::Numeric),
            ("releases", DataType::Numeric),
            ("resets", DataType::Numeric),
            ("reconciliations", DataType::Numeric),
        ]
    }

    /// Single row for SHOW BUDGET.
    pub fn show_budget_row(&self) -> Vec<String> {
        let state = self.state.lock();
        let total_waiting: u32 = state.pools.values().map(|p| p.waiting).sum();
        let pools_count = state.pools.len();
        let m = &self.metrics;
        vec![
            self.max_connections.load(Ordering::Relaxed).to_string(),
            state.total_held.to_string(),
            total_waiting.to_string(),
            pools_count.to_string(),
            self.min_lifetime.as_millis().to_string(),
            m.grants_guaranteed.load(Ordering::Relaxed).to_string(),
            m.grants_above.load(Ordering::Relaxed).to_string(),
            m.grants_after_eviction.load(Ordering::Relaxed).to_string(),
            m.evictions.load(Ordering::Relaxed).to_string(),
            m.evictions_blocked.load(Ordering::Relaxed).to_string(),
            m.denied_user_max.load(Ordering::Relaxed).to_string(),
            m.denied_timeout.load(Ordering::Relaxed).to_string(),
            m.would_block.load(Ordering::Relaxed).to_string(),
            m.releases.load(Ordering::Relaxed).to_string(),
            m.resets.load(Ordering::Relaxed).to_string(),
            m.reconciliations.load(Ordering::Relaxed).to_string(),
        ]
    }

    /// Column definitions for SHOW BUDGET_POOLS.
    pub fn show_budget_pools_header() -> Vec<(&'static str, DataType)> {
        vec![
            ("pool", DataType::Text),
            ("guaranteed", DataType::Numeric),
            ("weight", DataType::Numeric),
            ("max_pool_size", DataType::Numeric),
            ("held", DataType::Numeric),
            ("above_guarantee", DataType::Numeric),
            ("waiting", DataType::Numeric),
            ("is_waiter", DataType::Text),
        ]
    }

    /// Rows for SHOW BUDGET_POOLS. One row per registered pool, sorted by pool name.
    pub fn show_budget_pools_rows(&self) -> Vec<Vec<Cow<'static, str>>> {
        let state = self.state.lock();
        let mut names: Vec<&String> = state.pools.keys().collect();
        names.sort();

        names
            .iter()
            .map(|name| {
                let ps = &state.pools[*name];
                let above = ps.held.saturating_sub(ps.config.guaranteed);
                let is_waiter = state.waiters.contains(name);
                vec![
                    Cow::Owned((*name).clone()),
                    Cow::Owned(ps.config.guaranteed.to_string()),
                    Cow::Owned(ps.config.weight.to_string()),
                    Cow::Owned(ps.config.max_pool_size.to_string()),
                    Cow::Owned(ps.held.to_string()),
                    Cow::Owned(above.to_string()),
                    Cow::Owned(ps.waiting.to_string()),
                    Cow::Borrowed(if is_waiter { "1" } else { "0" }),
                ]
            })
            .collect()
    }
}
