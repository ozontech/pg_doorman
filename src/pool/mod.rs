use arc_swap::ArcSwap;
use dashmap::DashMap;
use log::{debug, info, warn};
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

use crate::auth::auth_query::{AuthQueryCache, AuthQueryExecutor};
use crate::config::{
    get_config, Address, AuthQueryConfig, BackendAuthMethod, General, PoolMode, User,
};
use crate::errors::Error;
use crate::messages::Parse;

use crate::server::{Server, ServerParameters};
use crate::stats::auth_query::AuthQueryStats;
use crate::stats::{AddressStats, ServerStats};

mod errors;
mod inner;
mod types;

pub use errors::{PoolError, RecycleError, RecycleResult};
pub use inner::{Object, Pool, PoolBuilder};
pub use types::{Metrics, PoolConfig, QueueMode, ScalingConfig, Status, Timeouts};

pub use crate::server::PreparedStatementCache;

pub mod gc;
pub mod retain;

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

// ---------------------------------------------------------------------------
// Auth query state (per-pool, lazily initialized)
// ---------------------------------------------------------------------------

/// Per-pool auth_query state: cache + config + shared pool identifier.
///
/// The executor and cache are lazily initialized via `OnceCell` on the first
/// auth_query authentication attempt. This ensures `from_config()` never fails
/// due to an unreachable auth_query PostgreSQL server — static users continue
/// working. If the first init fails, `OnceCell` retries on the next call.
pub struct AuthQueryState {
    cache_cell: tokio::sync::OnceCell<AuthQueryCache>,
    config: AuthQueryConfig,
    pool_name: String,
    server_host: String,
    server_port: u16,
    /// Pool identifier for the shared server_user pool (None = passthrough mode).
    pub shared_pool_id: Option<PoolIdentifier>,
    /// Per-pool auth_query metrics (shared with cache and admin/prometheus).
    pub stats: Arc<AuthQueryStats>,
}

impl AuthQueryState {
    /// Get the auth_query config for this pool.
    pub fn config(&self) -> &AuthQueryConfig {
        &self.config
    }

    /// Get the cache, lazily initializing the executor + cache on first access.
    ///
    /// If PG is unreachable, returns `Err`; the `OnceCell` does NOT store the
    /// error, so the next call will retry the connection.
    /// Clear the auth_query cache if it was already initialized.
    /// Called on RELOAD when auth_query config changes. Does NOT trigger
    /// lazy initialization (safe to call even if executor was never created).
    pub fn try_clear_cache(&self) {
        if let Some(cache) = self.cache_cell.get() {
            cache.clear();
            info!(
                "[pool: {}] auth_query cache cleared on RELOAD",
                self.pool_name
            );
        }
    }

    pub async fn cache(&self) -> Result<&AuthQueryCache, Error> {
        self.cache_cell
            .get_or_try_init(|| async {
                info!(
                    "[pool: {}] auth_query: initializing executor (lazy, first request)",
                    self.pool_name
                );
                let executor = AuthQueryExecutor::new(
                    &self.config,
                    &self.pool_name,
                    &self.server_host,
                    self.server_port,
                )
                .await?;
                Ok(AuthQueryCache::new(
                    Arc::new(executor),
                    &self.config,
                    Some(self.stats.clone()),
                ))
            })
            .await
    }

    /// Number of cached entries (0 if cache not yet initialized).
    pub fn cache_len(&self) -> usize {
        self.cache_cell.get().map_or(0, |c| c.len())
    }
}

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
}

impl ConnectionPool {
    /// Construct the connection pool from the configuration.
    pub async fn from_config(client_server_map: ClientServerMap) -> Result<(), Error> {
        set_client_server_map(client_server_map.clone());
        let config = get_config();

        let mut new_pools = HashMap::new();

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
                        info!(
                            "[pool: {}][user: {}] has not changed",
                            pool_name, user.username
                        );
                        new_pools.insert(identifier.clone(), pool.clone());
                        continue;
                    }
                }

                info!("Creating new pool {}@{}", user.username, pool_name);

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
                            "[pool: {}][user: {}] static passthrough: MD5 pass-the-hash",
                            pool_name, user.username
                        );
                        Some(Arc::new(RwLock::new(BackendAuthMethod::Md5PassTheHash(
                            user.password.clone(),
                        ))))
                    } else if user
                        .password
                        .starts_with(crate::messages::constants::SCRAM_SHA_256)
                    {
                        info!(
                            "[pool: {}][user: {}] static passthrough: SCRAM pending (ClientKey will be set after first client auth)",
                            pool_name, user.username
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
                );

                let queue_strategy = match config.general.server_round_robin {
                    true => QueueMode::Fifo,
                    false => QueueMode::Lifo,
                };

                info!("[pool: {}][user: {}]", pool_name, user.username);

                let mut builder_config = Pool::builder(manager);
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
                        pool_mode: user.pool_mode.unwrap_or(pool_config.pool_mode),
                        user: user.clone(),
                        db: pool_name.clone(),
                        idle_timeout_ms: pool_config
                            .idle_timeout
                            .unwrap_or(config.general.idle_timeout.as_millis()),
                        life_time_ms: pool_config
                            .server_lifetime
                            .unwrap_or(config.general.server_lifetime.as_millis()),
                        sync_server_parameters: config.general.sync_server_parameters,
                    },
                    prepared_statement_cache: match config.general.prepared_statements {
                        false => None,
                        true => Some(Arc::new(PreparedStatementCache::new(
                            prepared_statements_cache_size,
                            config.general.worker_threads,
                        ))),
                    },
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
                            pool_size: aq_config.default_pool_size,
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
                        );

                        let queue_strategy = match config.general.server_round_robin {
                            true => QueueMode::Fifo,
                            false => QueueMode::Lifo,
                        };

                        info!(
                            "[pool: {}][auth_query shared user: {}]",
                            pool_name, shared_user.username
                        );

                        let pool = Pool::builder(manager)
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
                                pool_mode: shared_user.pool_mode.unwrap_or(pool_config.pool_mode),
                                user: shared_user,
                                db: pool_name.clone(),
                                idle_timeout_ms: pool_config
                                    .idle_timeout
                                    .unwrap_or(config.general.idle_timeout.as_millis()),
                                life_time_ms: pool_config
                                    .server_lifetime
                                    .unwrap_or(config.general.server_lifetime.as_millis()),
                                sync_server_parameters: config.general.sync_server_parameters,
                            },
                            prepared_statement_cache: match config.general.prepared_statements {
                                false => None,
                                true => Some(Arc::new(PreparedStatementCache::new(
                                    prepared_statements_cache_size,
                                    config.general.worker_threads,
                                ))),
                            },
                        };

                        new_pools.insert(identifier.clone(), conn_pool);
                    }

                    Some(identifier)
                } else {
                    None // passthrough mode — not implemented yet
                };

                auth_query_states.insert(
                    pool_name.clone(),
                    Arc::new(AuthQueryState {
                        cache_cell: tokio::sync::OnceCell::new(),
                        config: aq_config.clone(),
                        pool_name: pool_name.clone(),
                        server_host: pool_config.server_host.clone(),
                        server_port: pool_config.server_port,
                        shared_pool_id,
                        stats: Arc::new(AuthQueryStats::default()),
                    }),
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
            "Fetching new server parameters from server: {}",
            self.address
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
}

/// Wrapper for the connection pool.
pub struct ServerPool {
    /// Server address.
    address: Address,

    /// Pool user.
    user: User,

    /// Server database.
    database: String,

    /// Client/server mapping.
    client_server_map: ClientServerMap,

    /// Should we clean up dirty connections before putting them into the pool?
    cleanup_connections: bool,

    application_name: String,

    /// Log client parameter status changes
    log_client_parameter_status_changes: bool,

    /// Prepared statement cache size
    prepared_statement_cache_size: usize,

    /// Semaphore to limit concurrent server connection creation.
    create_semaphore: Arc<Semaphore>,

    /// Counter for total connections created (for logging).
    connection_counter: AtomicU64,

    /// Server lifetime in milliseconds (0 = unlimited).
    lifetime_ms: u64,

    /// Idle timeout in milliseconds (0 = disabled).
    /// Connections idle longer than this are closed by retain.
    idle_timeout_ms: u64,

    /// Time after which idle connections should be checked before reuse (0 = disabled).
    idle_check_timeout_ms: u64,

    /// Connect timeout for alive checks.
    connect_timeout: Duration,

    /// Combined pool state: bit 32 = paused, bits 0-31 = reconnect epoch (u32).
    pool_state: AtomicU64,

    /// Notify to wake up clients blocked on PAUSE.
    resume_notify: Notify,
}

impl std::fmt::Debug for ServerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerPool")
            .field("address", &self.address)
            .field("user", &self.user)
            .field("database", &self.database)
            .field("cleanup_connections", &self.cleanup_connections)
            .field("application_name", &self.application_name)
            .field(
                "log_client_parameter_status_changes",
                &self.log_client_parameter_status_changes,
            )
            .field(
                "prepared_statement_cache_size",
                &self.prepared_statement_cache_size,
            )
            .field(
                "connection_counter",
                &self.connection_counter.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl ServerPool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        address: Address,
        user: User,
        database: &str,
        client_server_map: ClientServerMap,
        cleanup_connections: bool,
        log_client_parameter_status_changes: bool,
        prepared_statement_cache_size: usize,
        application_name: String,
        max_concurrent_creates: usize,
        lifetime_ms: u64,
        idle_timeout_ms: u64,
        idle_check_timeout_ms: u64,
        connect_timeout: Duration,
    ) -> ServerPool {
        ServerPool {
            address,
            user: user.clone(),
            database: database.to_string(),
            client_server_map,
            cleanup_connections,
            log_client_parameter_status_changes,
            prepared_statement_cache_size,
            create_semaphore: Arc::new(Semaphore::new(max_concurrent_creates)),
            connection_counter: AtomicU64::new(0),
            application_name,
            lifetime_ms,
            idle_timeout_ms,
            idle_check_timeout_ms,
            connect_timeout,
            pool_state: AtomicU64::new(0),
            resume_notify: Notify::new(),
        }
    }

    /// Attempts to create a new connection.
    /// Uses a semaphore to limit concurrent connection creation instead of serializing with mutex.
    pub async fn create(&self) -> Result<Server, Error> {
        // Acquire semaphore permit to limit concurrent creates
        let _permit = self
            .create_semaphore
            .acquire()
            .await
            .map_err(|_| Error::ServerStartupReadParameters("Semaphore closed".to_string()))?;

        let conn_num = self.connection_counter.fetch_add(1, Ordering::Relaxed) + 1;
        info!(
            "Creating a new server connection to {}[#{}]",
            self.address, conn_num
        );
        let stats = Arc::new(ServerStats::new(
            self.address.clone(),
            crate::utils::clock::now(),
        ));

        stats.register(stats.clone());

        // Connect to the PostgreSQL server.
        match Server::startup(
            &self.address,
            &self.user,
            &self.database,
            self.client_server_map.clone(),
            stats.clone(),
            self.cleanup_connections,
            self.log_client_parameter_status_changes,
            self.prepared_statement_cache_size,
            self.application_name.clone(),
        )
        .await
        {
            Ok(conn) => {
                // Permit is released automatically when _permit goes out of scope
                conn.stats.idle(0);
                Ok(conn)
            }
            Err(err) => {
                // Brief backoff on error to avoid hammering a failing server
                tokio::time::sleep(Duration::from_millis(10)).await;
                stats.disconnect();
                Err(err)
            }
        }
    }

    /// Returns the address of this pool.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Returns the base lifetime in milliseconds for connections in this pool.
    pub fn lifetime_ms(&self) -> u64 {
        self.lifetime_ms
    }

    /// Returns the base idle timeout in milliseconds for connections in this pool.
    pub fn idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms
    }

    /// Bit flag for the paused state within `pool_state`.
    const PAUSED_BIT: u64 = 1 << 32;
    /// Mask for the reconnect epoch (lower 32 bits) within `pool_state`.
    const EPOCH_MASK: u64 = 0xFFFF_FFFF;

    /// Returns whether the pool is paused.
    pub fn is_paused(&self) -> bool {
        self.pool_state.load(Ordering::Acquire) & Self::PAUSED_BIT != 0
    }

    /// Sets the pool as paused.
    pub fn pause(&self) {
        self.pool_state
            .fetch_or(Self::PAUSED_BIT, Ordering::Release);
    }

    /// Resumes the pool and wakes all waiting clients.
    pub fn resume(&self) {
        self.pool_state
            .fetch_and(!Self::PAUSED_BIT, Ordering::Release);
        self.resume_notify.notify_waiters();
    }

    /// Returns a future that completes when the pool is resumed.
    pub fn resume_notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.resume_notify.notified()
    }

    /// Returns the current reconnect epoch.
    pub fn current_epoch(&self) -> u32 {
        (self.pool_state.load(Ordering::Acquire) & Self::EPOCH_MASK) as u32
    }

    /// Increments the reconnect epoch and returns the new value.
    /// Uses CAS loop to modify only the lower 32 bits, preventing
    /// epoch overflow from corrupting PAUSED_BIT at bit 32.
    pub fn bump_epoch(&self) -> u32 {
        loop {
            let old = self.pool_state.load(Ordering::Acquire);
            let old_epoch = (old & Self::EPOCH_MASK) as u32;
            let new_epoch = old_epoch.wrapping_add(1);
            let new = (old & !Self::EPOCH_MASK) | (new_epoch as u64);
            if self
                .pool_state
                .compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return new_epoch;
            }
        }
    }

    /// Checks if the connection can be recycled.
    /// Performs lifetime check and alive check for idle connections.
    pub async fn recycle(&self, conn: &mut Server, metrics: &Metrics) -> RecycleResult {
        if conn.is_bad() {
            return Err(RecycleError::StaticMessage("Bad connection"));
        }

        // RECONNECT epoch check: reject connections created before current epoch
        if metrics.epoch < self.current_epoch() {
            return Err(RecycleError::StaticMessage(
                "Connection outdated (RECONNECT)",
            ));
        }

        // Check server_lifetime - applies to all connections, not just idle
        // Uses per-connection lifetime with jitter to prevent mass closures
        if metrics.lifetime_ms > 0 {
            let age_ms = metrics.age().as_millis() as u64;
            if age_ms > metrics.lifetime_ms {
                warn!(
                    "Connection {} exceeded lifetime ({}ms > {}ms)",
                    conn, age_ms, metrics.lifetime_ms
                );
                return Err(RecycleError::StaticMessage("Connection exceeded lifetime"));
            }
        }

        // Check if connection was idle too long and needs alive check
        if self.idle_check_timeout_ms > 0 {
            if let Some(recycled) = metrics.recycled {
                let idle_time_ms = recycled.elapsed().as_millis() as u64;
                if idle_time_ms > self.idle_check_timeout_ms {
                    debug!(
                        "Connection {} idle for {}ms, checking alive...",
                        conn, idle_time_ms
                    );
                    if conn.check_alive(self.connect_timeout).await.is_err() {
                        warn!(
                            "Connection {} failed alive check after {}ms idle",
                            conn, idle_time_ms
                        );
                        return Err(RecycleError::StaticMessage("Connection failed alive check"));
                    }
                    debug!("Connection {} passed alive check", conn);
                }
            }
        }

        Ok(())
    }
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

/// Create a dynamic data pool for auth_query passthrough mode.
/// Returns the new (or existing) pool. Race-safe: if another thread
/// created the pool concurrently, returns the existing one.
///
/// On RELOAD, dynamic pools are dropped (not in config) and recreated
/// on the next client connection with fresh settings.
pub fn create_dynamic_pool(
    pool_name: &str,
    username: &str,
    backend_auth: Option<BackendAuthMethod>,
) -> Result<ConnectionPool, Error> {
    // Fast path: pool already exists
    if let Some(existing) = get_pool(pool_name, username) {
        // Update backend_auth (credentials may have changed)
        if let (Some(ref ba_lock), Some(new_ba)) = (&existing.address.backend_auth, &backend_auth) {
            debug!(
                "[pool: {pool_name}] auth_query: dynamic pool for '{username}' already exists, updating backend_auth"
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
    let client_server_map = get_client_server_map()
        .ok_or_else(|| Error::AuthError("auth_query: client_server_map not initialized".into()))?;

    let server_database = pool_config
        .server_database
        .clone()
        .unwrap_or_else(|| pool_name.to_string());

    let ba_arc = backend_auth.map(|ba| Arc::new(parking_lot::RwLock::new(ba)));

    let address = Address {
        database: pool_name.to_string(),
        host: pool_config.server_host.clone(),
        port: pool_config.server_port,
        username: username.to_string(),
        password: String::new(),
        pool_name: pool_name.to_string(),
        stats: Arc::new(AddressStats::default()),
        backend_auth: ba_arc,
    };

    let user = User {
        username: username.to_string(),
        password: String::new(),
        pool_size: aq_config.default_pool_size,
        min_pool_size: if aq_config.default_min_pool_size > 0 {
            Some(aq_config.default_min_pool_size)
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

    let application_name = pool_config
        .application_name
        .clone()
        .unwrap_or_else(|| "pg_doorman".to_string());

    let manager = ServerPool::new(
        address.clone(),
        user.clone(),
        server_database.as_str(),
        client_server_map,
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
    );

    let queue_strategy = match config.general.server_round_robin {
        true => QueueMode::Fifo,
        false => QueueMode::Lifo,
    };

    let pool = Pool::builder(manager)
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
        original_server_parameters: Arc::new(tokio::sync::Mutex::new(ServerParameters::new())),
        settings: PoolSettings {
            pool_mode: user.pool_mode.unwrap_or(pool_config.pool_mode),
            user,
            db: pool_name.to_string(),
            idle_timeout_ms: pool_config
                .idle_timeout
                .unwrap_or(config.general.idle_timeout.as_millis()),
            life_time_ms: pool_config
                .server_lifetime
                .unwrap_or(config.general.server_lifetime.as_millis()),
            sync_server_parameters: config.general.sync_server_parameters,
        },
        prepared_statement_cache: match config.general.prepared_statements {
            false => None,
            true => Some(Arc::new(PreparedStatementCache::new(
                prepared_statements_cache_size,
                config.general.worker_threads,
            ))),
        },
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
    info!(
        "[pool: {pool_name}] auth_query: created dynamic passthrough pool for '{username}' (backend_auth={auth_method})"
    );
    new_pools.insert(identifier.clone(), conn_pool.clone());
    POOLS.store(Arc::new(new_pools));
    register_dynamic_pool(&identifier);

    // Prewarm: spawn background task to create default_min_pool_size connections
    if aq_config.default_min_pool_size > 0 {
        let pool_clone = conn_pool.clone();
        let min = aq_config.default_min_pool_size as usize;
        let pn = pool_name.to_string();
        let un = username.to_string();
        tokio::spawn(async move {
            let created = pool_clone.database.replenish(min).await;
            if created > 0 {
                info!(
                    "[pool: {pn}][user: {un}] prewarmed {created} dynamic connection(s) (default_min_pool_size: {min})"
                );
            } else {
                warn!(
                    "[pool: {pn}][user: {un}] dynamic prewarm failed (default_min_pool_size: {min})"
                );
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
