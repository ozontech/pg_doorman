//! Pool eviction source for the coordinator.
//!
//! Bridges `PoolCoordinator`'s eviction callbacks to real pool state,
//! scanning idle connections across user pools for the same database.

use log::info;

use super::pool_coordinator;
use super::{get_pool, ConnectionPool, PoolIdentifier, POOLS};

/// Adapter bridging `PoolCoordinator`'s eviction callbacks to real pool state.
///
/// The coordinator calls these methods when it needs to free a connection slot:
/// - `try_evict_one`: close one idle connection from another user's pool
/// - `queued_clients`: how many clients are waiting for this user's pool
/// - `is_starving`: whether a user is below their guaranteed minimum
pub struct PoolEvictionSource {
    database: String,
}

impl PoolEvictionSource {
    pub fn new(database: &str) -> Self {
        Self {
            database: database.to_string(),
        }
    }
}

impl pool_coordinator::EvictionSource for PoolEvictionSource {
    /// Evict one idle connection from the user with the largest surplus.
    ///
    /// Scans all pools for the same database, skipping the requesting user.
    /// Snapshots `spare_above_min()` once per candidate to avoid TOCTOU
    /// inconsistency from repeated locking. Evicts only connections older
    /// than `min_connection_lifetime`. The evicted connection's
    /// `CoordinatorPermit` drops synchronously, freeing the slot.
    fn try_evict_one(&self, requesting_user: &str) -> bool {
        let all_pools = POOLS.load();

        // Snapshot spare count once per candidate (avoids repeated locking).
        let mut candidates: Vec<(&PoolIdentifier, &ConnectionPool, usize)> = all_pools
            .iter()
            .filter(|(id, _)| id.db == self.database && id.user != requesting_user)
            .map(|(id, pool)| (id, pool, pool.spare_above_min()))
            .filter(|(_, _, spare)| *spare > 0)
            .collect();

        if candidates.is_empty() {
            return false;
        }

        // Evict from the user with the most surplus first — minimizes impact.
        candidates.sort_by(|a, b| b.2.cmp(&a.2));

        let min_lifetime_ms = candidates
            .first()
            .and_then(|(_, pool, _)| pool.coordinator.as_ref())
            .map(|c| c.config().min_connection_lifetime_ms)
            .unwrap_or(5000);

        for (id, pool, spare) in &candidates {
            if pool.database.evict_one_idle(min_lifetime_ms) {
                info!(
                    "[pool: {}][user: {}] coordinator evicted idle connection (spare:{}) for '{}'",
                    self.database, id.user, spare, requesting_user
                );
                return true;
            }
        }
        false
    }

    fn queued_clients(&self, user: &str) -> usize {
        get_pool(&self.database, user)
            .map(|p| p.pool_state().waiting)
            .unwrap_or(0)
    }

    fn is_starving(&self, user: &str) -> bool {
        get_pool(&self.database, user)
            .map(|p| {
                let user_min = p.settings.user.min_pool_size.unwrap_or(0) as usize;
                let pool_min = p.settings.min_guaranteed_pool_size as usize;
                let effective_min = user_min.max(pool_min);
                let current = p.pool_state().size;
                current < effective_min
            })
            .unwrap_or(false)
    }
}
