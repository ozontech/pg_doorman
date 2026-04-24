use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use log::{info, warn};
use rand::seq::SliceRandom;

use crate::config::get_config;
use crate::utils::{format_duration_ms, format_elapsed};

use super::{get_all_pools, ConnectionPool};

impl ConnectionPool {
    /// Retain pool connections based on idle timeout and lifetime settings.
    /// Returns the number of connections closed.
    /// If `max` is 0, all expired connections will be closed (unlimited).
    /// If `max` > 0, at most `max` connections will be closed across all pools,
    /// prioritizing the oldest connections first.
    ///
    /// Pools under client pressure are skipped: closing an idle connection
    /// the moment a client is queued behind it just turns a free recycle
    /// into a fresh connect on the wait path.
    pub fn retain_pool_connections(&self, count: Arc<AtomicUsize>, max: usize) -> usize {
        if self.database.under_pressure() {
            return 0;
        }

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
            let idle_timeout = self.settings.idle_timeout_ms;
            let lifetime = self.settings.life_time_ms;
            let limits = match (idle_timeout > 0, lifetime > 0) {
                (true, true) => format!(
                    "idle_timeout=~{}, lifetime=~{}",
                    format_duration_ms(idle_timeout),
                    format_duration_ms(lifetime),
                ),
                (true, false) => format!("idle_timeout=~{}", format_duration_ms(idle_timeout)),
                (false, true) => format!("lifetime=~{}", format_duration_ms(lifetime)),
                (false, false) => "no limits configured".to_string(),
            };
            info!(
                "[{}@{}] closed {} idle server{}: expired ({})",
                self.address.username,
                self.address.pool_name,
                closed,
                if closed == 1 { "" } else { "s" },
                limits,
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
                "[{}@{}] drained {} idle server{}",
                self.address.username,
                self.address.pool_name,
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
        "Retain task started: interval={}, max_per_cycle={}",
        format_elapsed(retain_time),
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
                    "[{}@{}] prewarmed {} server{} (min_pool_size={})",
                    pool.address.username,
                    pool.address.pool_name,
                    created,
                    if created == 1 { "" } else { "s" },
                    min,
                );
            } else {
                warn!(
                    "[{}@{}] prewarm failed (min_pool_size={})",
                    pool.address.username, pool.address.pool_name, min,
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

        // Reserve pressure relief runs in two steps, both off the hot path.
        //
        // Step 1 — upgrade: if any backend in this pool still holds a
        // reserve permit while the coordinator's main semaphore has
        // headroom, swap the accounting so the reserve slot is freed
        // without closing the backend. This fixes the case where a past
        // burst left reserve permits pinned to idle backends, making
        // `reserve_used` misrepresent actual burst buffer availability.
        //
        // Step 2 — close stale: for reserve backends that could not be
        // upgraded (main is still full) AND have been idle longer than
        // `min_connection_lifetime_ms`, close them the old way. These
        // two steps together guarantee that `reserve_used` converges to
        // the number of reserve permits actually defending against live
        // pressure, not to the number of historical grants.
        //
        // Pools currently under client pressure are skipped: closing a
        // reserve connection in front of a queued client just forces a
        // connect() onto the wait path. Reserve cleanup runs again next
        // cycle.
        for pool in &pool_refs {
            if pool.database.under_pressure() {
                continue;
            }
            if let Some(ref coordinator) = pool.coordinator {
                let upgraded = pool.database.upgrade_reserve_to_main();
                if upgraded > 0 {
                    info!(
                        "[{}@{}] upgraded {} reserve permit{} to main \
                         (main has headroom)",
                        pool.address.username,
                        pool.address.pool_name,
                        upgraded,
                        if upgraded == 1 { "" } else { "s" },
                    );
                }
                let min_lifetime = coordinator.config().min_connection_lifetime_ms;
                let closed = pool.database.close_idle_reserve_connections(min_lifetime);
                if closed > 0 {
                    info!(
                        "[{}@{}] released {} reserve server{} (idle > {})",
                        pool.address.username,
                        pool.address.pool_name,
                        closed,
                        if closed == 1 { "" } else { "s" },
                        format_duration_ms(min_lifetime),
                    );
                }
            }
        }

        // Idle / lifetime trimming. Pools under client pressure are skipped
        // inside `retain_pool_connections` itself.
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
                        let prev_failures = pool.replenish_failures.swap(0, Ordering::Relaxed);
                        if prev_failures > 0 {
                            info!(
                                "[{}@{}] replenish recovered after {} failure{}: created {} server{} (min_pool_size={})",
                                pool.address.username, pool.address.pool_name,
                                prev_failures,
                                if prev_failures == 1 { "" } else { "s" },
                                created,
                                if created == 1 { "" } else { "s" },
                                min,
                            );
                        } else {
                            info!(
                                "[{}@{}] replenished {} server{} (min_pool_size={})",
                                pool.address.username,
                                pool.address.pool_name,
                                created,
                                if created == 1 { "" } else { "s" },
                                min,
                            );
                        }
                    } else {
                        let failures = pool.replenish_failures.fetch_add(1, Ordering::Relaxed) + 1;
                        if failures == 1 {
                            warn!(
                                "[{}@{}] replenish failed (deficit={}, min_pool_size={})",
                                pool.address.username, pool.address.pool_name, deficit, min,
                            );
                        } else if failures % 20 == 0 {
                            warn!(
                                "[{}@{}] replenish still failing: {} consecutive failures (deficit={}, min_pool_size={})",
                                pool.address.username, pool.address.pool_name,
                                failures, deficit, min,
                            );
                        }
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
            "Graceful shutdown: drained {} idle server{} from all pools",
            total_drained,
            if total_drained == 1 { "" } else { "s" }
        );
    }
    total_drained
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use crate::config::{Address, PoolMode, User};
    use crate::pool::{Pool, PoolSettings, ServerParameters, ServerPool};
    use dashmap::DashMap;

    fn build_test_connection_pool() -> ConnectionPool {
        let server_pool = ServerPool::new(
            Address::default(),
            User::default(),
            "test_db",
            Arc::new(DashMap::new()),
            false,
            false,
            0,
            "test_app".to_string(),
            1,
            60_000,
            60_000,
            60_000,
            Duration::from_secs(5),
            false,
            None,
        );
        let database = Pool::builder(server_pool)
            .pool_name("test_db".to_string())
            .username("test_user".to_string())
            .build();

        ConnectionPool {
            database,
            address: Address::default(),
            original_server_parameters: Arc::new(tokio::sync::Mutex::new(ServerParameters::new())),
            settings: PoolSettings {
                pool_mode: PoolMode::Transaction,
                user: User::default(),
                db: "test_db".to_string(),
                idle_timeout_ms: 60_000,
                life_time_ms: 1, // tiny: any connection would be "expired"
                sync_server_parameters: false,
                min_guaranteed_pool_size: 0,
            },
            config_hash: 0,
            prepared_statement_cache: None,
            coordinator: None,
            replenish_failures: Arc::new(AtomicU32::new(0)),
            created_at: std::time::Instant::now(),
        }
    }

    /// Pools serving live traffic must not lose idle connections to retain
    /// trimming. The whole point of the skip is to make sure that an idle
    /// connection a queued client is about to grab is not closed for
    /// housekeeping reasons one tick before. Drain the semaphore (models
    /// "every permit is in flight"), call retain, and assert no closures.
    #[tokio::test]
    async fn retain_pool_skips_under_pressure() {
        let conn_pool = build_test_connection_pool();

        let semaphore = conn_pool.database.semaphore();
        let total_permits = semaphore.available_permits();
        let mut held = Vec::with_capacity(total_permits);
        for _ in 0..total_permits {
            held.push(semaphore.acquire().await.unwrap());
        }
        assert!(
            conn_pool.database.under_pressure(),
            "test setup must put the pool under pressure",
        );

        let count = Arc::new(AtomicUsize::new(0));
        let closed = conn_pool.retain_pool_connections(count.clone(), 0);

        assert_eq!(
            closed, 0,
            "retain must close zero connections under pressure"
        );
        assert_eq!(
            count.load(Ordering::Relaxed),
            0,
            "shared retain counter must not advance",
        );
    }
}
