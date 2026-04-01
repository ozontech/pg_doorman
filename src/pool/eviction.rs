//! Pool eviction source for the coordinator.
//!
//! Bridges `PoolCoordinator`'s eviction callbacks to real pool state,
//! scanning idle connections across user pools for the same database.

use log::{debug, info};

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
        let all_other_users: Vec<(&PoolIdentifier, &ConnectionPool, usize)> = all_pools
            .iter()
            .filter(|(id, _)| id.db == self.database && id.user != requesting_user)
            .map(|(id, pool)| (id, pool, pool.spare_above_min()))
            .collect();

        let mut candidates: Vec<(&PoolIdentifier, &ConnectionPool, usize)> = all_other_users
            .iter()
            .filter(|(_, _, spare)| *spare > 0)
            .cloned()
            .collect();

        if candidates.is_empty() {
            if all_other_users.is_empty() {
                debug!(
                    "[pool: {}] eviction: no other users' pools exist for this database \
                     (requesting_user='{}')",
                    self.database, requesting_user,
                );
            } else {
                debug!(
                    "[pool: {}] eviction: {} other user(s) checked, none have spare \
                     connections above guaranteed minimum (requesting_user='{}', users: {})",
                    self.database,
                    all_other_users.len(),
                    requesting_user,
                    all_other_users
                        .iter()
                        .map(|(id, _, spare)| format!("{}(spare={})", id.user, spare))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            return false;
        }

        // Evict from the user with the most surplus first — minimizes impact.
        candidates.sort_by(|a, b| b.2.cmp(&a.2));

        debug!(
            "[pool: {}] eviction: {} candidate(s) with spare connections \
             (requesting_user='{}', candidates: {})",
            self.database,
            candidates.len(),
            requesting_user,
            candidates
                .iter()
                .map(|(id, _, spare)| format!("{}(spare={})", id.user, spare))
                .collect::<Vec<_>>()
                .join(", "),
        );

        let min_lifetime_ms = candidates
            .first()
            .and_then(|(_, pool, _)| pool.coordinator.as_ref())
            .map(|c| c.config().min_connection_lifetime_ms)
            .unwrap_or(5000);

        for (id, pool, spare) in &candidates {
            if pool.database.evict_one_idle(min_lifetime_ms) {
                info!(
                    "[pool: {}][user: {}] coordinator evicted idle connection \
                     (spare={}, min_lifetime={}ms) to free slot for '{}'",
                    self.database, id.user, spare, min_lifetime_ms, requesting_user,
                );
                return true;
            }
            debug!(
                "[pool: {}][user: {}] eviction: candidate skipped — \
                 no idle connections older than {}ms (spare={})",
                self.database, id.user, min_lifetime_ms, spare,
            );
        }

        debug!(
            "[pool: {}] eviction: all {} candidate(s) had connections \
             too young to evict (min_lifetime={}ms, requesting_user='{}')",
            self.database,
            candidates.len(),
            min_lifetime_ms,
            requesting_user,
        );
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
