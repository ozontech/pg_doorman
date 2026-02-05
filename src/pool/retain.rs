use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use log::info;

use crate::config::get_config;

use super::{get_all_pools, ConnectionPool};

impl ConnectionPool {
    /// Retain pool connections based on idle timeout and lifetime settings.
    /// Returns the number of connections closed.
    /// If `max` is 0, all expired connections will be closed (unlimited).
    /// If `max` > 0, at most `max` connections will be closed across all pools.
    pub fn retain_pool_connections(&self, count: Arc<AtomicUsize>, max: usize) -> usize {
        let status_before = self.database.status();
        let size_before = status_before.size;

        self.database.retain(|_, metrics| {
            // If max > 0, check if we've reached the limit
            if max > 0 && count.load(Ordering::Relaxed) >= max {
                return true;
            }
            if let Some(v) = metrics.recycled {
                if (v.elapsed().as_millis() as u64) > self.settings.idle_timeout_ms {
                    count.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
            }
            if (metrics.age().as_millis() as u64) > self.settings.life_time_ms {
                count.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            true
        });

        let status_after = self.database.status();
        let closed = size_before.saturating_sub(status_after.size);

        if closed > 0 {
            info!(
                "[pool: {}][user: {}] closed {} idle connection{} (idle_timeout: {}ms, lifetime: {}ms)",
                self.address.pool_name,
                self.address.username,
                closed,
                if closed == 1 { "" } else { "s" },
                self.settings.idle_timeout_ms,
                self.settings.life_time_ms,
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

    loop {
        interval.tick().await;
        for (_, pool) in get_all_pools().iter() {
            pool.retain_pool_connections(count.clone(), retain_max);
        }
        count.store(0, Ordering::Relaxed);
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
