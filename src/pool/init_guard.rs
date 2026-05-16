//! RAII handle that closes the GC race on freshly created dynamic pools.
//!
//! A dynamic pool is inserted into `POOLS` before its first server
//! connection exists, so `pool_state().size == 0`. Without this guard
//! the next GC sweep could remove the pool while the caller is still
//! inside `get_server_parameters()` for the very first connection.
//!
//! The guard owns the pool's `init_complete` flag. `commit` flips the
//! flag after the first connection is established and disables the
//! `Drop` cleanup. If the guard is dropped without `commit` — typically
//! because the auth flow failed or panicked between insertion and the
//! first checkout — `Drop` removes the pool entry from `POOLS` so the
//! next login rebuilds the pool from scratch.
//!
//! `PoolInitGuard::already_committed()` is a no-op variant returned from
//! `create_dynamic_pool` when an existing pool is reused. Drop on it is
//! a no-op; `commit` is harmless.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::PoolIdentifier;

pub struct PoolInitGuard {
    identifier: Option<PoolIdentifier>,
    init_complete: Arc<AtomicBool>,
    committed: bool,
}

impl PoolInitGuard {
    /// Build a guard for a freshly inserted dynamic pool. The caller is
    /// responsible for calling `commit` after the first connection has
    /// been successfully established. Until then GC will skip the pool
    /// because of its `init_complete == false` flag.
    pub fn for_new_pool(identifier: PoolIdentifier, init_complete: Arc<AtomicBool>) -> Self {
        Self {
            identifier: Some(identifier),
            init_complete,
            committed: false,
        }
    }

    /// Build a no-op guard for the early-return path in
    /// `create_dynamic_pool` (existing pool reused). `Drop` does
    /// nothing; `commit` is harmless.
    pub fn already_committed() -> Self {
        Self {
            identifier: None,
            init_complete: Arc::new(AtomicBool::new(true)),
            committed: true,
        }
    }

    /// Mark the pool as fully initialized. After this call the pool is
    /// subject to normal GC rules. Consumes the guard so it cannot be
    /// committed twice or dropped after commit.
    pub fn commit(mut self) {
        self.init_complete.store(true, Ordering::Release);
        self.committed = true;
    }
}

impl Drop for PoolInitGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(id) = self.identifier.take() {
            crate::pool::drop_dynamic_pool(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_committed_leaves_flag_true_and_does_nothing_on_drop() {
        let guard = PoolInitGuard::already_committed();
        assert!(guard.init_complete.load(Ordering::Acquire));
        drop(guard);
    }

    #[test]
    fn commit_flips_flag_and_blocks_drop_cleanup() {
        let id = PoolIdentifier::new("db", "user");
        let flag = Arc::new(AtomicBool::new(false));
        let guard = PoolInitGuard::for_new_pool(id, Arc::clone(&flag));
        assert!(!flag.load(Ordering::Acquire));
        guard.commit();
        assert!(flag.load(Ordering::Acquire));
    }
}
