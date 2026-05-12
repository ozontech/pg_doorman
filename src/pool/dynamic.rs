//! Dynamic pool creation for auth_query passthrough mode.
//!
//! When a client authenticates via `auth_query` in passthrough mode (no `server_user`),
//! pg_doorman creates a per-user pool on the fly. These pools are tracked in `DYNAMIC_POOLS`
//! and garbage-collected when idle. On RELOAD, dynamic pools are dropped and recreated
//! on the next client connection with fresh settings.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use log::{debug, info, warn};

use crate::config::{get_config, BackendAuthMethod, PoolMode, User};
use crate::errors::Error;
use crate::server::ServerParameters;
use crate::stats::AddressStats;

use super::types::{PoolConfig, QueueMode, Timeouts};
use super::{
    build_server_tls_for_pool, get_auth_query_state, get_coordinator, get_pool,
    register_dynamic_pool, resolve_server_cache_size, Address, ConnectionPool, Pool,
    PoolIdentifier, PoolSettings, PreparedStatementCache, ServerPool, POOLS,
};

/// Create a dynamic data pool for auth_query passthrough mode.
/// Returns the new (or existing) pool. Race-safe: if another thread
/// created the pool concurrently, returns the existing one.
///
/// On RELOAD, dynamic pools are dropped (not in config) and recreated
/// on the next client connection with fresh settings.
/// `fetched_overlay` is the exact per-user `startup_parameters` map the
/// caller just fetched from `AuthQueryCache`. Passing it in avoids the
/// race where this function would re-peek the cache and see a different
/// (or missing) overlay under low TTLs or concurrent refetches.
pub fn create_dynamic_pool(
    pool_name: &str,
    username: &str,
    backend_auth: Option<BackendAuthMethod>,
    fetched_overlay: Arc<std::collections::HashMap<String, String>>,
) -> Result<ConnectionPool, Error> {
    // Fast path: pool already exists
    if let Some(existing) = get_pool(pool_name, username) {
        // Update backend_auth (credentials may have changed)
        if let (Some(ref ba_lock), Some(new_ba)) = (&existing.address.backend_auth, &backend_auth) {
            debug!(
                "[{username}@{pool_name}] auth_query: dynamic pool already exists, updating backend_auth"
            );
            *ba_lock.write() = new_ba.clone();
        }
        return Ok(existing);
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
    let base_startup_parameters = {
        let mut merged: std::collections::BTreeMap<String, String> =
            config.general.startup_parameters.clone();
        for (k, v) in &pool_config.startup_parameters {
            merged.insert(k.clone(), v.clone());
        }
        std::sync::Arc::new(merged)
    };

    // Convert the caller's HashMap snapshot (the same one freshly
    // returned by `cache.get_or_fetch(username)` in auth/mod.rs) into
    // the BTreeMap shape ServerPool stores. The caller owns the
    // snapshot, so there is no re-peek of the global cache here —
    // an interleaved refetch cannot swap a different overlay under us.
    // Dedicated-mode pools never reach this path (auth/mod.rs uses the
    // shared pool branch instead), but the filter stays for defence-
    // in-depth in case a future caller forwards a non-empty overlay
    // through the dedicated-mode branch by accident.
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

    // Snapshot the overlay hash before the Arc moves into ServerPool.
    // The auth_query cache compares the new fetched per-user map against
    // this value after every refetch; a mismatch drops the dynamic pool
    // so the next connect rebuilds with the new reset_val.
    let overlay_hash = super::per_user_overlay_hash(per_user_startup_overlay.iter());

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
        coordinator: get_coordinator(pool_name),
        replenish_failures: Arc::new(AtomicU32::new(0)),
        created_at: std::time::Instant::now(),
    };

    // Atomic insert into POOLS
    let identifier = PoolIdentifier::new(pool_name, username);
    let current = POOLS.load();
    let mut new_pools = (**current).clone();

    // Re-check after clone (another thread may have created it)
    if let Some(existing) = new_pools.get(&identifier) {
        if let (Some(ref ba_lock), Some(ref new_ba)) = (
            &existing.address.backend_auth,
            &conn_pool.address.backend_auth,
        ) {
            *ba_lock.write() = new_ba.read().clone();
        }
        return Ok(existing.clone());
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

    Ok(conn_pool)
}
