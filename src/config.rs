use crate::constants::JWT_PUB_KEY_PASSWORD_PREFIX;
use arc_swap::ArcSwap;
use bytes::{BufMut, BytesMut};
use ipnet::IpNet;
use log::{error, info};
use once_cell::sync::Lazy;
use serde_derive::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::mem;
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::errors::Error;
use crate::jwt_auth::load_jwt_pub_key;
use crate::pool::{ClientServerMap, ConnectionPool};
use crate::stats::AddressStats;
use crate::tls::load_identity;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Globally available configuration.
static CONFIG: Lazy<ArcSwap<Config>> = Lazy::new(|| ArcSwap::from_pointee(Config::default()));

/// Address identifying a PostgreSQL server uniquely.
#[derive(Clone, Debug)]
pub struct Address {
    /// Server host.
    pub host: String,
    /// Server port.
    pub port: u16,
    /// Virtual pool ID
    pub virtual_pool_id: u16,
    /// The name of the Postgres database.
    pub database: String,
    /// The name of the user configured to use this pool.
    pub username: String,
    /// The password of the user configured to use this pool
    pub password: String,
    /// The name of this pool (i.e. database name visible to the client).
    pub pool_name: String,
    /// Address stats
    pub stats: Arc<AddressStats>,
    /// Number of errors encountered since last successful checkout
    pub error_count: Arc<AtomicU64>,
}

impl Default for Address {
    fn default() -> Address {
        Address {
            host: String::from("127.0.0.1"),
            port: 5432,
            virtual_pool_id: 0,
            database: String::from("database"),
            username: String::from("username"),
            password: String::from("password"),
            pool_name: String::from("pool_name"),
            stats: Arc::new(AddressStats::default()),
            error_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "vp-{}-{}@{}:{}/{}",
            self.virtual_pool_id, self.username, self.host, self.port, self.database
        )
    }
}

// We need to implement PartialEq by ourselves so we skip stats in the comparison
impl PartialEq for Address {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
            && self.port == other.port
            && self.virtual_pool_id == other.virtual_pool_id
            && self.database == other.database
            && self.username == other.username
            && self.pool_name == other.pool_name
    }
}
impl Eq for Address {}

// We need to implement Hash by ourselves so we skip stats in the comparison
impl Hash for Address {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.host.hash(state);
        self.port.hash(state);
        self.virtual_pool_id.hash(state);
        self.database.hash(state);
        self.username.hash(state);
        self.pool_name.hash(state);
    }
}

impl Address {
    /// Address name (aka database) used in `SHOW STATS`, `SHOW DATABASES`, and `SHOW POOLS`.
    pub fn name(&self) -> String {
        self.pool_name.clone() + "-" + &*self.virtual_pool_id.to_string()
    }
    pub fn error_count(&self) -> u64 {
        self.error_count.load(Ordering::Relaxed)
    }

    pub fn increment_error_count(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reset_error_count(&self) {
        self.error_count.store(0, Ordering::Relaxed);
    }
}

/// Pool mode:
/// - transaction: server serves one transaction,
/// - session: server is attached to the client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy, Hash)]
pub enum PoolMode {
    #[serde(alias = "transaction", alias = "Transaction")]
    Transaction,

    #[serde(alias = "session", alias = "Session")]
    Session,
}

impl Display for PoolMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match *self {
            PoolMode::Transaction => "transaction".to_string(),
            PoolMode::Session => "session".to_string(),
        };
        write!(f, "{}", str)
    }
}

/// PostgreSQL user.
#[derive(Clone, PartialEq, Hash, Eq, Serialize, Deserialize, Debug)]
pub struct User {
    pub username: String,
    pub password: String,
    pub pool_size: u32,
    pub min_pool_size: Option<u32>,
    pub pool_mode: Option<PoolMode>,
    pub server_lifetime: Option<u64>,
    // If the server_username parameter is specified,
    // authorization on the server will be performed using the credentials
    // of THIS server_user and server_password.
    pub server_username: Option<String>,
    pub server_password: Option<String>,
}

impl Default for User {
    fn default() -> User {
        User {
            username: String::from("postgres"),
            password: String::from(""),
            pool_size: 40,
            min_pool_size: None,
            pool_mode: None,
            server_lifetime: None,
            server_username: None,
            server_password: None,
        }
    }
}

impl User {
    async fn validate(&self) -> Result<(), Error> {
        if self.password.starts_with(JWT_PUB_KEY_PASSWORD_PREFIX) {
            let jwt_pub_key_file = self
                .password
                .strip_prefix(JWT_PUB_KEY_PASSWORD_PREFIX)
                .unwrap()
                .to_string();
            load_jwt_pub_key(jwt_pub_key_file).await?;
        }
        if (self.server_password.is_some() && self.server_username.is_none())
            || (self.server_password.is_none() && self.server_username.is_some())
        {
            return Err(Error::BadConfig(
                "both the server_password and server_username must be specified at the same time"
                    .to_string(),
            ));
        }
        if let Some(min_pool_size) = self.min_pool_size {
            if min_pool_size > self.pool_size {
                return Err(Error::BadConfig(format!(
                    "min_pool_size of {} cannot be larger than pool_size of {}",
                    min_pool_size, self.pool_size
                )));
            }
        };

        Ok(())
    }
}

/// General configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct General {
    #[serde(default = "General::default_host")]
    pub host: String,

    #[serde(default = "General::default_port")]
    pub port: u16,

    #[serde(default = "General::default_virtual_pool_count")]
    pub virtual_pool_count: u16,

    #[serde(default = "General::default_tokio_global_queue_interval")]
    pub tokio_global_queue_interval: u32,

    #[serde(default = "General::default_tokio_event_interval")]
    pub tokio_event_interval: u32,

    #[serde(default = "General::default_connect_timeout")]
    pub connect_timeout: u64,

    #[serde(default = "General::default_query_wait_timeout")]
    pub query_wait_timeout: u64,

    #[serde(default = "General::default_idle_timeout")]
    pub idle_timeout: u64,

    #[serde(default = "General::default_tcp_keepalives_idle")]
    pub tcp_keepalives_idle: u64,
    #[serde(default = "General::default_tcp_keepalives_count")]
    pub tcp_keepalives_count: u32,
    #[serde(default = "General::default_tcp_keepalives_interval")]
    pub tcp_keepalives_interval: u64,
    #[serde(default = "General::default_tcp_so_linger")]
    pub tcp_so_linger: u64,
    #[serde(default = "General::default_tcp_no_delay")]
    pub tcp_no_delay: bool,

    #[serde(default = "General::default_unix_socket_buffer_size")]
    pub unix_socket_buffer_size: usize,

    #[serde(default)] // True
    pub log_client_connections: bool,

    #[serde(default)] // True
    pub log_client_disconnections: bool,

    #[serde(default = "General::default_shutdown_timeout")] // 10_000
    pub shutdown_timeout: u64,

    #[serde(default = "General::default_message_size_to_be_stream")] // 1024 * 1024
    pub message_size_to_be_stream: u32,

    #[serde(default = "General::default_max_memory_usage")] // 1m
    pub max_memory_usage: u64,

    #[serde(default = "General::default_max_connections")]
    pub max_connections: u64,

    #[serde(default = "General::default_server_lifetime")]
    pub server_lifetime: u64,

    #[serde(default = "General::default_server_round_robin")] // False
    pub server_round_robin: bool,

    #[serde(default = "General::default_sync_server_parameters")] // False
    pub sync_server_parameters: bool,

    #[serde(default = "General::default_worker_threads")]
    pub worker_threads: usize,

    #[serde(default = "General::default_proxy_copy_data_timeout")] // 15_000
    pub proxy_copy_data_timeout: u64,

    // worker_cpu_affinity_pinning: пытаемся пинить каждый worker на CPU, начиная со второго CPU.
    #[serde(default = "General::default_worker_cpu_affinity_pinning")]
    pub worker_cpu_affinity_pinning: bool,
    // worker_stack_size: размера стэка каждого воркера.
    #[serde(default = "General::default_worker_stack_size")] // 8388608
    pub worker_stack_size: usize,
    // tcp backlog.
    #[serde(default = "General::default_backlog")]
    pub backlog: u32,

    // pooler_check_query: ping pooler with simple query like '/* ping pooler */;'.
    #[serde(default = "General::default_pooler_check_query")]
    pub pooler_check_query: String,
    pooler_check_query_request_bytes: Option<Vec<u8>>,
    pooler_check_query_response_bytes: Option<Vec<u8>>,

    pub tls_certificate: Option<String>,
    pub tls_private_key: Option<String>,
    #[serde(default = "General::default_tls_rate_limit_per_second")]
    pub tls_rate_limit_per_second: usize,

    #[serde(default)] // false
    pub server_tls: bool,

    #[serde(default)] // false
    pub verify_server_certificate: bool,

    pub admin_username: String,
    pub admin_password: String,

    #[serde(default = "General::default_prepared_statements")]
    pub prepared_statements: bool,

    #[serde(default = "General::default_prepared_statements_cache_size")]
    pub prepared_statements_cache_size: usize,

    #[serde(default = "General::default_daemon_pid_file")]
    pub daemon_pid_file: String, // can be enabled only in daemon mode.

    pub syslog_prog_name: Option<String>,

    #[serde(default = "General::default_hba")]
    pub hba: Vec<IpNet>,
}

impl General {
    pub fn default_host() -> String {
        "0.0.0.0".into()
    }

    pub fn default_port() -> u16 {
        5432
    }

    pub fn default_virtual_pool_count() -> u16 {
        1
    }

    pub fn default_tokio_global_queue_interval() -> u32 {
        5
    }

    pub fn default_tokio_event_interval() -> u32 {
        1
    }

    pub fn default_tls_rate_limit_per_second() -> usize {
        0
    }
    pub fn default_server_lifetime() -> u64 {
        1000 * 60 * 5 // 5 min
    }

    pub fn default_connect_timeout() -> u64 {
        3_000
    }

    pub fn default_query_wait_timeout() -> u64 {
        5000
    }

    pub fn default_tcp_so_linger() -> u64 {
        0 // 0 seconds
    }

    pub fn default_unix_socket_buffer_size() -> usize {
        1024 * 1024 // 1mb
    }

    pub fn default_worker_cpu_affinity_pinning() -> bool {
        true
    }

    pub fn default_worker_stack_size() -> usize {
        8 * 1024 * 1024
    }

    pub fn default_max_memory_usage() -> u64 {
        256 * 1024 * 1024
    }

    pub fn default_max_connections() -> u64 {
        8 * 1024
    }

    pub fn default_backlog() -> u32 {
        0
    }

    pub fn default_tcp_no_delay() -> bool {
        true
    }

    pub fn default_sync_server_parameters() -> bool {
        false
    }

    // These keepalive defaults should detect a dead connection within 30 seconds.
    // Tokio defaults to disabling keepalives which keeps dead connections around indefinitely.
    // This can lead to permanent server pool exhaustion
    pub fn default_tcp_keepalives_idle() -> u64 {
        5 // 5 seconds
    }

    pub fn default_tcp_keepalives_count() -> u32 {
        5 // 5 time
    }

    pub fn default_tcp_keepalives_interval() -> u64 {
        5 // 5 seconds
    }

    pub fn default_idle_timeout() -> u64 {
        300_000_000 // 5 minutes
    }

    pub fn default_shutdown_timeout() -> u64 {
        10_000
    }

    pub fn default_proxy_copy_data_timeout() -> u64 {
        15_000
    }

    pub fn default_message_size_to_be_stream() -> u32 {
        1024 * 1024
    }

    pub fn default_worker_threads() -> usize {
        4
    }

    pub fn default_idle_client_in_transaction_timeout() -> u64 {
        0
    }

    pub fn default_server_round_robin() -> bool {
        false
    }

    pub fn default_prepared_statements_cache_size() -> usize {
        8 * 1024
    }
    pub fn default_prepared_statements() -> bool {
        true
    }

    pub fn default_daemon_pid_file() -> String {
        "/tmp/pg_doorman.pid".to_string()
    }

    pub fn default_pooler_check_query() -> String {
        ";".to_string()
    }

    pub fn poller_check_query_request_bytes_vec(mut self) -> Vec<u8> {
        if self.pooler_check_query_request_bytes.is_some() {
            return self.pooler_check_query_request_bytes.unwrap();
        }
        let mut buf = BytesMut::from(&b"Q"[..]);
        buf.put_i32(self.pooler_check_query.len() as i32 + mem::size_of::<i32>() as i32 + 1);
        buf.put_slice(self.pooler_check_query.as_bytes());
        buf.put_u8(b'\0');
        self.pooler_check_query_request_bytes = Option::from(buf.to_vec());
        self.pooler_check_query_request_bytes.unwrap()
    }
    pub fn poller_check_query_response_bytes_vec(mut self) -> Vec<u8> {
        if self.pooler_check_query_response_bytes.is_some() {
            return self.pooler_check_query_response_bytes.unwrap();
        }
        let mut res = BytesMut::with_capacity(128);
        res.put_u8(b'I');
        res.put_i32(mem::size_of::<i32>() as i32);
        res.put_u8(b'Z');
        res.put_i32(mem::size_of::<i32>() as i32 + 1);
        res.put_u8(b'I');
        self.pooler_check_query_response_bytes = Option::from(res.to_vec());
        self.pooler_check_query_response_bytes.unwrap()
    }

    pub fn default_hba() -> Vec<IpNet> {
        vec![]
    }

    pub fn default_include_files() -> Vec<String> {
        vec![]
    }

    pub fn default_include() -> Include {
        Include {
            files: Self::default_include_files(),
        }
    }
}

impl Default for General {
    fn default() -> General {
        General {
            host: Self::default_host(),
            port: Self::default_port(),
            virtual_pool_count: Self::default_virtual_pool_count(),
            tokio_global_queue_interval: Self::default_tokio_global_queue_interval(),
            tokio_event_interval: Self::default_tokio_event_interval(),
            connect_timeout: General::default_connect_timeout(),
            query_wait_timeout: General::default_query_wait_timeout(),
            idle_timeout: General::default_idle_timeout(),
            shutdown_timeout: Self::default_shutdown_timeout(),
            proxy_copy_data_timeout: Self::default_proxy_copy_data_timeout(),
            message_size_to_be_stream: Self::default_message_size_to_be_stream(),
            max_memory_usage: Self::default_max_memory_usage(),
            max_connections: Self::default_max_connections(),
            worker_threads: Self::default_worker_threads(),
            worker_cpu_affinity_pinning: Self::default_worker_cpu_affinity_pinning(),
            worker_stack_size: Self::default_worker_stack_size(),
            tcp_keepalives_idle: Self::default_tcp_keepalives_idle(),
            tcp_keepalives_count: Self::default_tcp_keepalives_count(),
            tcp_keepalives_interval: Self::default_tcp_keepalives_interval(),
            tcp_so_linger: Self::default_tcp_so_linger(),
            tcp_no_delay: Self::default_tcp_no_delay(),
            unix_socket_buffer_size: Self::default_unix_socket_buffer_size(),
            log_client_connections: true,
            log_client_disconnections: true,
            sync_server_parameters: Self::default_sync_server_parameters(),
            tls_certificate: None,
            tls_private_key: None,
            tls_rate_limit_per_second: Self::default_tls_rate_limit_per_second(),
            server_tls: false,
            verify_server_certificate: false,
            admin_username: String::from("admin"),
            admin_password: String::from("admin"),
            server_lifetime: Self::default_server_lifetime(),
            server_round_robin: Self::default_server_round_robin(),
            prepared_statements: Self::default_prepared_statements(),
            prepared_statements_cache_size: Self::default_prepared_statements_cache_size(),
            hba: vec![],
            daemon_pid_file: Self::default_daemon_pid_file(),
            syslog_prog_name: None,
            pooler_check_query: Self::default_pooler_check_query(),
            pooler_check_query_request_bytes: None,
            pooler_check_query_response_bytes: None,
            backlog: Self::default_backlog(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pool {
    #[serde(default = "Pool::default_pool_mode")]
    pub pool_mode: PoolMode,

    /// Maximum time to allow for establishing a new server connection.
    pub connect_timeout: Option<u64>,

    /// Close idle connections that have been opened for longer than this.
    pub idle_timeout: Option<u64>,

    /// Close server connections that have been opened for longer than this.
    /// Only applied to idle connections. If the connection is actively used for
    /// longer than this period, the pool will not interrupt it.
    pub server_lifetime: Option<u64>,

    #[serde(default = "Pool::default_cleanup_server_connections")]
    pub cleanup_server_connections: bool,

    #[serde(default)] // False
    pub log_client_parameter_status_changes: bool,

    pub application_name: Option<String>,

    #[serde(default = "Pool::default_server_host")]
    pub server_host: String,

    #[serde(default = "Pool::default_server_port")]
    pub server_port: u16,

    // The real name of the database on the server. If it is not specified, the pool name is used.
    pub server_database: Option<String>,

    pub prepared_statements_cache_size: Option<usize>,

    pub users: BTreeMap<String, User>,
    // Note, don't put simple fields below these configs. There's a compatibility issue with TOML that makes it
    // incompatible to have simple fields in TOML after complex objects. See
    // https://users.rust-lang.org/t/why-toml-to-string-get-error-valueaftertable/85903
}

impl Pool {
    pub fn hash_value(&self) -> u64 {
        let mut s = DefaultHasher::new();
        self.hash(&mut s);
        s.finish()
    }

    pub fn default_pool_mode() -> PoolMode {
        PoolMode::Transaction
    }

    pub fn default_server_port() -> u16 {
        5432
    }

    pub fn default_server_host() -> String {
        String::from("127.0.0.1")
    }

    pub fn default_cleanup_server_connections() -> bool {
        true
    }

    pub async fn validate(&mut self) -> Result<(), Error> {
        for user in self.users.values() {
            user.validate().await?;
        }

        Ok(())
    }
}

impl Default for Pool {
    fn default() -> Pool {
        Pool {
            pool_mode: Self::default_pool_mode(),
            users: BTreeMap::default(),
            server_port: 5432,
            server_host: String::from("127.0.0.1"),
            server_database: None,
            connect_timeout: None,
            idle_timeout: None,
            server_lifetime: None,
            cleanup_server_connections: true,
            log_client_parameter_status_changes: false,
            application_name: None,
            prepared_statements_cache_size: None,
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug, Hash, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Include {
    #[serde(default = "General::default_include_files")]
    pub files: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GeneralWithInclude {
    #[serde(default = "General::default_include")]
    pub include: Include,
}

/// Configuration wrapper.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Config {
    // Serializer maintains the order of fields in the struct
    // so we should always put simple fields before nested fields
    // in all serializable structs to avoid ValueAfterTable errors
    // These errors occur when the toml serializer is about to produce
    // ambiguous toml structure like the one below
    // [main]
    // field1_under_main = 1
    // field2_under_main = 2
    // [main.subconf]
    // field1_under_subconf = 1
    // field3_under_main = 3 # This field will be interpreted as being under subconf and not under main
    #[serde(default = "Config::default_path")]
    pub path: String,

    // General and global settings.
    pub general: General,

    // Connection pools.
    pub pools: HashMap<String, Pool>,

    // Include files.
    #[serde(default = "General::default_include")]
    pub include: Include,
}

impl Config {
    pub fn default_path() -> String {
        String::from("pg_doorman.toml")
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            path: Self::default_path(),
            general: General::default(),
            pools: HashMap::default(),
            include: Include { files: Vec::new() },
        }
    }
}

impl From<&Config> for std::collections::HashMap<String, String> {
    fn from(config: &Config) -> HashMap<String, String> {
        let mut r: Vec<(String, String)> = config
            .pools
            .iter()
            .flat_map(|(pool_name, pool)| {
                [
                    (
                        format!("pools.{}.pool_mode", pool_name),
                        pool.pool_mode.to_string(),
                    ),
                    (
                        format!("pools.{:?}.users", pool_name),
                        pool.users
                            .values()
                            .map(|user| &user.username)
                            .cloned()
                            .collect::<Vec<String>>()
                            .join(", "),
                    ),
                ]
            })
            .collect();

        let mut static_settings = vec![
            ("host".to_string(), config.general.host.to_string()),
            ("port".to_string(), config.general.port.to_string()),
            (
                "connect_timeout".to_string(),
                config.general.connect_timeout.to_string(),
            ),
            (
                "idle_timeout".to_string(),
                config.general.idle_timeout.to_string(),
            ),
            (
                "shutdown_timeout".to_string(),
                config.general.shutdown_timeout.to_string(),
            ),
        ];

        r.append(&mut static_settings);
        r.iter().cloned().collect()
    }
}

impl Config {
    /// Print current configuration.
    pub fn show(&self) {
        info!("Worker threads: {}", self.general.worker_threads);
        info!("Connection timeout: {}ms", self.general.connect_timeout);
        info!("Idle timeout: {}ms", self.general.idle_timeout);
        info!(
            "Log client connections: {}",
            self.general.log_client_connections
        );
        info!(
            "Log client disconnections: {}",
            self.general.log_client_disconnections
        );
        info!("Shutdown timeout: {}ms", self.general.shutdown_timeout);
        info!(
            "Message size to be steam: {}",
            self.general.message_size_to_be_stream
        );
        info!(
            "Max memory usage for processing messages: {}",
            self.general.max_memory_usage
        );
        info!(
            "Default max server lifetime: {}ms",
            self.general.server_lifetime
        );
        info!("Backlog: {}", self.general.backlog);
        info!("Max connections: {}", self.general.max_connections);
        info!("Sever round robin: {}", self.general.server_round_robin);
        info!("HBA config: {:?}", self.general.hba);
        match self.general.tls_certificate.clone() {
            Some(tls_certificate) => {
                info!("TLS certificate: {}", tls_certificate);

                if let Some(tls_private_key) = self.general.tls_private_key.clone() {
                    info!("TLS private key: {}", tls_private_key);
                    info!("TLS support is enabled");
                }
            }

            None => {
                info!("TLS support is disabled");
            }
        };
        info!("Server TLS enabled: {}", self.general.server_tls);
        info!(
            "Server TLS certificate verification: {}",
            self.general.verify_server_certificate
        );
        info!("Prepared statements: {}", self.general.prepared_statements);
        if self.general.prepared_statements {
            info!(
                "Prepared statements server cache size: {}",
                self.general.prepared_statements_cache_size
            );
        }

        for (pool_name, pool_config) in &self.pools {
            // TODO: Make this output prettier (maybe a table?)
            info!(
                "[pool: {}] Maximum user connections: {}",
                pool_name,
                pool_config
                    .users
                    .values()
                    .map(|user_cfg| user_cfg.pool_size)
                    .sum::<u32>()
            );
            info!(
                "[pool: {}] Default pool mode: {}",
                pool_name, pool_config.pool_mode
            );
            let connect_timeout = pool_config
                .connect_timeout
                .unwrap_or(self.general.connect_timeout);
            info!(
                "[pool: {}] Connection timeout: {}ms",
                pool_name, connect_timeout
            );
            let idle_timeout = pool_config
                .idle_timeout
                .unwrap_or(self.general.idle_timeout);
            info!("[pool: {}] Idle timeout: {}ms", pool_name, idle_timeout);
            info!(
                "[pool: {}] Number of users: {}",
                pool_name,
                pool_config.users.len()
            );
            info!(
                "[pool: {}] Max server lifetime: {}",
                pool_name,
                match pool_config.server_lifetime {
                    Some(server_lifetime) => format!("{}ms", server_lifetime),
                    None => "default".to_string(),
                }
            );
            info!(
                "[pool: {}] Cleanup server connections: {}",
                pool_name, pool_config.cleanup_server_connections
            );
            info!(
                "[pool: {}] Log client parameter status changes: {}",
                pool_name, pool_config.log_client_parameter_status_changes
            );

            for user in &pool_config.users {
                info!(
                    "[pool: {}][user: {}] Pool size: {}",
                    pool_name, user.1.username, user.1.pool_size,
                );
                info!(
                    "[pool: {}][user: {}] Minimum pool size: {}",
                    pool_name,
                    user.1.username,
                    user.1.min_pool_size.unwrap_or(0)
                );
                info!(
                    "[pool: {}][user: {}] Pool mode: {}",
                    pool_name,
                    user.1.username,
                    match user.1.pool_mode {
                        Some(pool_mode) => pool_mode.to_string(),
                        None => pool_config.pool_mode.to_string(),
                    }
                );
                info!(
                    "[pool: {}][user: {}] Max server lifetime: {}",
                    pool_name,
                    user.1.username,
                    match user.1.server_lifetime {
                        Some(server_lifetime) => format!("{}ms", server_lifetime),
                        None => "default".to_string(),
                    }
                );
            }
        }
    }

    pub async fn validate(&mut self) -> Result<(), Error> {
        for (name, pool) in self.pools.iter() {
            for (_name, user_data) in pool.users.iter() {
                if self.general.virtual_pool_count > user_data.pool_size as u16 {
                    return Err(Error::BadConfig(format!(
                        "Error in pool {{ {} }}. \
                    Please set virtual_pool_count less then pool_size.",
                        name
                    )));
                }
                if user_data.password.is_empty() {
                    return Err(Error::BadConfig(format!(
                        "Error in pool {{ {} }}. \
                        You don't have to specify a user password for every pool",
                        name
                    )));
                }
            }
        }

        if self.general.tls_rate_limit_per_second < 100
            && self.general.tls_rate_limit_per_second != 0
        {
            return Err(Error::BadConfig(
                "tls rate limit should be > 100".to_string(),
            ));
        }
        if self.general.tls_rate_limit_per_second % 100 != 0 {
            return Err(Error::BadConfig(
                "tls rate limit should be multiple 100".to_string(),
            ));
        }

        // Validate prepared_statements
        if self.general.prepared_statements && self.general.prepared_statements_cache_size == 0 {
            return Err(Error::BadConfig("The value of prepared_statements_cache should be greater than 0 if prepared_statements are enabled".to_string()));
        }

        // Validate TLS!
        if let Some(tls_certificate) = self.general.tls_certificate.clone() {
            if let Some(tls_private_key) = self.general.tls_private_key.clone() {
                match load_identity(Path::new(&tls_certificate), Path::new(&tls_private_key)) {
                    Ok(_) => (),
                    Err(err) => {
                        return Err(Error::BadConfig(format!(
                            "tls is incorrectly configured: {:?}",
                            err
                        )));
                    }
                }
            }
        };
        for pool in self.pools.values_mut() {
            pool.validate().await?;
        }

        Ok(())
    }
}

/// Get a read-only instance of the configuration
/// from anywhere in the app.
/// ArcSwap makes this cheap and quick.
pub fn get_config() -> Config {
    (*(*CONFIG.load())).clone()
}

async fn load_file(path: &str) -> Result<String, Error> {
    let mut contents = String::new();
    let mut file = match File::open(path).await {
        Ok(file) => file,
        Err(err) => {
            return Err(Error::BadConfig(format!(
                "Could not open '{}': {}",
                path, err
            )));
        }
    };
    match file.read_to_string(&mut contents).await {
        Ok(_) => (),
        Err(err) => {
            return Err(Error::BadConfig(format!(
                "Could not read config file: {}",
                err
            )));
        }
    };
    Ok(contents)
}

/// Parse the configuration file located at the path.
pub async fn parse(path: &str) -> Result<(), Error> {
    // parse only include.files = ["./path/to/file",...]
    let include_only_config_contents = load_file(path).await?;
    let include_config: GeneralWithInclude = match toml::from_str(&include_only_config_contents) {
        Ok(config) => config,
        Err(err) => {
            return Err(Error::BadConfig(format!(
                "Could not parse config file {}: {}",
                path, err
            )));
        }
    };

    // merge main with include files via serge-toml-merge.
    let mut config_merged = match load_file(path).await?.parse() {
        Ok(value) => value,
        Err(err) => {
            return Err(Error::BadConfig(format!(
                "Could not toml parse file {}: {:?}",
                path, err
            )));
        }
    };
    for file in include_config.include.files {
        info!("Merge config with include file: {}", file);
        let include_file_content = load_file(file.as_str()).await?;
        let include_file_value = match include_file_content.parse() {
            Ok(value) => value,
            Err(err) => {
                return Err(Error::BadConfig(format!(
                    "Could not toml parse file {}: {:?}",
                    file, err
                )));
            }
        };
        config_merged = match serde_toml_merge::merge(config_merged, include_file_value) {
            Ok(value) => value,
            Err(err) => {
                return Err(Error::BadConfig(format!(
                    "Could merge config file {}: {:?}",
                    file, err
                )));
            }
        };
    }

    let table = config_merged.as_table().unwrap();
    let mut config: Config = match toml::from_str(&table.to_string()) {
        Ok(config) => config,
        Err(err) => {
            return Err(Error::BadConfig(format!(
                "Could not merge config: {:?}",
                err
            )));
        }
    };

    config.validate().await?;

    config.path = path.to_string();

    // Update the configuration globally.
    CONFIG.store(Arc::new(config.clone()));

    Ok(())
}

pub async fn reload_config(client_server_map: ClientServerMap) -> Result<bool, Error> {
    let old_config = get_config();

    match parse(&old_config.path).await {
        Ok(()) => (),
        Err(err) => {
            error!("Config reload error: {:?}", err);
            return Err(Error::BadConfig(format!("Config reload error: {:?}", err)));
        }
    };

    let new_config = get_config();

    if old_config != new_config {
        info!("Config changed, reloading");
        ConnectionPool::from_config(client_server_map).await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn addr_in_hba(addr: IpAddr) -> bool {
    let config = get_config();
    if config.general.hba.is_empty() {
        return true;
    }
    config.general.hba.iter().any(|net| net.contains(&addr))
}

#[cfg(test)]
mod test {
    use super::*;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_config() {
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("tests.toml");
        parse(file.as_os_str().to_str().unwrap()).await.unwrap();

        assert_eq!(get_config().general.idle_timeout, 300000000);
        assert_eq!(get_config().pools.len(), 4);
        assert_eq!(get_config().pools["example_db"].idle_timeout, Some(40000));
        assert_eq!(get_config().pools["example_db"].users.len(), 4);
        assert_eq!(
            get_config().pools["example_db"].users["0"].username,
            "example_user_1"
        );
        assert_eq!(
            get_config().pools["example_db"].users["1"].password,
            "SCRAM-SHA-256$4096:p2j/1lMdQF6r1dD9I9f7PQ==$H3xt5yh7lwSq9zUPYwHovRu3FyUCCXchG/skydJRa9o=:5xU6Wj/GNg3UnN2uQIx3ezx7uZyzGeM5NrvSJRIxnlw="
        );
        assert_eq!(get_config().pools["example_db"].users["1"].pool_size, 20);
        assert_eq!(
            get_config().pools["example_db"].users["1"].username,
            "example_user_2"
        );
        assert_eq!(get_config().pools["example_db"].users["0"].pool_size, 40);
        assert_eq!(
            get_config().pools["example_db"].users["0"].pool_mode,
            Some(PoolMode::Transaction)
        );
        assert!(addr_in_hba(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!addr_in_hba(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(addr_in_hba(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
    }

    #[tokio::test]
    async fn test_serialize_configs() {
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("tests.toml");
        parse(file.as_os_str().to_str().unwrap()).await.unwrap();
        print!("{}", toml::to_string(&get_config()).unwrap());
    }
}
