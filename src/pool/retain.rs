use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use log::info;

use crate::config::get_config;

use super::{get_all_pools, ConnectionPool};

impl ConnectionPool {
    /// Retain pool connections based on idle timeout and lifetime settings.
    /// Returns the number of connections closed.
    /// If `max` is 0, all expired connections will be closed (unlimited).
    /// If `max` > 0, at most `max` connections will be closed across all pools,
    /// prioritizing the oldest connections first.
    pub fn retain_pool_connections(&self, count: Arc<AtomicUsize>, max: usize) -> usize {
        let idle_timeout_ms = self.settings.idle_timeout_ms;
        let life_time_ms = self.settings.life_time_ms;

        // Closure to determine if a connection should be closed
        let should_close = |_: &crate::server::Server, metrics: &crate::pool::Metrics| -> bool {
            if let Some(v) = metrics.recycled {
                if (v.elapsed().as_millis() as u64) > idle_timeout_ms {
                    return true;
                }
            }
            if (metrics.age().as_millis() as u64) > life_time_ms {
                return true;
            }
            false
        };

        // Calculate remaining quota for this pool
        let current_count = count.load(Ordering::Relaxed);
        let remaining = if max > 0 {
            max.saturating_sub(current_count)
        } else {
            0 // 0 means unlimited
        };

        // Use retain_oldest_first which sorts by age when max > 0
        let closed = self.database.retain_oldest_first(should_close, remaining);
        count.fetch_add(closed, Ordering::Relaxed);

        if closed > 0 {
            info!(
                "[pool: {}][user: {}] closed {} idle connection{} (idle_timeout: {}ms, lifetime: {}ms, oldest_first: {})",
                self.address.pool_name,
                self.address.username,
                closed,
                if closed == 1 { "" } else { "s" },
                idle_timeout_ms,
                life_time_ms,
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
