//! Dynamic pool creation for auth_query passthrough mode.
//!
//! When a client authenticates via `auth_query` in passthrough mode (no `server_user`),
//! pg_doorman creates a per-user pool on the fly. These pools are tracked in `DYNAMIC_POOLS`
//! and garbage-collected when idle. On RELOAD, dynamic pools are dropped and recreated
//! on the next client connection with fresh settings.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use log::{debug, info, warn};

use crate::config::{get_config, BackendAuthMethod, PoolMode, User};
use crate::errors::Error;
use crate::server::ServerParameters;
use crate::stats::AddressStats;

use super::types::{PoolConfig, QueueMode, Timeouts};
use super::{
    build_server_tls_for_pool, get_auth_query_state, get_coordinator, get_pool,
    register_dynamic_pool, resolve_server_cache_size, Address, CheckQueryCache, ConnectionPool,
    Pool, PoolIdentifier, PoolSettings, PreparedStatementCache, ServerPool, POOLS,
};

/// Create a dynamic data pool for auth_query passthrough mode.
/// Returns the new (or existing) pool. Race-safe: if another thread
/// created the pool concurrently, returns the existing one.
///
/// On RELOAD, dynamic pools are dropped (not in config) and recreated
/// on the next client connection with fresh settings.
/// `fetched_overlay` is the per-user `startup_parameters` map from the
/// auth_query row that authenticated this user. Passing it in ties pool
/// creation to that row instead of reading the cache again.
pub fn create_dynamic_pool(
    pool_name: &str,
    username: &str,
    backend_auth: Option<BackendAuthMethod>,
    fetched_overlay: Arc<std::collections::HashMap<String, String>>,
    fetched_overlay_hash: u64,
) -> Result<(ConnectionPool, super::PoolInitGuard), Error> {
    // Fast path: pool already exists. The cache-side refetch path
    // already drops the live pool when an auth_query refetch changes
    // the overlay (see `drop_dynamic_pool_if_overlay_drifted`), but a
    // concurrent login can still arrive after the cache published the
    // fresh entry yet before the drop fires, or with a fetched_overlay
    // newer than what the live pool was frozen with. Check the overlay
    // hash here too so that login rebuilds the pool against the
    // current snapshot instead of inheriting a stale one. The hash is
    // precomputed on `CacheEntry`, so the fast path skips the sort +
    // SipHash on every login.
    if let Some(existing) = get_pool(pool_name, username) {
        let identifier = super::PoolIdentifier::new(pool_name, username);
        let live_hash = existing.per_user_startup_overlay_hash;
        let is_dyn = super::is_dynamic_pool(&identifier);
        if !should_rebuild_for_overlay_drift(live_hash, fetched_overlay_hash, is_dyn) {
            // Hash matches, or the live pool is static and the empty
            // baseline does not match an auth_query overlay — either
            // way the existing pool wins. Refresh `backend_auth` only
            // on hash match: a password rotation between cache
            // fetches still applies, but a static pool is left alone.
            if live_hash == fetched_overlay_hash {
                if let (Some(ref ba_lock), Some(new_ba)) =
                    (&existing.address.backend_auth, &backend_auth)
                {
                    debug!(
                        "[{username}@{pool_name}] auth_query: dynamic pool already exists, updating backend_auth"
                    );
                    *ba_lock.write() = new_ba.clone();
                }
            }
            return Ok((existing, super::PoolInitGuard::already_committed()));
        }
        if super::drop_dynamic_pool(&identifier) {
            info!(
                "[{username}@{pool_name}] auth_query: per-user startup_parameters overlay drift on login — dynamic pool dropped, rebuilding"
            );
        }
    }

    let config = get_config();
    let pool_config = config.pools.get(pool_name).ok_or_else(|| {
        Error::AuthError(format!(
            "auth_query: pool config '{pool_name}' not found for dynamic pool"
        ))
    })?;
    let aq_config = pool_config.auth_query.as_ref().ok_or_else(|| {
        Error::AuthError(format!(
            "auth_query: config not found in pool '{pool_name}' for dynamic pool"
        ))
    })?;
    let client_server_map = super::get_client_server_map()
        .ok_or_else(|| Error::AuthError("auth_query: client_server_map not initialized".into()))?;

    let server_database = pool_config
        .server_database
        .clone()
        .unwrap_or_else(|| pool_name.to_string());

    let ba_arc = backend_auth.map(|ba| Arc::new(parking_lot::RwLock::new(ba)));
    debug!(
        "[{username}@{pool_name}] building server TLS config (mode={})",
        pool_config
            .server_tls_mode
            .as_deref()
            .unwrap_or(&config.general.server_tls_mode)
    );
    let server_tls = build_server_tls_for_pool(pool_config, &config.general)?;

    let address = Address {
        database: pool_name.to_string(),
        host: pool_config.server_host.clone(),
        port: pool_config.server_port,
        username: username.to_string(),
        password: String::new(),
        pool_name: pool_name.to_string(),
        stats: Arc::new(AddressStats::default()),
        backend_auth: ba_arc,
        server_tls,
    };

    let user = User {
        username: username.to_string(),
        password: String::new(),
        pool_size: aq_config.pool_size,
        min_pool_size: if aq_config.min_pool_size > 0 {
            Some(aq_config.min_pool_size)
        } else {
            None
        },
        server_username: Some(username.to_string()),
        server_password: None,
        ..Default::default()
    };

    let prepared_statements_cache_size = match config.general.prepared_statements {
        true => pool_config
            .prepared_statements_cache_size
            .unwrap_or(config.general.prepared_statements_cache_size),
        false => 0,
    };

    let server_prepared_statements_cache_size = resolve_server_cache_size(
        prepared_statements_cache_size,
        pool_config.server_prepared_statements_cache_size,
        config.general.server_prepared_statements_cache_size,
    );

    let application_name = pool_config
        .application_name
        .clone()
        .unwrap_or_else(|| "pg_doorman".to_string());

    let pool_mode = user.pool_mode.unwrap_or(pool_config.pool_mode);

    let fallback_state = super::build_fallback_state(pool_name, pool_config, &config.general);

    // Merge general+pool startup_parameters baseline from the same config
    // snapshot. Dynamic auth_query pools follow the same lifecycle as
    // static pools: rebuilt on RELOAD when the underlying base changes
    // (see `general_startup_parameters_changed` in pool/mod.rs).
    let base_startup_parameters = std::sync::Arc::new(
        crate::config::startup_parameters::cascade_canonical_keys(&[
            &config.general.startup_parameters,
            &pool_config.startup_parameters,
        ]),
    );

    // Convert the caller's HashMap snapshot into the BTreeMap shape
    // ServerPool stores. The snapshot comes from the auth_query row used
    // for this login, so TTL expiry or an interleaved refetch cannot
    // change the overlay while the pool is created. Dedicated-mode pools
    // should not reach this path, but keep the guard so a future caller
    // cannot attach a per-user overlay to a shared backend pool.
    let per_user_startup_overlay: std::sync::Arc<std::collections::BTreeMap<String, String>> = {
        let is_dedicated = super::get_auth_query_state(pool_name)
            .map(|state| state.config.is_dedicated_mode())
            .unwrap_or(false);
        if is_dedicated || fetched_overlay.is_empty() {
            std::sync::Arc::new(std::collections::BTreeMap::new())
        } else {
            let map: std::collections::BTreeMap<String, String> = fetched_overlay
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            std::sync::Arc::new(map)
        }
    };

    let manager = ServerPool::new(
        address.clone(),
        user.clone(),
        server_database.as_str(),
        client_server_map,
        pool_config.cleanup_server_connections,
        pool_config.log_client_parameter_status_changes,
        server_prepared_statements_cache_size,
        application_name,
        config.general.max_concurrent_creates,
        pool_config
            .server_lifetime
            .unwrap_or(config.general.server_lifetime.as_millis()),
        pool_config
            .idle_timeout
            .unwrap_or(config.general.idle_timeout.as_millis()),
        config.general.server_idle_check_timeout.as_millis(),
        config.general.connect_timeout.as_std(),
        config.general.query_wait_timeout.as_std(),
        pool_mode == PoolMode::Session,
        fallback_state,
        base_startup_parameters,
        per_user_startup_overlay.clone(),
    );

    // The auth_query cache compares the new fetched per-user map against
    // this value after every refetch; a mismatch drops the dynamic pool
    // so the next connect rebuilds with the new reset_val. The caller
    // already has the hash precomputed on the `CacheEntry`, so we reuse
    // it instead of re-running per_user_overlay_hash on the same map.
    let overlay_hash = fetched_overlay_hash;

    let queue_strategy = match config.general.server_round_robin {
        true => QueueMode::Fifo,
        false => QueueMode::Lifo,
    };

    let pool = Pool::builder(manager)
        .coordinator(get_coordinator(pool_name))
        .pool_name(pool_name.to_string())
        .username(username.to_string())
        .config(PoolConfig {
            max_size: user.pool_size as usize,
            timeouts: Timeouts {
                wait: Some(config.general.query_wait_timeout.as_std()),
                create: Some(config.general.connect_timeout.as_std()),
                recycle: None,
            },
            queue_mode: queue_strategy,
            scaling: pool_config.resolve_scaling_config(&config.general),
        })
        .build();

    let conn_pool = ConnectionPool {
        database: pool,
        address,
        config_hash: 0, // dynamic pools don't participate in hash-based reload
        per_user_startup_overlay_hash: overlay_hash,
        original_server_parameters: Arc::new(tokio::sync::Mutex::new(ServerParameters::new())),
        settings: PoolSettings {
            pool_mode,
            user,
            db: pool_name.to_string(),
            idle_timeout_ms: pool_config
                .idle_timeout
                .unwrap_or(config.general.idle_timeout.as_millis()),
            life_time_ms: pool_config
                .server_lifetime
                .unwrap_or(config.general.server_lifetime.as_millis()),
            sync_server_parameters: config.general.sync_server_parameters,
            min_guaranteed_pool_size: pool_config.min_guaranteed_pool_size.unwrap_or(0),
        },
        prepared_statement_cache: match config.general.prepared_statements {
            false => None,
            true => Some(Arc::new(PreparedStatementCache::new(
                prepared_statements_cache_size,
                config.general.worker_threads,
            ))),
        },
        check_query_cache: Arc::new(CheckQueryCache::new()),
        coordinator: get_coordinator(pool_name),
        replenish_failures: Arc::new(AtomicU32::new(0)),
        init_complete: Arc::new(AtomicBool::new(false)),
    };

    // Atomic insert into POOLS
    let identifier = PoolIdentifier::new(pool_name, username);
    let current = POOLS.load();
    let mut new_pools = (**current).clone();

    // Re-check after clone (another thread may have created it). The
    // fast path at the top of this function already validates the
    // overlay hash; do the same here so the slow path doesn't reuse a
    // pool another login built with a stale `startup_parameters`
    // snapshot. Without this compare, two concurrent logins after an
    // auth_query row update can race: one wins the slow path with the
    // new overlay, the other finds the loser's `existing` and inherits
    // the stale `reset_val` until TTL or RELOAD.
    if let Some(existing) = new_pools.get(&identifier) {
        let live_hash = existing.per_user_startup_overlay_hash;
        let is_dyn = super::is_dynamic_pool(&identifier);
        if !should_rebuild_for_overlay_drift(live_hash, overlay_hash, is_dyn) {
            // Same reasoning as the fast path: refresh backend_auth
            // only when the live pool is a hash-matching dynamic. A
            // static pool registered concurrently with the in-flight
            // dynamic-pool build is preserved unchanged.
            if live_hash == overlay_hash {
                if let (Some(ref ba_lock), Some(ref new_ba)) = (
                    &existing.address.backend_auth,
                    &conn_pool.address.backend_auth,
                ) {
                    *ba_lock.write() = new_ba.read().clone();
                }
            }
            return Ok((existing.clone(), super::PoolInitGuard::already_committed()));
        }
        info!(
            "[{username}@{pool_name}] auth_query: per-user startup_parameters overlay drift on slow-path race — replacing concurrently-built pool"
        );
        new_pools.remove(&identifier);
    }

    let auth_method = match &conn_pool.address.backend_auth {
        Some(ba) => {
            let guard = ba.read();
            match &*guard {
                BackendAuthMethod::Md5PassTheHash(_) => "md5-pass-the-hash",
                BackendAuthMethod::ScramPassthrough(_) => "scram-passthrough",
                BackendAuthMethod::ScramPending => "scram-pending",
            }
        }
        None => "none",
    };
    info!("[{username}@{pool_name}] dynamic pool created (backend_auth={auth_method})");
    new_pools.insert(identifier.clone(), conn_pool.clone());
    POOLS.store(Arc::new(new_pools));
    register_dynamic_pool(&identifier);

    // Prewarm: spawn background task to create min_pool_size connections
    if aq_config.min_pool_size > 0 {
        let pool_clone = conn_pool.clone();
        let min = aq_config.min_pool_size as usize;
        let pn = pool_name.to_string();
        let un = username.to_string();
        tokio::spawn(async move {
            let created = pool_clone.database.replenish(min).await;
            if created > 0 {
                info!("[{un}@{pn}] prewarmed {created} dynamic server(s) (min_pool_size={min})");
            } else {
                warn!("[{un}@{pn}] dynamic prewarm failed: 0 of {min} connections created");
            }
        });
    }

    // Increment dynamic_pools_created stat
    if let Some(state) = get_auth_query_state(pool_name) {
        state
            .stats
            .dynamic_pools_created
            .fetch_add(1, Ordering::Relaxed);
    }

    let guard =
        super::PoolInitGuard::for_new_pool(identifier, Arc::clone(&conn_pool.init_complete));
    Ok((conn_pool, guard))
}

/// Decide whether `create_dynamic_pool` should replace an existing
/// `(pool, user)` entry in `POOLS`. Hash drift alone is not enough —
/// a static pool registered for the same identifier (during a config
/// reload race with an in-flight auth_query login) keeps the empty
/// overlay hash, and replacing it would silently swap the operator's
/// configured backend auth/startup-parameters for the auth_query
/// passthrough version. Rebuild only when the live pool is dynamic.
fn should_rebuild_for_overlay_drift(live_hash: u64, fetched_hash: u64, is_dynamic: bool) -> bool {
    live_hash != fetched_hash && is_dynamic
}

#[cfg(test)]
mod tests {
    use super::should_rebuild_for_overlay_drift;

    #[test]
    fn overlay_drift_reuses_on_hash_match() {
        let h = 0x1234_5678_9abc_def0_u64;
        assert!(!should_rebuild_for_overlay_drift(h, h, true));
        assert!(!should_rebuild_for_overlay_drift(h, h, false));
    }

    #[test]
    fn overlay_drift_rebuilds_dynamic_on_hash_mismatch() {
        assert!(should_rebuild_for_overlay_drift(0xAAAA, 0xBBBB, true));
    }

    #[test]
    fn overlay_drift_preserves_static_on_hash_mismatch() {
        // A static pool registered during reload races with an
        // in-flight auth_query login that fetched a non-empty overlay.
        // The live pool's hash is `empty_overlay_hash()`; the fetched
        // hash is non-empty. Static-overrides-dynamic must hold, so
        // the existing pool wins and is not replaced.
        let empty = crate::pool::empty_overlay_hash();
        let fetched = 0xBEEF_0000_0000_0001_u64;
        assert_ne!(empty, fetched);
        assert!(!should_rebuild_for_overlay_drift(
            empty, fetched, /*is_dynamic=*/ false
        ));
    }
}
