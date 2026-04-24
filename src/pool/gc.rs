use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info};

use super::{PoolIdentifier, AUTH_QUERY_STATE, DYNAMIC_POOLS, POOLS};

/// Spawn a background task that periodically removes idle dynamic pools.
/// Dynamic pools are created by auth_query passthrough mode — one per user.
/// When all connections in a dynamic pool are closed (size == 0), the pool
/// is garbage-collected to prevent unbounded memory growth.
///
/// This is a no-op when DYNAMIC_POOLS is empty (no passthrough auth_query).
pub fn spawn_dynamic_pool_gc(interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            gc_idle_dynamic_pools();
        }
    });
}

fn gc_idle_dynamic_pools() {
    let dynamic_ids: Vec<PoolIdentifier> = DYNAMIC_POOLS.load().iter().cloned().collect();
    if dynamic_ids.is_empty() {
        return;
    }

    let pools = POOLS.load();
    let mut to_remove = Vec::new();

    for id in &dynamic_ids {
        match pools.get(id) {
            Some(pool) if pool.pool_state().size == 0 => {
                // Don't GC paused pools — they're under admin control
                if pool.database.is_paused() {
                    debug!("[{id}] GC: paused, skipping");
                    continue;
                }
                // Don't GC pools with min_pool_size — retain cycle manages them
                if pool.settings.user.min_pool_size.unwrap_or(0) > 0 {
                    debug!("[{id}] GC: min_pool_size > 0, skipping despite size=0");
                    continue;
                }
                // Grace period: allow-mode retry does two sequential TCP connects
                // (plain + TLS), each needing 1-2 RTT for handshake. Over WAN
                // this totals ~1s. 2s = 2x worst case.
                let age = pool.created_at.elapsed();
                if age < std::time::Duration::from_secs(2) {
                    debug!("[{id}] GC: pool age {age:?} < 2s, skipping");
                    continue;
                }
                debug!("[{id}] GC: 0 connections, marking for removal");
                to_remove.push(id.clone());
            }
            None => {
                debug!("[{id}] GC: stale entry (not in POOLS), removing");
                to_remove.push(id.clone());
            }
            _ => {}
        }
    }

    if to_remove.is_empty() {
        return;
    }

    // Increment dynamic_pools_destroyed stats before removal
    let aq_states = AUTH_QUERY_STATE.load();
    for id in &to_remove {
        if let Some(state) = aq_states.get(&id.db) {
            state
                .stats
                .dynamic_pools_destroyed
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    // Remove from POOLS
    let mut new_pools = (**POOLS.load()).clone();
    for id in &to_remove {
        new_pools.remove(id);
    }
    POOLS.store(Arc::new(new_pools));

    // Remove from DYNAMIC_POOLS
    let mut new_dynamic = (**DYNAMIC_POOLS.load()).clone();
    for id in &to_remove {
        new_dynamic.remove(id);
    }
    DYNAMIC_POOLS.store(Arc::new(new_dynamic));

    info!(
        "GC: removed {} idle dynamic pool(s): {}",
        to_remove.len(),
        to_remove
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
}
