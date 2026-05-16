use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info};

use super::{ConnectionPool, PoolIdentifier, AUTH_QUERY_STATE, DYNAMIC_POOLS, POOLS};

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
                if !should_gc_idle_pool(pool, id) {
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

/// Decide whether an idle dynamic pool (`pool_state().size == 0`) is
/// eligible for removal during this GC sweep. A pool is kept when it is
/// paused (admin control), when it has a `min_pool_size` (retain cycle
/// is responsible), or when its first server connection is still being
/// established (`init_complete == false`). The last case is the race
/// fix for issue #209 — `PoolInitGuard::commit` is what flips the flag
/// to `true` after `get_server_parameters` succeeds, and any guard
/// dropped without `commit` has already removed the pool entry by the
/// time the next sweep observes the map.
fn should_gc_idle_pool(pool: &ConnectionPool, id: &PoolIdentifier) -> bool {
    if pool.database.is_paused() {
        debug!("[{id}] GC: paused, skipping");
        return false;
    }
    if pool.settings.user.min_pool_size.unwrap_or(0) > 0 {
        debug!("[{id}] GC: min_pool_size > 0, skipping despite size=0");
        return false;
    }
    if !pool.init_complete.load(Ordering::Acquire) {
        debug!("[{id}] GC: init not complete, skipping");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Address, PoolMode, User};
    use crate::pool::{CheckQueryCache, Pool, PoolSettings, ServerParameters, ServerPool};
    use dashmap::DashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32};

    fn pool(init_complete: bool, min_pool_size: u32) -> ConnectionPool {
        let server_pool = ServerPool::new(
            Address::default(),
            User {
                min_pool_size: if min_pool_size == 0 {
                    None
                } else {
                    Some(min_pool_size)
                },
                ..User::default()
            },
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
            Duration::from_secs(5),
            false,
            None,
            Arc::new(std::collections::BTreeMap::new()),
            Arc::new(std::collections::BTreeMap::new()),
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
                user: User {
                    min_pool_size: if min_pool_size == 0 {
                        None
                    } else {
                        Some(min_pool_size)
                    },
                    ..User::default()
                },
                db: "test_db".to_string(),
                idle_timeout_ms: 60_000,
                life_time_ms: 60_000,
                sync_server_parameters: false,
                min_guaranteed_pool_size: 0,
            },
            config_hash: 0,
            per_user_startup_overlay_hash: crate::pool::empty_overlay_hash(),
            prepared_statement_cache: None,
            check_query_cache: Arc::new(CheckQueryCache::new()),
            coordinator: None,
            replenish_failures: Arc::new(AtomicU32::new(0)),
            init_complete: Arc::new(AtomicBool::new(init_complete)),
        }
    }

    #[test]
    fn idle_pool_with_completed_init_is_collected() {
        // Regular case: pool finished initializing, drained to zero, GC should reap it.
        let id = PoolIdentifier::new("test_db", "test_user");
        let p = pool(true, 0);
        assert!(should_gc_idle_pool(&p, &id));
    }

    #[test]
    fn idle_pool_still_initializing_is_skipped() {
        // Issue #209: GC must not reap a pool whose first server connection
        // is still being established. Without this check the next login
        // observes "No pool configured" and the connection is dropped.
        let id = PoolIdentifier::new("test_db", "test_user");
        let p = pool(false, 0);
        assert!(!should_gc_idle_pool(&p, &id));
    }

    #[test]
    fn pool_with_min_pool_size_is_skipped() {
        // The retain cycle keeps `min_pool_size` connections warm; GC must
        // never compete with it on the same pool.
        let id = PoolIdentifier::new("test_db", "test_user");
        let p = pool(true, 5);
        assert!(!should_gc_idle_pool(&p, &id));
    }

    #[test]
    fn flipping_init_complete_makes_pool_eligible() {
        // Same pool object, different observable behavior before and after
        // `PoolInitGuard::commit` runs. Concretizes the contract between the
        // guard and the GC sweep.
        let id = PoolIdentifier::new("test_db", "test_user");
        let p = pool(false, 0);
        assert!(!should_gc_idle_pool(&p, &id));
        p.init_complete.store(true, Ordering::Release);
        assert!(should_gc_idle_pool(&p, &id));
    }
}
