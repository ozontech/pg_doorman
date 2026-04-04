//! Pool eviction source for the coordinator.
//!
//! Bridges `PoolCoordinator`'s eviction callbacks to real pool state,
//! scanning idle connections across user pools for the same database.

use log::{debug, info};

use crate::utils::format_duration_ms;

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
                    "[{requesting_user}@{}] eviction: no other users' pools exist for this database",
                    self.database,
                );
            } else {
                debug!(
                    "[{requesting_user}@{}] eviction: {} other user(s) checked, none have spare \
                     connections above guaranteed minimum (users: {})",
                    self.database,
                    all_other_users.len(),
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
            "[{requesting_user}@{}] eviction: {} candidate(s) with spare connections ({})",
            self.database,
            candidates.len(),
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
            // Re-check spare to narrow TOCTOU window: another thread may have
            // acquired a connection since the snapshot, reducing spare to 0.
            let current_spare = pool.spare_above_min();
            if current_spare == 0 {
                debug!(
                    "[{}@{}] eviction: skipped — spare dropped to 0 since snapshot \
                     (was {}, requesting_user='{}')",
                    id.user, self.database, spare, requesting_user,
                );
                continue;
            }
            if pool.database.evict_one_idle(min_lifetime_ms) {
                info!(
                    "[{}@{}] coordinator evicted idle connection \
                     (spare={}, min_lifetime={}) to free slot for '{}'",
                    id.user,
                    self.database,
                    spare,
                    format_duration_ms(min_lifetime_ms),
                    requesting_user,
                );
                return true;
            }
            debug!(
                "[{}@{}] eviction: candidate skipped — \
                 no idle connections older than {} (spare={})",
                id.user,
                self.database,
                format_duration_ms(min_lifetime_ms),
                spare,
            );
        }

        debug!(
            "[{requesting_user}@{}] eviction: all {} candidate(s) had connections \
             too young to evict (min_lifetime={})",
            self.database,
            candidates.len(),
            format_duration_ms(min_lifetime_ms),
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
