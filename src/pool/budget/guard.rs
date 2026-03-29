use std::time::Instant;

use super::controller::BudgetController;

/// RAII guard returned by `try_acquire_guard`.
/// Automatically calls `release()` on drop unless `confirm()` is called.
/// Use this to protect against CREATE failure leaving phantom held slots.
pub struct AcquireGuard<'a> {
    controller: &'a BudgetController,
    pool: String,
    confirmed: bool,
    now: Instant,
}

impl<'a> AcquireGuard<'a> {
    pub(crate) fn new(controller: &'a BudgetController, pool: &str, now: Instant) -> Self {
        Self {
            controller,
            pool: pool.to_string(),
            confirmed: false,
            now,
        }
    }

    /// Mark the connection as successfully created. The held slot stays.
    pub fn confirm(mut self) {
        self.confirmed = true;
    }
}

impl Drop for AcquireGuard<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.controller.release(&self.pool, self.now);
        }
    }
}
