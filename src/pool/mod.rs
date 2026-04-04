use arc_swap::ArcSwap;
use dashmap::DashMap;
use log::{debug, info};
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::config::{get_config, Address, BackendAuthMethod, General, PoolMode, User};
use crate::errors::Error;
use crate::messages::Parse;

use crate::server::ServerParameters;
use crate::stats::auth_query::AuthQueryStats;
use crate::stats::AddressStats;

mod errors;
mod inner;
mod types;

pub use errors::{PoolError, RecycleError, RecycleResult};
pub use inner::{Object, Pool, PoolBuilder};
pub use types::{Metrics, PoolConfig, QueueMode, ScalingConfig, Status, Timeouts};

pub use crate::server::PreparedStatementCache;

mod auth_query_state;
mod dynamic;
mod eviction;
pub mod gc;
pub mod pool_coordinator;
pub mod retain;
mod server_pool;

pub use auth_query_state::AuthQueryState;
pub use dynamic::create_dynamic_pool;
pub use eviction::PoolEvictionSource;
pub use server_pool::ServerPool;

pub type ProcessId = i32;
pub type SecretKey = i32;
pub type ServerHost = String;
pub type ServerPort = u16;

pub type ClientServerMap =
    Arc<DashMap<(ProcessId, SecretKey), (ProcessId, SecretKey, ServerHost, ServerPort)>>;
pub type PoolMap = HashMap<PoolIdentifier, ConnectionPool>;

/// The connection pool, globally available.
/// This is atomic and safe and read-optimized.
/// The pool is recreated dynamically when the config is reloaded.
pub static POOLS: Lazy<ArcSwap<PoolMap>> = Lazy::new(|| ArcSwap::from_pointee(HashMap::default()));
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

fn get_client_server_map() -> Option<ClientServerMap> {
    CLIENT_SERVER_MAP.get().cloned()
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

    /// Cache
    pub prepared_statement_cache: Option<PreparedStatementCacheType>,

    /// Database-level connection coordinator. `Some` when `max_db_connections > 0`
    /// in the pool config, `None` otherwise (disabled, zero overhead).
    /// Shared across all user pools for the same database.
    pub(crate) coordinator: Option<Arc<pool_coordinator::PoolCoordinator>>,

    /// Consecutive replenish failure counter for log noise suppression.
    /// Reset to 0 on successful replenish.
    pub(crate) replenish_failures: Arc<AtomicU32>,
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
                min_connection_lifetime_ms: pool_config.min_connection_lifetime.unwrap_or(5000),
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

        for (pool_name, pool_config) in &config.pools {
            let new_pool_hash_value = pool_config.hash_value();

            // There is one pool per database/user pair.
            for user in &pool_config.users {
                let old_pool_ref = get_pool(pool_name, &user.username);
                let identifier = PoolIdentifier::new(pool_name, &user.username);

                if let Some(pool) = old_pool_ref {
                    // If the pool hasn't changed, get existing reference and insert it into the new_pools.
                    // We replace all pools at the end, but if the reference is kept, the pool won't get re-created (bb8).
                    if pool.config_hash == new_pool_hash_value {
                        info!("[{}@{}] config unchanged", user.username, pool_name);
                        new_pools.insert(identifier.clone(), pool.clone());
                        continue;
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
                };

                let prepared_statements_cache_size = match config.general.prepared_statements {
                    true => pool_config
                        .prepared_statements_cache_size
                        .unwrap_or(config.general.prepared_statements_cache_size),
                    false => 0,
                };

                let application_name = pool_config
                    .application_name
                    .clone()
                    .unwrap_or_else(|| "pg_doorman".to_string());

                let pool_mode = user.pool_mode.unwrap_or(pool_config.pool_mode);

                let manager = ServerPool::new(
                    address.clone(),
                    user.clone(),
                    server_database.as_str(),
                    client_server_map.clone(),
                    pool_config.cleanup_server_connections,
                    pool_config.log_client_parameter_status_changes,
                    prepared_statements_cache_size,
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
                    pool_mode == PoolMode::Session,
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
                // RELOAD: reuse state when config unchanged (preserves cache, executor, stats)
                if let Some(old_state) = old_aq_states_for_reuse.get(pool_name) {
                    if old_state.config == *aq_config {
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

                        let address = Address {
                            database: pool_name.clone(),
                            host: pool_config.server_host.clone(),
                            port: pool_config.server_port,
                            username: shared_user.username.clone(),
                            password: shared_user.password.clone(),
                            pool_name: pool_name.clone(),
                            stats: Arc::new(AddressStats::default()),
                            backend_auth: None,
                        };

                        let prepared_statements_cache_size =
                            match config.general.prepared_statements {
                                true => pool_config
                                    .prepared_statements_cache_size
                                    .unwrap_or(config.general.prepared_statements_cache_size),
                                false => 0,
                            };

                        let application_name = pool_config
                            .application_name
                            .clone()
                            .unwrap_or_else(|| "pg_doorman".to_string());

                        let pool_mode = shared_user.pool_mode.unwrap_or(pool_config.pool_mode);

                        let manager = ServerPool::new(
                            address.clone(),
                            shared_user.clone(),
                            server_database.as_str(),
                            client_server_map.clone(),
                            pool_config.cleanup_server_connections,
                            pool_config.log_client_parameter_status_changes,
                            prepared_statements_cache_size,
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
                            pool_mode == PoolMode::Session,
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

        // 1. Compare old vs new auth_query configs
        for (pool_name, old_state) in old_aq_states.iter() {
            let new_aq = config
                .pools
                .get(pool_name)
                .and_then(|p| p.auth_query.as_ref());
            let changed = match new_aq {
                None => true,                          // auth_query removed
                Some(new) => *new != old_state.config, // config changed
            };
            if changed {
                info!("[pool: {pool_name}] auth_query config changed — collecting dynamic pools for removal");
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

    /// Register a parse statement to the pool's cache and return the rewritten parse
    ///
    /// Do not pass an anonymous parse statement to this function
    #[inline(always)]
    pub fn register_parse_to_cache(&self, hash: u64, parse: &Parse) -> Option<Arc<Parse>> {
        // We should only be calling this function if the cache is enabled
        self.prepared_statement_cache
            .as_ref()
            .map(|cache| cache.get_or_insert(parse, hash))
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
    let config = get_config();
    config.pools.get(db).cloned()
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
}
