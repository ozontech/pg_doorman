use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
}

pub async fn retain_connections() {
    let retain_time_ms = get_config().general.retain_connections_time;
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(retain_time_ms));
    let count = Arc::new(AtomicUsize::new(0));
    loop {
        interval.tick().await;
        for (_, pool) in get_all_pools() {
            pool.retain_pool_connections(count.clone(), 1);
        }
        count.store(0, Ordering::Relaxed);
    }
}
