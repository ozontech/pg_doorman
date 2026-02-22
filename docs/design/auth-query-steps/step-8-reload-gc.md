# Step 8: RELOAD + idle pool GC

## Goal

Handle config RELOAD for dynamic pools (Decision from Problem 4: destroy all
dynamic pools on auth_query config change). Implement idle pool garbage
collection for passthrough mode.

## Dependencies

- Step 6 (dynamic pools exist)

## 8.1 Track dynamic pools

### File: `src/pool/mod.rs`

Need to distinguish dynamic pools from static pools during RELOAD.
Add a global set of dynamic pool identifiers:

```rust
/// Set of dynamically created pool identifiers (auth_query passthrough mode).
/// Used during RELOAD to identify which pools to destroy.
pub static DYNAMIC_POOLS: Lazy<ArcSwap<HashSet<PoolIdentifier>>> =
    Lazy::new(|| ArcSwap::from_pointee(HashSet::new()));

/// Register a pool as dynamic (created by auth_query).
pub fn register_dynamic_pool(id: PoolIdentifier) {
    let pools = DYNAMIC_POOLS.load();
    let mut new_set = (**pools).clone();
    new_set.insert(id);
    DYNAMIC_POOLS.store(Arc::new(new_set));
}

/// Check if a pool is dynamic.
pub fn is_dynamic_pool(id: &PoolIdentifier) -> bool {
    DYNAMIC_POOLS.load().contains(id)
}
```

Call `register_dynamic_pool()` in `create_dynamic_pool()` after insertion.

## 8.2 RELOAD handling

### File: `src/pool/mod.rs` — in `ConnectionPool::from_config()`

After creating new static pools and new auth_query states, compare auth_query
configs to detect changes:

```rust
// In from_config(), before POOLS.store():

// Detect auth_query config changes
let old_aq_states = AUTH_QUERY_STATE.load();
let mut pools_to_remove: Vec<PoolIdentifier> = Vec::new();

for (pool_name, old_state) in old_aq_states.iter() {
    let new_aq = config.pools.get(pool_name)
        .and_then(|p| p.auth_query.as_ref());

    let config_changed = match new_aq {
        None => true, // auth_query removed
        Some(new) => new != &old_state.config, // auth_query changed
    };

    if config_changed {
        info!("[pool: {pool_name}] auth_query config changed — destroying all dynamic pools");

        // Collect dynamic pools for this database
        for id in DYNAMIC_POOLS.load().iter() {
            if id.db == *pool_name {
                pools_to_remove.push(id.clone());
            }
        }

        // Clear cache
        old_state.cache.clear();
    }
}

// Also check: static user added that matches dynamic pool → destroy dynamic
for (pool_name, pool_config) in &config.pools {
    for user in &pool_config.users {
        let id = PoolIdentifier::new(pool_name, &user.username);
        if is_dynamic_pool(&id) {
            info!(
                "Static user '{}' added for pool '{}' — destroying dynamic pool",
                user.username, pool_name
            );
            pools_to_remove.push(id);
        }
    }
}

// Remove dynamic pools from new_pools map
for id in &pools_to_remove {
    new_pools.remove(id);
}

// Update DYNAMIC_POOLS — remove destroyed ones
let mut new_dynamic = (**DYNAMIC_POOLS.load()).clone();
for id in &pools_to_remove {
    new_dynamic.remove(id);
}
DYNAMIC_POOLS.store(Arc::new(new_dynamic));

POOLS.store(Arc::new(new_pools));
```

## 8.3 Idle pool garbage collection

For passthrough mode, dynamic pools accumulate over time. Need periodic cleanup
of idle pools.

### File: `src/pool/mod.rs` (or new `src/pool/gc.rs`)

```rust
/// Spawn a background task that periodically checks dynamic pools for idleness.
/// A dynamic pool is "idle" if it has 0 active + 0 waiting connections for
/// longer than idle_timeout.
pub fn spawn_dynamic_pool_gc(interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            gc_idle_dynamic_pools().await;
        }
    });
}

async fn gc_idle_dynamic_pools() {
    let dynamic_ids: Vec<PoolIdentifier> = DYNAMIC_POOLS.load().iter().cloned().collect();

    if dynamic_ids.is_empty() {
        return;
    }

    let pools = POOLS.load();
    let mut to_remove = Vec::new();

    for id in &dynamic_ids {
        if let Some(pool) = pools.get(id) {
            let status = pool.pool_state();
            // Pool is idle if no connections are active and no clients waiting
            if status.size == 0 || (status.available == status.size && status.waiting == 0) {
                // Check how long it's been idle
                // Need to track last_activity timestamp on the pool
                // For now: just check if pool is completely empty
                if status.size == 0 {
                    to_remove.push(id.clone());
                }
            }
        } else {
            // Pool no longer in POOLS map — clean up tracking
            to_remove.push(id.clone());
        }
    }

    if to_remove.is_empty() {
        return;
    }

    info!("GC: removing {} idle dynamic pools", to_remove.len());

    let mut new_pools = (**POOLS.load()).clone();
    let mut new_dynamic = (**DYNAMIC_POOLS.load()).clone();

    for id in &to_remove {
        new_pools.remove(id);
        new_dynamic.remove(id);
        debug!("GC: removed dynamic pool {}", id);
    }

    POOLS.store(Arc::new(new_pools));
    DYNAMIC_POOLS.store(Arc::new(new_dynamic));
}
```

**Note:** Idle detection is approximate. The pool's internal connections have
their own `idle_timeout` which causes them to disconnect. Once all connections
in a pool are closed and no new clients arrive, the pool becomes empty and
GC removes the metadata.

Start GC task in `run_server()` if any pool has `auth_query` without `server_user`.

## 8.4 BDD tests

```gherkin
@auth-query @reload
Scenario: RELOAD removes auth_query — dynamic pools destroyed
  Given auth_query configured, "alice" has dynamic pool
  When auth_query section removed from config and RELOAD executed
  Then "alice" dynamic pool is destroyed
  And new connections as "alice" are rejected

@auth-query @reload
Scenario: RELOAD adds static user matching dynamic — static wins
  Given "alice" has dynamic pool via auth_query
  When static user "alice" added to config and RELOAD executed
  Then "alice" dynamic pool destroyed
  And "alice" uses static authentication

@auth-query @reload
Scenario: RELOAD changes server_user — mode change
  Given auth_query in passthrough mode, "alice" and "bob" have pools
  When server_user added to auth_query and RELOAD executed
  Then all dynamic pools destroyed
  And cache cleared
  And new connections use shared pool mode

@auth-query @gc
Scenario: Idle dynamic pool GC
  Given dynamic pool "alice" in passthrough mode
  When no client uses "alice" for longer than idle_timeout
  Then "alice" pool is garbage collected
  When "alice" connects again
  Then new pool is created
```

## Checklist

- [ ] `DYNAMIC_POOLS` tracking set
- [ ] `register_dynamic_pool()` / `is_dynamic_pool()`
- [ ] RELOAD: detect auth_query config changes
- [ ] RELOAD: destroy dynamic pools on change
- [ ] RELOAD: handle static user overriding dynamic
- [ ] Idle pool GC background task
- [ ] Start GC when passthrough mode is active
- [ ] BDD tests (4)
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
