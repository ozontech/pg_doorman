use arc_swap::ArcSwap;
use log::info;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{get_config, Address, General, PoolMode, User};
use crate::errors::Error;
use crate::messages::Parse;

use crate::server::{Server, ServerParameters};
use crate::stats::{AddressStats, ServerStats};

mod errors;
mod inner;
mod types;

pub use errors::{PoolError, RecycleError, RecycleResult};
pub use inner::{Object, Pool, PoolBuilder};
pub use types::{Metrics, PoolConfig, Status, Timeouts};

pub use crate::server::PreparedStatementCache;

pub mod retain;

pub type ProcessId = i32;
pub type SecretKey = i32;
pub type ServerHost = String;
pub type ServerPort = u16;

pub type ClientServerMap =
    Arc<Mutex<HashMap<(ProcessId, SecretKey), (ProcessId, SecretKey, ServerHost, ServerPort)>>>;
pub type PoolMap = HashMap<PoolIdentifierVirtual, ConnectionPool>;

/// The connection pool, globally available.
/// This is atomic and safe and read-optimized.
/// The pool is recreated dynamically when the config is reloaded.
pub static POOLS: Lazy<ArcSwap<PoolMap>> = Lazy::new(|| ArcSwap::from_pointee(HashMap::default()));
pub static CANCELED_PIDS: Lazy<Arc<Mutex<HashSet<ProcessId>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashSet::new())));

pub type PreparedStatementCacheType = Arc<Mutex<PreparedStatementCache>>;
pub type ServerParametersType = Arc<tokio::sync::Mutex<ServerParameters>>;

/// An identifier for a PgDoorman pool,
/// a virtual database pool.
#[derive(Hash, Debug, Clone, PartialEq, Eq, Default)]
pub struct PoolIdentifierVirtual {
    // The name of the database clients want to connect to.
    pub db: String,

    // The username the client connects with. Each user gets its own pool.
    pub user: String,

    // Virtual pool ID
    pub virtual_pool_id: u16,
}

/// An identifier for a PgDoorman pool,
/// a real database visible to clients.
/// Used for statistics.
#[derive(Hash, Debug, Clone, PartialEq, Eq, Default)]
pub struct StatsPoolIdentifier {
    pub db: String,
    pub user: String,
}

impl StatsPoolIdentifier {
    pub fn contains(self, p: PoolIdentifierVirtual) -> bool {
        self.db == p.db && self.user == p.user
    }
}

impl PoolIdentifierVirtual {
    /// Create a new user/pool identifier.
    pub fn new(db: &str, user: &str, virtual_pool_id: u16) -> PoolIdentifierVirtual {
        PoolIdentifierVirtual {
            db: db.to_string(),
            user: user.to_string(),
            virtual_pool_id,
        }
    }
}

impl Display for PoolIdentifierVirtual {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.user, self.db)
    }
}

impl From<&Address> for PoolIdentifierVirtual {
    fn from(address: &Address) -> PoolIdentifierVirtual {
        PoolIdentifierVirtual::new(
            &address.database,
            &address.username,
            address.virtual_pool_id,
        )
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
            idle_timeout_ms: General::default_idle_timeout(),
            life_time_ms: General::default_server_lifetime(),
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
        let config = get_config();

        let mut new_pools = HashMap::new();

        for (pool_name, pool_config) in &config.pools {
            let new_pool_hash_value = pool_config.hash_value();

            // There is one pool per database/user pair.
            for user in pool_config.users.values() {
                for virtual_pool_id in 0..config.general.virtual_pool_count {
                    let old_pool_ref = get_pool(pool_name, &user.username, virtual_pool_id);
                    let identifier =
                        PoolIdentifierVirtual::new(pool_name, &user.username, virtual_pool_id);

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

                    info!(
                        "Creating new pool {}@{}-{}",
                        user.username, pool_name, virtual_pool_id
                    );

                    // real database name on postgresql server.
                    let server_database = pool_config
                        .server_database
                        .clone()
                        .unwrap_or(pool_name.clone().to_string());

                    let address = Address {
                        database: pool_name.clone(),
                        host: pool_config.server_host.clone(),
                        port: pool_config.server_port,
                        virtual_pool_id,
                        username: user.username.clone(),
                        password: user.password.clone(),
                        pool_name: pool_name.clone(),
                        stats: Arc::new(AddressStats::default()),
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
                    );

                    info!(
                        "[pool: {}][user: {}][vpid: {}]",
                        pool_name, user.username, virtual_pool_id
                    );

                    let mut builder_config = Pool::builder(manager);
                    builder_config = builder_config.config(PoolConfig {
                        max_size: (user.pool_size / config.general.virtual_pool_count as u32)
                            as usize,
                        timeouts: Timeouts {
                            wait: Some(Duration::from_millis(config.general.query_wait_timeout)),
                            create: Some(Duration::from_millis(config.general.connect_timeout)),
                            recycle: None,
                        },
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
                            idle_timeout_ms: config.general.idle_timeout,
                            life_time_ms: config.general.server_lifetime,
                            sync_server_parameters: config.general.sync_server_parameters,
                        },
                        prepared_statement_cache: match config.general.prepared_statements {
                            false => None,
                            true => Some(Arc::new(Mutex::new(PreparedStatementCache::new(
                                config.general.prepared_statements_cache_size,
                            )))),
                        },
                    };

                    // There is one pool per database/user pair.
                    new_pools.insert(
                        PoolIdentifierVirtual::new(pool_name, &user.username, virtual_pool_id),
                        pool,
                    );
                }
            }
        }

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
        match self.prepared_statement_cache {
            Some(ref prepared_statement_cache) => {
                let mut cache = prepared_statement_cache.lock();
                Some(cache.get_or_insert(parse, hash))
            }
            None => None,
        }
    }

    /// Promote a prepared statement hash in the LRU
    #[inline(always)]
    pub fn promote_prepared_statement_hash(&self, hash: &u64) {
        // We should only be calling this function if the cache is enabled
        if let Some(ref prepared_statement_cache) = self.prepared_statement_cache {
            let mut cache = prepared_statement_cache.lock();
            cache.promote(hash);
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
#[derive(Debug)]
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

    /// Lock to limit of server connections creating concurrently.
    open_new_server: Arc<tokio::sync::Mutex<u64>>,
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
    ) -> ServerPool {
        ServerPool {
            address,
            user: user.clone(),
            database: database.to_string(),
            client_server_map,
            cleanup_connections,
            log_client_parameter_status_changes,
            prepared_statement_cache_size,
            open_new_server: Arc::new(tokio::sync::Mutex::new(0)),
            application_name,
        }
    }

    /// Attempts to create a new connection.
    pub async fn create(&self) -> Result<Server, Error> {
        let mut guard = self.open_new_server.lock().await;
        *guard += 1;
        info!(
            "Creating a new server connection to {}[#{}]",
            self.address, guard
        );
        let stats = Arc::new(ServerStats::new(
            self.address.clone(),
            tokio::time::Instant::now(),
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
                // max rate limit 1 server connection per 10 ms.
                tokio::time::sleep(Duration::from_millis(10)).await;
                drop(guard);
                conn.stats.idle(0);
                Ok(conn)
            }
            Err(err) => {
                // if server feels bad sleep more.
                tokio::time::sleep(Duration::from_millis(50)).await;
                drop(guard);
                stats.disconnect();
                Err(err)
            }
        }
    }

    /// Checks if the connection can be recycled.
    pub async fn recycle(&self, conn: &mut Server, _: &Metrics) -> RecycleResult {
        if conn.is_bad() {
            return Err(RecycleError::StaticMessage("Bad connection"));
        }
        Ok(())
    }
}

/// Get the connection pool
pub fn get_pool(db: &str, user: &str, virtual_pool_id: u16) -> Option<ConnectionPool> {
    (*(*POOLS.load()))
        .get(&PoolIdentifierVirtual::new(db, user, virtual_pool_id))
        .cloned()
}

/// Get a pointer to all configured pools.
pub fn get_all_pools() -> HashMap<PoolIdentifierVirtual, ConnectionPool> {
    (*(*POOLS.load())).clone()
}
