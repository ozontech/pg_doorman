use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use log::{info, warn};
use rand::seq::SliceRandom;

use crate::config::get_config;

use super::{get_all_pools, ConnectionPool};

impl ConnectionPool {
    /// Retain pool connections based on idle timeout and lifetime settings.
    /// Returns the number of connections closed.
    /// If `max` is 0, all expired connections will be closed (unlimited).
    /// If `max` > 0, at most `max` connections will be closed across all pools,
    /// prioritizing the oldest connections first.
    pub fn retain_pool_connections(&self, count: Arc<AtomicUsize>, max: usize) -> usize {
        // Closure to determine if a connection should be closed
        // Uses per-connection timeouts with jitter to prevent mass closures
        let should_close = |_: &crate::server::Server, metrics: &crate::pool::Metrics| -> bool {
            // Check idle timeout (per-connection with jitter, 0 = disabled)
            if metrics.idle_timeout_ms > 0 {
                if let Some(v) = metrics.recycled {
                    if (v.elapsed().as_millis() as u64) > metrics.idle_timeout_ms {
                        return true;
                    }
                }
            }
            // Check server lifetime (per-connection with jitter, 0 = disabled)
            if metrics.lifetime_ms > 0 && (metrics.age().as_millis() as u64) > metrics.lifetime_ms {
                return true;
            }
            false
        };

        // Calculate remaining quota for this pool
        let current_count = count.load(Ordering::Relaxed);
        if max > 0 && current_count >= max {
            return 0; // Quota exhausted, skip this pool
        }
        let max_to_close = if max > 0 {
            max - current_count
        } else {
            0 // 0 means unlimited
        };

        // Use retain_oldest_first which sorts by age when max > 0
        let closed = self
            .database
            .retain_oldest_first(should_close, max_to_close);
        count.fetch_add(closed, Ordering::Relaxed);

        if closed > 0 {
            info!(
                "[pool: {}][user: {}] closed {} idle connection{} (base_idle_timeout: {}ms±20%, base_lifetime: {}ms±20%, oldest_first: {})",
                self.address.pool_name,
                self.address.username,
                closed,
                if closed == 1 { "" } else { "s" },
                self.settings.idle_timeout_ms,
                self.settings.life_time_ms,
                max > 0,
            );
        }

        closed
    }

    /// Drain all idle connections from the pool during graceful shutdown.
    /// This immediately closes all idle connections and marks remaining ones for removal.
    pub fn drain_idle_connections(&self) -> usize {
        let status_before = self.database.status();
        let idle_before = status_before.available;

        // Close all idle connections by returning false for all
        self.database.retain(|_, _| false);

        let status_after = self.database.status();
        let closed = idle_before.saturating_sub(status_after.available);

        if closed > 0 {
            info!(
                "[pool: {}][user: {}] drained {} idle connection{}",
                self.address.pool_name,
                self.address.username,
                closed,
                if closed == 1 { "" } else { "s" }
            );
        }

        closed
    }
}

pub async fn retain_connections() {
    let config = get_config();
    let retain_time = config.general.retain_connections_time.as_std();
    let retain_max = config.general.retain_connections_max;
    let mut interval = tokio::time::interval(retain_time);
    let count = Arc::new(AtomicUsize::new(0));

    info!(
        "Starting connection retain task: interval={}ms, max_per_cycle={}",
        retain_time.as_millis(),
        if retain_max == 0 {
            "unlimited".to_string()
        } else {
            retain_max.to_string()
        }
    );

    // Prewarm pools with min_pool_size before the first retain cycle
    for (_, pool) in get_all_pools().iter() {
        if let Some(min_pool_size) = pool.settings.user.min_pool_size {
            let min = min_pool_size as usize;
            let created = pool.database.replenish(min).await;
            if created > 0 {
                info!(
                    "[pool: {}][user: {}] prewarmed {} connection{} (min_pool_size: {})",
                    pool.address.pool_name,
                    pool.address.username,
                    created,
                    if created == 1 { "" } else { "s" },
                    min,
                );
            } else {
                warn!(
                    "[pool: {}][user: {}] prewarm failed — could not create connections (min_pool_size: {})",
                    pool.address.pool_name,
                    pool.address.username,
                    min,
                );
            }
        }
    }

    loop {
        interval.tick().await;

        // Use a single snapshot for both retain and replenish phases
        // to avoid inconsistency if POOLS is atomically updated between them.
        let pools = get_all_pools();

        // Shuffle pool iteration order for fair retain_connections_max distribution.
        // HashMap iteration order is deterministic within a process (fixed RandomState seed),
        // so without shuffling the same pool always gets the entire quota.
        let mut pool_refs: Vec<_> = pools.values().collect();
        pool_refs.shuffle(&mut rand::rng());

        // Reserve pressure relief: close idle reserve connections early.
        // Reserve connections are temporary (created when max_db_connections was
        // reached) and should be released as soon as they've been idle long enough.
        for pool in &pool_refs {
            if let Some(ref coordinator) = pool.coordinator {
                let min_lifetime = coordinator.config().min_connection_lifetime_ms;
                let closed = pool.database.close_idle_reserve_connections(min_lifetime);
                if closed > 0 {
                    info!(
                        "[pool: {}][user: {}] released {} reserve connection{} (idle > {}ms)",
                        pool.address.pool_name,
                        pool.address.username,
                        closed,
                        if closed == 1 { "" } else { "s" },
                        min_lifetime,
                    );
                }
            }
        }

        for pool in &pool_refs {
            pool.retain_pool_connections(count.clone(), retain_max);
        }
        count.store(0, Ordering::Relaxed);

        // Replenish pools below min_pool_size
        for pool in &pool_refs {
            // Don't replenish paused pools — no new connections during PAUSE
            if pool.database.is_paused() {
                continue;
            }
            if let Some(min_pool_size) = pool.settings.user.min_pool_size {
                let min = min_pool_size as usize;
                let current_size = pool.database.status().size;
                if current_size < min {
                    let deficit = min - current_size;
                    let created = pool.database.replenish(deficit).await;
                    if created > 0 {
                        info!(
                            "[pool: {}][user: {}] replenished {} connection{} (min_pool_size: {})",
                            pool.address.pool_name,
                            pool.address.username,
                            created,
                            if created == 1 { "" } else { "s" },
                            min,
                        );
                    } else {
                        warn!(
                            "[pool: {}][user: {}] failed to replenish connections (deficit: {}, min_pool_size: {})",
                            pool.address.pool_name,
                            pool.address.username,
                            deficit,
                            min,
                        );
                    }
                }
            }
        }
    }
}

/// Drain all idle connections from all pools during graceful shutdown.
/// Returns the total number of connections drained.
pub fn drain_all_pools() -> usize {
    let mut total_drained = 0;
    for (_, pool) in get_all_pools().iter() {
        total_drained += pool.drain_idle_connections();
    }
    if total_drained > 0 {
        info!(
            "Graceful shutdown: drained {} idle connection{} from all pools",
            total_drained,
            if total_drained == 1 { "" } else { "s" }
        );
    }
    total_drained
}
