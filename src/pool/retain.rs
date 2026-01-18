use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use log::info;

use crate::config::get_config;

use super::{get_all_pools, ConnectionPool};

impl ConnectionPool {
    pub fn retain_pool_connections(&self, count: Arc<AtomicUsize>, max: usize) {
        self.database.retain(|_, metrics| {
            if count.load(Ordering::Relaxed) >= max {
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
        })
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
    let retain_time = get_config().general.retain_connections_time.as_std();
    let mut interval = tokio::time::interval(retain_time);
    let count = Arc::new(AtomicUsize::new(0));
    loop {
        interval.tick().await;
        for (_, pool) in get_all_pools().iter() {
            pool.retain_pool_connections(count.clone(), 1);
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
