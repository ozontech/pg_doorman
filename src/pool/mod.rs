use arc_swap::ArcSwap;
use dashmap::DashMap;
use log::{debug, info};
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::config::{
    get_config, tls, Address, BackendAuthMethod, General, Pool as ConfigPool, PoolMode, User,
};
use crate::errors::Error;
use crate::messages::Parse;

use crate::server::ServerParameters;
use crate::stats::auth_query::AuthQueryStats;
use crate::stats::AddressStats;

mod errors;
mod inner;
mod types;

pub use errors::{PoolError, RecycleError, RecycleResult};
pub use inner::{Object, Pool, PoolBuilder, ScalingStatsSnapshot};
pub use types::{Metrics, PoolConfig, QueueMode, ScalingConfig, Status, Timeouts};

pub use crate::server::PreparedStatementCache;

mod auth_query_state;
mod dynamic;
mod eviction;
pub mod gc;
pub mod pool_coordinator;
pub mod retain;
mod server_pool;
pub mod startup_resolver;

pub mod fallback;

pub use auth_query_state::AuthQueryState;
pub use dynamic::create_dynamic_pool;
pub use eviction::PoolEvictionSource;
pub use server_pool::ServerPool;

pub type ProcessId = i32;
pub type SecretKey = i32;
pub type ServerHost = String;
pub type ServerPort = u16;

/// Target information for forwarding a CancelRequest to the correct backend.
#[derive(Debug, Clone)]
pub struct CancelTarget {
    pub process_id: ProcessId,
    pub secret_key: SecretKey,
    pub host: ServerHost,
    pub port: ServerPort,
    pub server_tls: Arc<tls::ServerTlsConfig>,
    pub connected_with_tls: bool,
    pub pool_name: String,
}

pub type ClientServerMap = Arc<DashMap<(ProcessId, SecretKey), CancelTarget>>;
pub type PoolMap = HashMap<PoolIdentifier, ConnectionPool>;

/// The connection pool, globally available.
/// This is atomic and safe and read-optimized.
/// The pool is recreated dynamically when the config is reloaded.
pub static POOLS: Lazy<ArcSwap<PoolMap>> = Lazy::new(|| ArcSwap::from_pointee(HashMap::default()));

/// Hash of the previous reload's `general.startup_parameters` map. Used by
/// `ConnectionPool::from_config` to recognize when a SIGHUP changed the
/// general-level baseline so dynamic auth_query pools can be drained — the
/// per-pool reuse hash already folds in the baseline, but dynamic pools are
/// carried over by identifier rather than rebuilt from the same path.
static PREVIOUS_GENERAL_STARTUP_HASH: AtomicU64 = AtomicU64::new(0);
pub static CANCELED_PIDS: Lazy<Arc<Mutex<HashSet<ProcessId>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashSet::new())));

/// Per-database pool coordinators, keyed by pool name.
/// Created in `from_config()` for pools with `max_db_connections > 0`.
/// Replaced atomically on RELOAD. When a coordinator is replaced, old connections
/// that hold permits from the previous coordinator continue working until they
/// are naturally closed — the old `Arc<PoolCoordinator>` lives as long as its
/// permits do.
pub static COORDINATORS: Lazy<ArcSwap<HashMap<String, Arc<pool_coordinator::PoolCoordinator>>>> =
    Lazy::new(|| ArcSwap::from_pointee(HashMap::new()));

/// Global client-server map, initialized once by `from_config()`.
/// Needed by `create_dynamic_pool()` which doesn't have access to the map
/// through function parameters.
static CLIENT_SERVER_MAP: OnceCell<ClientServerMap> = OnceCell::new();

fn set_client_server_map(csm: ClientServerMap) {
    CLIENT_SERVER_MAP.set(csm).ok();
}

pub fn get_client_server_map() -> Option<ClientServerMap> {
    CLIENT_SERVER_MAP.get().cloned()
}

/// Stable hash of a per-user auth_query `startup_parameters` overlay.
/// Used to detect overlay drift after `auth_query` refetches: if the
/// new row's hash differs from `ConnectionPool::per_user_startup_overlay_hash`,
/// the dynamic pool is dropped so the next client connection rebuilds
/// against the new overlay. Accepts both `HashMap` (auth_query cache
/// shape) and `BTreeMap` (the immutable snapshot stored on the pool)
/// via a borrowed iterator, normalising key order so the hash is shape-
/// independent.
pub(crate) fn per_user_overlay_hash<'a, I>(entries: I) -> u64
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    use std::hash::{Hash, Hasher};
    let mut sorted: Vec<(&str, &str)> = entries
        .into_iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sorted.hash(&mut hasher);
    hasher.finish()
}

/// Hash that `per_user_overlay_hash` produces for the empty overlay.
/// Computed once and reused by every static / dedicated-mode pool so
/// drift comparisons against dynamic pools' real overlay hashes are
/// shape-stable across the codebase.
pub(crate) fn empty_overlay_hash() -> u64 {
    per_user_overlay_hash(std::iter::empty::<(&String, &String)>())
}

/// Build a `ServerTlsConfig` for a pool, merging pool-level overrides with general defaults.
pub(crate) fn build_server_tls_for_pool(
    pool_config: &ConfigPool,
    general: &General,
) -> Result<Arc<tls::ServerTlsConfig>, Error> {
    let mode_str = pool_config
        .server_tls_mode
        .as_deref()
        .unwrap_or(&general.server_tls_mode);
    let mode = mode_str.parse::<tls::ServerTlsMode>()?;

    let ca = pool_config
        .server_tls_ca_cert
        .as_ref()
        .or(general.server_tls_ca_cert.as_ref());
    let cert = pool_config
        .server_tls_certificate
        .as_ref()
        .or(general.server_tls_certificate.as_ref());
    let key = pool_config
        .server_tls_private_key
        .as_ref()
        .or(general.server_tls_private_key.as_ref());

    let config = tls::ServerTlsConfig::new(
        mode,
        ca.map(|s| std::path::Path::new(s.as_str())),
        cert.map(|s| std::path::Path::new(s.as_str())),
        key.map(|s| std::path::Path::new(s.as_str())),
    )?;

    Ok(Arc::new(config))
}

pub type PreparedStatementCacheType = Arc<PreparedStatementCache>;
pub type ServerParametersType = Arc<tokio::sync::Mutex<ServerParameters>>;

// AuthQueryState is in auth_query_state.rs, re-exported above.

/// Global auth_query state per database pool.
/// Replaced atomically on RELOAD together with POOLS.
pub static AUTH_QUERY_STATE: Lazy<ArcSwap<HashMap<String, Arc<AuthQueryState>>>> =
    Lazy::new(|| ArcSwap::from_pointee(HashMap::new()));

/// Tracks which pool identifiers were created dynamically (auth_query passthrough).
/// Used by RELOAD logic and GC to distinguish dynamic pools from static ones.
pub static DYNAMIC_POOLS: Lazy<ArcSwap<HashSet<PoolIdentifier>>> =
    Lazy::new(|| ArcSwap::from_pointee(HashSet::new()));

/// Register a pool identifier as dynamic (created by auth_query passthrough).
pub fn register_dynamic_pool(id: &PoolIdentifier) {
    let current = DYNAMIC_POOLS.load();
    if current.contains(id) {
        return;
    }
    let mut new_set = (**current).clone();
    new_set.insert(id.clone());
    DYNAMIC_POOLS.store(Arc::new(new_set));
}

/// Check if a pool identifier is a dynamic (auth_query passthrough) pool.
pub fn is_dynamic_pool(id: &PoolIdentifier) -> bool {
    DYNAMIC_POOLS.load().contains(id)
}

/// Drop a dynamic pool from `POOLS` and `DYNAMIC_POOLS`. No-op for
/// static pools — overlay drift only applies to auth_query passthrough.
/// Used by the auth_query cache after a refetch when the new per-user
/// `startup_parameters` map no longer matches the snapshot frozen in
/// the live pool: the next client connection rebuilds the dynamic pool
/// against the new overlay.
pub fn drop_dynamic_pool(id: &PoolIdentifier) -> bool {
    if !is_dynamic_pool(id) {
        return false;
    }
    let pools = POOLS.load();
    let mut new_pools = (**pools).clone();
    let removed = new_pools.remove(id).is_some();
    if removed {
        POOLS.store(Arc::new(new_pools));
    }
    let dynamics = DYNAMIC_POOLS.load();
    if dynamics.contains(id) {
        let mut new_set = (**dynamics).clone();
        new_set.remove(id);
        DYNAMIC_POOLS.store(Arc::new(new_set));
    }
    removed
}

/// Get auth_query state for a database pool.
pub fn get_auth_query_state(db: &str) -> Option<Arc<AuthQueryState>> {
    AUTH_QUERY_STATE.load().get(db).cloned()
}

/// An identifier for a PgDoorman pool.
#[derive(Hash, Debug, Clone, PartialEq, Eq, Default)]
pub struct PoolIdentifier {
    // The name of the database clients want to connect to.
    pub db: String,

    // The username the client connects with. Each user gets its own pool.
    pub user: String,
}

impl PoolIdentifier {
    /// Create a new user/pool identifier.
    pub fn new(db: &str, user: &str) -> PoolIdentifier {
        PoolIdentifier {
            db: db.to_string(),
            user: user.to_string(),
        }
    }
}

impl Display for PoolIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.user, self.db)
    }
}

impl From<&Address> for PoolIdentifier {
    fn from(address: &Address) -> PoolIdentifier {
        PoolIdentifier::new(&address.database, &address.username)
    }
}

/// Pool settings.
#[derive(Clone, Debug)]
pub struct PoolSettings {
    /// Transaction or Session.
    pub pool_mode: PoolMode,

    // Connecting user.
    pub user: User,
    pub db: String,

    /// Синхронизируем серверные параметры установленные клиентом через SET. (False).
    pub sync_server_parameters: bool,

    idle_timeout_ms: u64,
    life_time_ms: u64,

    /// Pool-level minimum connections protected from coordinator eviction.
    /// Effective protection = max(user.min_pool_size, this value).
    pub min_guaranteed_pool_size: u32,
}

impl Default for PoolSettings {
    fn default() -> PoolSettings {
        PoolSettings {
            pool_mode: PoolMode::Transaction,
            user: User::default(),
            db: String::default(),
            idle_timeout_ms: General::default_idle_timeout().as_millis(),
            life_time_ms: General::default_server_lifetime().as_millis(),
            sync_server_parameters: General::default_sync_server_parameters(),
            min_guaranteed_pool_size: 0,
        }
    }
}

/// The globally accessible connection pool.
#[derive(Clone, Debug)]
pub struct ConnectionPool {
    /// The pool.
    pub database: Pool,

    /// The address (host, port)
    pub address: Address,

    /// The server information has to be passed to the
    /// clients on startup.
    original_server_parameters: ServerParametersType,

    /// Pool configuration.
    pub settings: PoolSettings,

    /// Hash value for the pool configs. It is used to compare new configs
    /// against current config to decide whether or not we need to recreate
    /// the pool after a RELOAD command
    pub config_hash: u64,

    /// Hash of the per-user auth_query overlay frozen into this pool at
    /// creation time. After a refetch, the auth_query cache compares the
    /// new per-user startup_parameters map against this value; a mismatch
    /// drops the dynamic pool so the next client connection rebuilds
    /// against the new overlay. Static pools and dedicated-mode shared
    /// pools both pin this to the empty-map hash.
    pub per_user_startup_overlay_hash: u64,

    /// Cache
    pub prepared_statement_cache: Option<PreparedStatementCacheType>,

    /// Database-level connection coordinator. `Some` when `max_db_connections > 0`
    /// in the pool config, `None` otherwise (disabled, zero overhead).
    /// Shared across all user pools for the same database.
    pub(crate) coordinator: Option<Arc<pool_coordinator::PoolCoordinator>>,

    /// Consecutive replenish failure counter for log noise suppression.
    /// Reset to 0 on successful replenish.
    pub(crate) replenish_failures: Arc<AtomicU32>,

    /// When this pool was created. Used by GC to avoid removing
    /// dynamic pools that are still establishing their first connection.
    pub(crate) created_at: std::time::Instant,
}

impl ConnectionPool {
    /// Construct the connection pool from the configuration.
    pub async fn from_config(client_server_map: ClientServerMap) -> Result<(), Error> {
        set_client_server_map(client_server_map.clone());
        let config = get_config();

        let mut new_pools = HashMap::new();

        // Build per-database coordinators for pools with max_db_connections > 0.
        // Reuse existing coordinators when config hasn't changed (avoids resetting
        // semaphore state and losing in-flight permits on benign RELOAD).
        let mut coordinators: HashMap<String, Arc<pool_coordinator::PoolCoordinator>> =
            HashMap::new();
        let old_coordinators = COORDINATORS.load();
        for (pool_name, pool_config) in &config.pools {
            let max = pool_config.max_db_connections.unwrap_or(0) as usize;
            if max == 0 {
                continue;
            }
            let new_cfg = pool_coordinator::CoordinatorConfig {
                max_db_connections: max,
                min_connection_lifetime_ms: pool_config.min_connection_lifetime.unwrap_or(30_000),
                reserve_pool_size: pool_config.reserve_pool_size.unwrap_or(0) as usize,
                reserve_pool_timeout_ms: pool_config.reserve_pool_timeout.unwrap_or(3000),
            };
            // Reuse if config unchanged — keeps semaphores, arbiter, and in-flight permits alive.
            if let Some(existing) = old_coordinators.get(pool_name.as_str()) {
                if *existing.config() == new_cfg {
                    debug!(
                        "[pool: {}] coordinator config unchanged, reusing",
                        pool_name
                    );
                    coordinators.insert(pool_name.clone(), existing.clone());
                    continue;
                }
                info!(
                    "[pool: {}] coordinator config changed, creating new (old connections drain naturally)",
                    pool_name
                );
            } else {
                info!(
                    "[pool: {}] creating coordinator (max_db_connections={})",
                    pool_name, max
                );
            }
            coordinators.insert(
                pool_name.clone(),
                pool_coordinator::PoolCoordinator::new(pool_name.clone(), new_cfg),
            );
        }

        // Hashing each pool's effective config against (Pool, general
        // startup_parameters baseline) folds general-level GUC changes into
        // the same reuse decision pg_doorman already uses for pool-level
        // changes. Without this, a SIGHUP that only edits
        // `general.startup_parameters` would leave every idle backend
        // pinned to the previous `reset_val` until the connection rotates
        // through `lifetime_ms`, so clients would see mixed defaults from
        // the same pool depending on which backend they got.
        let general_startup_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            config.general.startup_parameters.hash(&mut hasher);
            hasher.finish()
        };
        // Load only; the hash is not advanced until the new pool map has
        // been committed at the bottom of from_config. Otherwise a reload
        // that fails halfway poisons the hash, and the next reload of the
        // *same* config silently skips the recycle of dynamic pools that
        // still carry the old reset_val.
        let previous_general_startup_hash = PREVIOUS_GENERAL_STARTUP_HASH.load(Ordering::Relaxed);
        // The static defaults to `0`, which collides with the empty-map
        // hash on a fresh process; treat that special case as "no prior
        // value" so the first reload never falsely claims a change.
        let general_startup_parameters_changed = previous_general_startup_hash != 0
            && previous_general_startup_hash != general_startup_hash;
        for (pool_name, pool_config) in &config.pools {
            let new_pool_hash_value = {
                use std::hash::Hasher;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                hasher.write_u64(pool_config.hash_value());
                hasher.write_u64(general_startup_hash);
                hasher.finish()
            };
            let server_tls_config = build_server_tls_for_pool(pool_config, &config.general)?;

            // There is one pool per database/user pair.
            for user in &pool_config.users {
                let old_pool_ref = get_pool(pool_name, &user.username);
                let identifier = PoolIdentifier::new(pool_name, &user.username);

                if let Some(pool) = old_pool_ref {
                    // If the pool hasn't changed, get existing reference and insert it into the new_pools.
                    // We replace all pools at the end, but if the reference is kept, the pool won't get re-created (bb8).
                    if pool.config_hash == new_pool_hash_value
                        && pool.address.server_tls.as_ref() == server_tls_config.as_ref()
                    {
                        info!("[{}@{}] config unchanged", user.username, pool_name);
                        new_pools.insert(identifier.clone(), pool.clone());
                        continue;
                    }
                    if pool.config_hash == new_pool_hash_value
                        && pool.address.server_tls.as_ref() != server_tls_config.as_ref()
                    {
                        info!(
                            "[{}@{}] tls certificates changed on disk, recreating pool",
                            user.username, pool_name
                        );
                    }
                }

                info!("[{}@{}] creating pool", user.username, pool_name);

                // real database name on postgresql server.
                let server_database = pool_config
                    .server_database
                    .clone()
                    .unwrap_or(pool_name.clone().to_string());

                // Detect passthrough-eligible static users:
                // server_password is None AND (server_username is None OR equals username)
                let backend_auth = if user.server_password.is_none()
                    && (user.server_username.is_none()
                        || user.server_username.as_deref() == Some(&user.username))
                {
                    if user
                        .password
                        .starts_with(crate::messages::constants::MD5_PASSWORD_PREFIX)
                    {
                        info!(
                            "[{}@{}] static passthrough: MD5 pass-the-hash",
                            user.username, pool_name
                        );
                        Some(Arc::new(RwLock::new(BackendAuthMethod::Md5PassTheHash(
                            user.password.clone(),
                        ))))
                    } else if user
                        .password
                        .starts_with(crate::messages::constants::SCRAM_SHA_256)
                    {
                        info!(
                            "[{}@{}] static passthrough: SCRAM pending",
                            user.username, pool_name
                        );
                        Some(Arc::new(RwLock::new(BackendAuthMethod::ScramPending)))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let address = Address {
                    database: pool_name.clone(),
                    host: pool_config.server_host.clone(),
                    port: pool_config.server_port,
                    username: user.username.clone(),
                    password: user.password.clone(),
                    pool_name: pool_name.clone(),
                    stats: Arc::new(AddressStats::default()),
                    backend_auth,
                    server_tls: server_tls_config.clone(),
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

                let fallback_state = build_fallback_state(pool_name, pool_config, &config.general);

                // Merge general+pool startup_parameters from the same
                // `config` snapshot we hashed above. ServerPool keeps this
                // as Arc<BTreeMap> for the rest of its life — the reload
                // path rebuilds the pool whenever either layer's hash
                // changes, so the snapshot stays valid until then. Passing
                // it in explicitly (rather than letting ServerPool::new
                // call config_arc() again) closes a narrow race where a
                // second reload between this iteration and constructor
                // execution would write a different baseline to the pool
                // than the one the reuse hash captured.
                let base_startup_parameters = {
                    let mut merged: std::collections::BTreeMap<String, String> =
                        config.general.startup_parameters.clone();
                    for (k, v) in &pool_config.startup_parameters {
                        merged.insert(k.clone(), v.clone());
                    }
                    Arc::new(merged)
                };

                let manager = ServerPool::new(
                    address.clone(),
                    user.clone(),
                    server_database.as_str(),
                    client_server_map.clone(),
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
                    // Static pools carry no per-user auth_query overlay.
                    Arc::new(std::collections::BTreeMap::new()),
                );

                let queue_strategy = match config.general.server_round_robin {
                    true => QueueMode::Fifo,
                    false => QueueMode::Lifo,
                };

                let mut builder_config = Pool::builder(manager)
                    .coordinator(coordinators.get(pool_name).cloned())
                    .pool_name(pool_name.clone())
                    .username(user.username.clone());
                builder_config = builder_config.config(PoolConfig {
                    max_size: user.pool_size as usize,
                    timeouts: Timeouts {
                        wait: Some(config.general.query_wait_timeout.as_std()),
                        create: Some(config.general.connect_timeout.as_std()),
                        recycle: None,
                    },
                    queue_mode: queue_strategy,
                    scaling: pool_config.resolve_scaling_config(&config.general),
                });

                let pool = builder_config.build();

                let pool = ConnectionPool {
                    database: pool,
                    address,
                    config_hash: new_pool_hash_value,
                    // Static and dedicated-mode shared pools carry no
                    // per-user overlay, so they pin to the empty-map
                    // hash. Dynamic passthrough pools set this from the
                    // captured overlay in dynamic.rs.
                    per_user_startup_overlay_hash: empty_overlay_hash(),
                    original_server_parameters: Arc::new(tokio::sync::Mutex::new(
                        ServerParameters::new(),
                    )),
                    settings: PoolSettings {
                        pool_mode,
                        user: user.clone(),
                        db: pool_name.clone(),
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
                    coordinator: coordinators.get(pool_name).cloned(),
                    replenish_failures: Arc::new(AtomicU32::new(0)),
                    created_at: std::time::Instant::now(),
                };

                // There is one pool per database/user pair.
                new_pools.insert(PoolIdentifier::new(pool_name, &user.username), pool);
            }
        }

        // -----------------------------------------------------------------
        // Auth query: create AuthQueryState per pool (lazy executor init)
        // and shared connection pool for server_user mode.
        // -----------------------------------------------------------------
        let mut auth_query_states: HashMap<String, Arc<AuthQueryState>> = HashMap::new();

        let old_aq_states_for_reuse = AUTH_QUERY_STATE.load();

        for (pool_name, pool_config) in &config.pools {
            if let Some(ref aq_config) = pool_config.auth_query {
                let pool_startup_hash = {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    pool_config.startup_parameters.hash(&mut hasher);
                    hasher.finish()
                };
                // RELOAD: reuse state when both the auth_query config AND
                // the pool-level startup_parameters are unchanged. A
                // pool.startup_parameters edit must drop the cache and
                // recycle the shared/dynamic pools: their backends were
                // started with the old baseline as `reset_val`, and that
                // value survives client-side `RESET ALL` / `DISCARD ALL`
                // unless the backend is recreated.
                if let Some(old_state) = old_aq_states_for_reuse.get(pool_name) {
                    if old_state.config == *aq_config
                        && old_state.pool_startup_hash == pool_startup_hash
                    {
                        info!("[pool: {pool_name}] auth_query config unchanged — reusing state");
                        auth_query_states.insert(pool_name.clone(), old_state.clone());
                        // Still need to ensure shared pool exists in new_pools
                        if let Some(ref spid) = old_state.shared_pool_id {
                            if !new_pools.contains_key(spid) {
                                if let Some(pool) = POOLS.load().get(spid) {
                                    new_pools.insert(spid.clone(), pool.clone());
                                }
                            }
                        }
                        continue;
                    }
                }

                let shared_pool_id = if aq_config.is_dedicated_mode() {
                    let su = aq_config.server_user.as_ref().unwrap();
                    let identifier = PoolIdentifier::new(pool_name, su);

                    // Create the shared data pool if it doesn't already exist
                    // (a static user with the same name takes priority).
                    if !new_pools.contains_key(&identifier) {
                        let sp = aq_config
                            .server_password
                            .as_ref()
                            .cloned()
                            .unwrap_or_default();

                        let shared_user = User {
                            username: su.clone(),
                            password: String::new(),
                            pool_size: aq_config.pool_size,
                            server_username: Some(su.clone()),
                            server_password: Some(sp),
                            ..Default::default()
                        };

                        let server_database = pool_config
                            .server_database
                            .clone()
                            .unwrap_or_else(|| pool_name.to_string());
                        let server_tls_config =
                            build_server_tls_for_pool(pool_config, &config.general)?;

                        let address = Address {
                            database: pool_name.clone(),
                            host: pool_config.server_host.clone(),
                            port: pool_config.server_port,
                            username: shared_user.username.clone(),
                            password: shared_user.password.clone(),
                            pool_name: pool_name.clone(),
                            stats: Arc::new(AddressStats::default()),
                            backend_auth: None,
                            server_tls: server_tls_config,
                        };

                        let prepared_statements_cache_size =
                            match config.general.prepared_statements {
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

                        let pool_mode = shared_user.pool_mode.unwrap_or(pool_config.pool_mode);

                        let fallback_state =
                            build_fallback_state(pool_name, pool_config, &config.general);

                        let base_startup_parameters = {
                            let mut merged: std::collections::BTreeMap<String, String> =
                                config.general.startup_parameters.clone();
                            for (k, v) in &pool_config.startup_parameters {
                                merged.insert(k.clone(), v.clone());
                            }
                            Arc::new(merged)
                        };

                        let manager = ServerPool::new(
                            address.clone(),
                            shared_user.clone(),
                            server_database.as_str(),
                            client_server_map.clone(),
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
                            // Dedicated-mode shared pool serves multiple
                            // dynamic users — no single per-user override.
                            Arc::new(std::collections::BTreeMap::new()),
                        );

                        let queue_strategy = match config.general.server_round_robin {
                            true => QueueMode::Fifo,
                            false => QueueMode::Lifo,
                        };

                        info!(
                            "[{}@{}] creating auth_query shared pool",
                            shared_user.username, pool_name
                        );

                        let pool = Pool::builder(manager)
                            .coordinator(coordinators.get(pool_name).cloned())
                            .pool_name(pool_name.clone())
                            .username(shared_user.username.clone())
                            .config(PoolConfig {
                                max_size: shared_user.pool_size as usize,
                                timeouts: Timeouts {
                                    wait: Some(config.general.query_wait_timeout.as_std()),
                                    create: Some(config.general.connect_timeout.as_std()),
                                    recycle: None,
                                },
                                queue_mode: queue_strategy,
                                scaling: pool_config.resolve_scaling_config(&config.general),
                            })
                            .build();

                        let new_pool_hash_value = pool_config.hash_value();
                        let conn_pool = ConnectionPool {
                            database: pool,
                            address,
                            config_hash: new_pool_hash_value,
                            // Static and dedicated-mode shared pools carry no
                            // per-user overlay, so they pin to the empty-map
                            // hash. Dynamic passthrough pools set this from the
                            // captured overlay in dynamic.rs.
                            per_user_startup_overlay_hash: empty_overlay_hash(),
                            original_server_parameters: Arc::new(tokio::sync::Mutex::new(
                                ServerParameters::new(),
                            )),
                            settings: PoolSettings {
                                pool_mode,
                                user: shared_user,
                                db: pool_name.clone(),
                                idle_timeout_ms: pool_config
                                    .idle_timeout
                                    .unwrap_or(config.general.idle_timeout.as_millis()),
                                life_time_ms: pool_config
                                    .server_lifetime
                                    .unwrap_or(config.general.server_lifetime.as_millis()),
                                sync_server_parameters: config.general.sync_server_parameters,
                                min_guaranteed_pool_size: pool_config
                                    .min_guaranteed_pool_size
                                    .unwrap_or(0),
                            },
                            prepared_statement_cache: match config.general.prepared_statements {
                                false => None,
                                true => Some(Arc::new(PreparedStatementCache::new(
                                    prepared_statements_cache_size,
                                    config.general.worker_threads,
                                ))),
                            },
                            coordinator: coordinators.get(pool_name).cloned(),
                            replenish_failures: Arc::new(AtomicU32::new(0)),
                            created_at: std::time::Instant::now(),
                        };

                        new_pools.insert(identifier.clone(), conn_pool);
                    }

                    Some(identifier)
                } else {
                    None // passthrough mode — dynamic pool created on first client connection
                };

                auth_query_states.insert(
                    pool_name.clone(),
                    Arc::new(AuthQueryState::new(
                        aq_config.clone(),
                        pool_startup_hash,
                        pool_name.clone(),
                        pool_config.server_host.clone(),
                        pool_config.server_port,
                        shared_pool_id,
                        Arc::new(AuthQueryStats::default()),
                    )),
                );
            }
        }

        // --- RELOAD: detect auth_query config changes, manage dynamic pools ---
        let old_aq_states = old_aq_states_for_reuse;
        let mut pools_to_remove: Vec<PoolIdentifier> = Vec::new();

        // 1. Compare old vs new auth_query configs, plus pool-level
        //    startup_parameters: either change must drain dynamic pools
        //    for this pool_name so the next auth_query lookup builds
        //    fresh backends with the new baseline reset_val.
        for (pool_name, old_state) in old_aq_states.iter() {
            let new_pool_config = config.pools.get(pool_name);
            let new_aq = new_pool_config.and_then(|p| p.auth_query.as_ref());
            let aq_changed = match new_aq {
                None => true,                          // auth_query removed
                Some(new) => *new != old_state.config, // config changed
            };
            let new_pool_startup_hash = new_pool_config.map(|p| {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                p.startup_parameters.hash(&mut hasher);
                hasher.finish()
            });
            let pool_startup_changed = new_pool_startup_hash
                .map(|h| h != old_state.pool_startup_hash)
                .unwrap_or(false);
            if aq_changed || pool_startup_changed {
                if aq_changed {
                    info!("[pool: {pool_name}] auth_query config changed — collecting dynamic pools for removal");
                } else {
                    info!("[pool: {pool_name}] pool.startup_parameters changed — collecting dynamic pools for removal");
                }
                for id in DYNAMIC_POOLS.load().iter() {
                    if id.db == *pool_name {
                        pools_to_remove.push(id.clone());
                    }
                }
                old_state.try_clear_cache();
            }
        }

        // 2. Static user overrides dynamic pool
        for (pool_name, pool_config) in &config.pools {
            for user in &pool_config.users {
                let id = PoolIdentifier::new(pool_name, &user.username);
                if is_dynamic_pool(&id) && !pools_to_remove.contains(&id) {
                    info!(
                        "[pool: {pool_name}] static user '{}' overrides dynamic pool",
                        user.username
                    );
                    pools_to_remove.push(id);
                }
            }
        }

        // 2b. general.startup_parameters changed: drain every dynamic pool
        //     so the next auth_query lookup builds fresh backends with the
        //     new baseline. Static pools are already handled by the pool
        //     reuse hash above, which folds in `general_startup_hash`.
        if general_startup_parameters_changed {
            info!(
                "general.startup_parameters changed on reload — collecting all dynamic pools for recycle"
            );
            for id in DYNAMIC_POOLS.load().iter() {
                if !pools_to_remove.contains(id) {
                    pools_to_remove.push(id.clone());
                }
            }
        }

        // 3. Carry over surviving dynamic pools
        let old_pools = POOLS.load();
        for id in DYNAMIC_POOLS.load().iter() {
            if pools_to_remove.contains(id) {
                continue;
            }
            if new_pools.contains_key(id) {
                continue;
            }
            if let Some(pool) = old_pools.get(id) {
                new_pools.insert(id.clone(), pool.clone());
            }
        }

        // 4. Remove destroyed pools, update tracking + stats
        for id in &pools_to_remove {
            new_pools.remove(id);
            // Increment dynamic_pools_destroyed on the OLD state (before we replace it)
            if let Some(old_state) = old_aq_states.get(&id.db) {
                old_state
                    .stats
                    .dynamic_pools_destroyed
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        if !pools_to_remove.is_empty() {
            let mut new_dynamic = (**DYNAMIC_POOLS.load()).clone();
            for id in &pools_to_remove {
                new_dynamic.remove(id);
            }
            DYNAMIC_POOLS.store(Arc::new(new_dynamic));
            info!("RELOAD: removed {} dynamic pool(s)", pools_to_remove.len());
        }

        COORDINATORS.store(Arc::new(coordinators));
        AUTH_QUERY_STATE.store(Arc::new(auth_query_states));
        POOLS.store(Arc::new(new_pools.clone()));
        // Advance the recycle-watcher hash only after the new state is
        // published; a failure path above (Err returned via `?`) leaves
        // PREVIOUS_GENERAL_STARTUP_HASH alone so the next reload still
        // sees the old value and re-evaluates the change correctly.
        PREVIOUS_GENERAL_STARTUP_HASH.store(general_startup_hash, Ordering::Relaxed);
        Ok(())
    }

    /// Get pool state for a particular shard server as reported by pooler.
    #[inline(always)]
    pub fn pool_state(&self) -> Status {
        self.database.status()
    }

    /// Get the address information for a server.
    #[inline(always)]
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Register a parse statement to the pool's cache and return the rewritten parse.
    ///
    /// `client_given_name` is the original Parse name from the client. `None`
    /// indicates an anonymous prepared statement (PostgreSQL's empty Parse
    /// name); `Some(name)` carries the client-supplied identifier. It is
    /// forwarded to the pool cache so each entry tracks whether it was ever
    /// Parse'd as a named statement, an anonymous one, or both — surfaced via
    /// `CacheEntryKind`.
    #[inline(always)]
    pub fn register_parse_to_cache(
        &self,
        hash: u64,
        parse: &Parse,
        client_given_name: Option<&str>,
    ) -> Option<Arc<Parse>> {
        // We should only be calling this function if the cache is enabled
        self.prepared_statement_cache
            .as_ref()
            .map(|cache| cache.get_or_insert(parse, hash, client_given_name))
    }

    /// Promote a prepared statement hash in the LRU
    #[inline(always)]
    pub fn promote_prepared_statement_hash(&self, hash: &u64) {
        // We should only be calling this function if the cache is enabled
        if let Some(ref prepared_statement_cache) = self.prepared_statement_cache {
            prepared_statement_cache.promote(hash);
        }
    }

    pub async fn get_server_parameters(&mut self) -> Result<ServerParameters, Error> {
        let mut guard = self.original_server_parameters.lock().await;
        if !guard.is_empty() {
            return Ok(guard.clone());
        }
        info!(
            "[{}@{}] fetching server parameters",
            self.address.username, self.address.pool_name
        );
        {
            let conn = match self.database.get().await {
                Ok(conn) => conn,
                // PG-side rejection of an operator-supplied startup
                // parameter must keep its typed shape so the cold auth
                // path returns the same `ErrorResponse` (real SQLSTATE
                // and PG message) to the client that the transaction
                // checkout path already returns through
                // src/client/transaction.rs. Stringifying the
                // PoolError here collapses the carried sqlstate/message
                // into a generic 58000/3D000 — which contradicts the
                // "rejection forwarded verbatim" contract.
                Err(PoolError::Backend(err @ Error::ServerStartupParameterRejection { .. })) => {
                    return Err(err);
                }
                Err(err) => return Err(Error::ServerStartupReadParameters(err.to_string())),
            };
            guard.set_from_hashmap(&conn.server_parameters_as_hashmap(), true);
        }
        Ok(guard.clone())
    }

    /// Connections above the user's guaranteed minimum — these are eligible
    /// for eviction by the coordinator when another user needs a connection.
    /// Effective minimum = max(user.min_pool_size, pool.min_guaranteed_pool_size).
    pub fn spare_above_min(&self) -> usize {
        let current = self.pool_state().size;
        compute_spare(
            current,
            self.settings.user.min_pool_size,
            self.settings.min_guaranteed_pool_size,
        )
    }
}

/// Compute how many connections are above the effective guaranteed minimum.
/// Pure function extracted from `ConnectionPool::spare_above_min()` for testability.
fn compute_spare(
    current_pool_size: usize,
    user_min_pool_size: Option<u32>,
    pool_min_guaranteed: u32,
) -> usize {
    let user_min = user_min_pool_size.unwrap_or(0) as usize;
    let pool_min = pool_min_guaranteed as usize;
    let effective_min = user_min.max(pool_min);
    current_pool_size.saturating_sub(effective_min)
}

/// Build Patroni-assisted fallback state. Returns None when no `patroni_api_urls`
/// are configured at either pool or general level.
fn build_fallback_state(
    pool_name: &str,
    pool_config: &ConfigPool,
    general: &crate::config::General,
) -> Option<Arc<fallback::FallbackState>> {
    let urls = pool_config
        .patroni_api_urls
        .as_ref()
        .or(general.patroni_api_urls.as_ref())?;

    let cooldown = pool_config
        .fallback_cooldown
        .or(general.fallback_cooldown)
        .map(|d| d.as_std())
        .unwrap_or(std::time::Duration::from_secs(30));
    let api_timeout = pool_config
        .patroni_api_timeout
        .or(general.patroni_api_timeout)
        .map(|d| d.as_std())
        .unwrap_or(std::time::Duration::from_secs(5));
    let connect_timeout = pool_config
        .fallback_connect_timeout
        .or(general.fallback_connect_timeout)
        .map(|d| d.as_std())
        .unwrap_or(std::time::Duration::from_secs(5));
    let lifetime = pool_config
        .fallback_lifetime
        .or(general.fallback_lifetime)
        .map(|d| d.as_millis())
        .unwrap_or(cooldown.as_millis() as u64);

    match fallback::FallbackState::new(
        pool_name.to_string(),
        urls.clone(),
        cooldown,
        connect_timeout,
        api_timeout,
        lifetime,
    ) {
        Ok(state) => Some(Arc::new(state)),
        Err(e) => {
            log::error!("pool {pool_name}: Patroni-assisted fallback disabled: {e}");
            None
        }
    }
}

/// Resolve the per-backend prepared-statement LRU size for a pool.
///
/// Resolution order (most specific wins):
/// 1. `pool_override` (per-pool `server_prepared_statements_cache_size`)
/// 2. `general_override` (general-level `server_prepared_statements_cache_size`)
/// 3. fallback to `pool_cache_size` — the resolved
///    `prepared_statements_cache_size` for that pool, preserving the
///    behaviour from before this knob existed.
///
/// Returns 0 when `pool_cache_size` is 0: the pool-level cache is
/// disabled, so a per-backend LRU adds no value.
pub(crate) fn resolve_server_cache_size(
    pool_cache_size: usize,
    pool_override: Option<usize>,
    general_override: Option<usize>,
) -> usize {
    if pool_cache_size == 0 {
        return 0;
    }
    pool_override
        .or(general_override)
        .unwrap_or(pool_cache_size)
}

/// Pure helper that decides the per-client Anonymous LRU size from a
/// general config and an already-extracted per-pool override. Pulled
/// out so the unit tests can exercise the resolution table without
/// touching global pool state.
pub(crate) fn resolve_client_anon_cache_size_inner(
    general: &General,
    pool_override: Option<usize>,
) -> usize {
    if let Some(explicit) = general.client_anonymous_prepared_cache_size {
        return explicit;
    }
    pool_override.unwrap_or(general.prepared_statements_cache_size)
}

/// Resolve the per-client Anonymous LRU size for a connection coming
/// in on `pool_name`. Looks up the pool's
/// `prepared_statements_cache_size` override and feeds it into the
/// pure helper above. Falls back to general defaults when the pool
/// is not in static config — admin connections, dynamic auth_query
/// pools that have not been registered yet — matching pre-3.7
/// behaviour for those paths.
pub fn resolve_client_anon_cache_size(pool_name: &str, general: &General) -> usize {
    let pool_override = get_pool_config(pool_name).and_then(|p| p.prepared_statements_cache_size);
    resolve_client_anon_cache_size_inner(general, pool_override)
}

/// Get the connection pool
pub fn get_pool(db: &str, user: &str) -> Option<ConnectionPool> {
    (*(*POOLS.load()))
        .get(&PoolIdentifier::new(db, user))
        .cloned()
}

/// Get pool-level configuration by database name.
/// Returns the Pool config if the database exists in configuration.
/// Used by auth_query to find auth_query config when user is not in static config.
pub fn get_pool_config(db: &str) -> Option<crate::config::Pool> {
    crate::config::config_arc().pools.get(db).cloned()
}

/// Get a pointer to all configured pools.
/// Returns an Arc to avoid cloning the entire HashMap on each call.
pub fn get_all_pools() -> Arc<PoolMap> {
    POOLS.load_full()
}

/// Get pool coordinator for a database pool (if `max_db_connections > 0`).
/// Returns `None` when coordination is disabled for this pool.
pub fn get_coordinator(db: &str) -> Option<Arc<pool_coordinator::PoolCoordinator>> {
    COORDINATORS.load().get(db).cloned()
}

// create_dynamic_pool is in dynamic.rs, re-exported above.

#[cfg(test)]
mod tests {
    use super::*;

    // --- per_user_overlay_hash tests ---

    #[test]
    fn per_user_overlay_hash_empty_matches_empty_overlay_hash() {
        let empty_map = std::collections::HashMap::<String, String>::new();
        assert_eq!(
            per_user_overlay_hash(empty_map.iter()),
            empty_overlay_hash()
        );
    }

    #[test]
    fn per_user_overlay_hash_ignores_input_order() {
        // HashMap with the same key/value pairs but inserted in different
        // orders must hash identically. Without the internal sort the
        // hash would depend on HashMap iteration order, which is
        // randomized per process and would falsely flag overlay drift on
        // every refetch.
        let mut a = std::collections::HashMap::new();
        a.insert("work_mem".to_string(), "64MB".to_string());
        a.insert("statement_timeout".to_string(), "30s".to_string());
        let mut b = std::collections::HashMap::new();
        b.insert("statement_timeout".to_string(), "30s".to_string());
        b.insert("work_mem".to_string(), "64MB".to_string());
        assert_eq!(
            per_user_overlay_hash(a.iter()),
            per_user_overlay_hash(b.iter())
        );
    }

    #[test]
    fn per_user_overlay_hash_changes_when_value_changes() {
        let mut a = std::collections::HashMap::new();
        a.insert("work_mem".to_string(), "64MB".to_string());
        let mut b = std::collections::HashMap::new();
        b.insert("work_mem".to_string(), "128MB".to_string());
        assert_ne!(
            per_user_overlay_hash(a.iter()),
            per_user_overlay_hash(b.iter())
        );
    }

    #[test]
    fn per_user_overlay_hash_changes_when_key_added() {
        let mut a = std::collections::HashMap::new();
        a.insert("work_mem".to_string(), "64MB".to_string());
        let mut b = a.clone();
        b.insert("statement_timeout".to_string(), "30s".to_string());
        assert_ne!(
            per_user_overlay_hash(a.iter()),
            per_user_overlay_hash(b.iter())
        );
    }

    #[test]
    fn per_user_overlay_hash_matches_across_hashmap_and_btreemap() {
        // The auth_query cache stores HashMap; the pool freezes a
        // BTreeMap snapshot. Drift detection compares the two — they
        // must hash to the same value for identical content.
        let mut h = std::collections::HashMap::new();
        h.insert("work_mem".to_string(), "64MB".to_string());
        let mut b = std::collections::BTreeMap::new();
        b.insert("work_mem".to_string(), "64MB".to_string());
        assert_eq!(
            per_user_overlay_hash(h.iter()),
            per_user_overlay_hash(b.iter())
        );
    }

    // --- compute_spare tests ---

    #[test]
    fn spare_no_minimums_set() {
        // No min_pool_size, no min_guaranteed → all connections are spare
        assert_eq!(compute_spare(5, None, 0), 5);
    }

    #[test]
    fn spare_with_user_min_pool_size_only() {
        // user.min_pool_size=3, pool.min_guaranteed=0 → effective_min=3
        assert_eq!(compute_spare(5, Some(3), 0), 2);
    }

    #[test]
    fn spare_with_pool_guaranteed_only() {
        // user.min_pool_size=None, pool.min_guaranteed=4 → effective_min=4
        assert_eq!(compute_spare(5, None, 4), 1);
    }

    #[test]
    fn spare_pool_guaranteed_wins_over_user_min() {
        // user.min_pool_size=2, pool.min_guaranteed=4 → effective_min=max(2,4)=4
        assert_eq!(compute_spare(5, Some(2), 4), 1);
    }

    #[test]
    fn spare_user_min_wins_over_pool_guaranteed() {
        // user.min_pool_size=5, pool.min_guaranteed=2 → effective_min=max(5,2)=5
        assert_eq!(compute_spare(5, Some(5), 2), 0);
    }

    #[test]
    fn spare_at_exact_minimum() {
        // current == effective_min → 0 spare
        assert_eq!(compute_spare(3, Some(3), 0), 0);
        assert_eq!(compute_spare(4, None, 4), 0);
    }

    #[test]
    fn spare_below_minimum_saturates_to_zero() {
        // current < effective_min → saturating_sub returns 0
        assert_eq!(compute_spare(2, Some(5), 0), 0);
        assert_eq!(compute_spare(1, None, 3), 0);
        assert_eq!(compute_spare(0, Some(1), 2), 0);
    }

    #[test]
    fn spare_zero_current_connections() {
        assert_eq!(compute_spare(0, None, 0), 0);
        assert_eq!(compute_spare(0, Some(3), 5), 0);
    }

    #[test]
    fn spare_both_minimums_equal() {
        // user.min_pool_size=3, pool.min_guaranteed=3 → effective_min=3
        assert_eq!(compute_spare(5, Some(3), 3), 2);
    }

    #[test]
    fn spare_large_values() {
        assert_eq!(compute_spare(1000, Some(100), 200), 800);
        assert_eq!(compute_spare(1000, Some(999), 1), 1);
    }

    // --- resolve_server_cache_size tests ---

    #[test]
    fn server_cache_size_defaults_to_pool_size() {
        // Neither override is set → inherit pool_cache_size.
        assert_eq!(resolve_server_cache_size(8192, None, None), 8192);
    }

    #[test]
    fn server_cache_size_general_override_takes_effect() {
        // General override applied when pool override absent.
        assert_eq!(resolve_server_cache_size(8192, None, Some(1024)), 1024);
    }

    #[test]
    fn server_cache_size_pool_override_wins_over_general() {
        // Per-pool override is the most specific level.
        assert_eq!(
            resolve_server_cache_size(8192, Some(2048), Some(1024)),
            2048
        );
    }

    #[test]
    fn server_cache_size_pool_override_wins_over_inheritance() {
        assert_eq!(resolve_server_cache_size(8192, Some(2048), None), 2048);
    }

    #[test]
    fn server_cache_size_zero_pool_disables_server_lru() {
        // pool_cache_size=0 means caches are off; server LRU is forced to 0
        // regardless of overrides.
        assert_eq!(resolve_server_cache_size(0, None, None), 0);
        assert_eq!(resolve_server_cache_size(0, Some(1024), None), 0);
        assert_eq!(resolve_server_cache_size(0, None, Some(1024)), 0);
        assert_eq!(resolve_server_cache_size(0, Some(2048), Some(1024)), 0);
    }

    #[test]
    fn server_cache_size_explicit_zero_per_pool_allowed() {
        // Operators may explicitly disable the per-backend LRU even with a
        // positive pool cache; resolve must return 0 in that case.
        assert_eq!(resolve_server_cache_size(8192, Some(0), None), 0);
        assert_eq!(resolve_server_cache_size(8192, Some(0), Some(1024)), 0);
    }

    // --- resolve_client_anon_cache_size_inner tests ---

    #[test]
    fn anon_cache_inherits_general_when_no_overrides() {
        let g = General::test_with_cache_sizes(8192, None);
        assert_eq!(resolve_client_anon_cache_size_inner(&g, None), 8192);
    }

    #[test]
    fn anon_cache_uses_pool_override_when_no_explicit() {
        let g = General::test_with_cache_sizes(8192, None);
        assert_eq!(resolve_client_anon_cache_size_inner(&g, Some(1024)), 1024);
    }

    #[test]
    fn anon_cache_explicit_wins_over_pool_override() {
        let g = General::test_with_cache_sizes(8192, Some(256));
        assert_eq!(resolve_client_anon_cache_size_inner(&g, Some(1024)), 256);
        assert_eq!(resolve_client_anon_cache_size_inner(&g, None), 256);
    }

    #[test]
    fn anon_cache_explicit_zero_disables_lru_regardless_of_pool() {
        let g = General::test_with_cache_sizes(8192, Some(0));
        assert_eq!(resolve_client_anon_cache_size_inner(&g, Some(1024)), 0);
    }
}
