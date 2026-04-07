//! Connection pool configuration.

use crate::errors::Error;
use log::warn;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};

use super::{Duration, PoolMode, User};

/// Custom deserializer for users field that supports both formats:
/// - Array format (recommended): `users: [{ username: "user1", ... }]`
/// - Map format (legacy TOML): `users: { "0": { username: "user1", ... } }`
fn deserialize_users<'de, D>(deserializer: D) -> Result<Vec<User>, D::Error>
where
    D: Deserializer<'de>,
{
    struct UsersVisitor;

    impl<'de> Visitor<'de> for UsersVisitor {
        type Value = Vec<User>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a sequence of users or a map with string keys")
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Vec<User>, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut users = Vec::new();
            while let Some(user) = seq.next_element()? {
                users.push(user);
            }
            Ok(users)
        }

        fn visit_map<M>(self, mut map: M) -> Result<Vec<User>, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut users = Vec::new();
            while let Some((key, user)) = map.next_entry::<String, User>()? {
                // Validate that key is a valid index (for legacy format)
                if key.parse::<usize>().is_err() {
                    return Err(de::Error::custom(format!(
                        "invalid user key '{}': expected numeric index or use array format",
                        key
                    )));
                }
                users.push(user);
            }
            Ok(users)
        }
    }

    deserializer.deserialize_any(UsersVisitor)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pool {
    #[serde(default = "Pool::default_pool_mode")]
    pub pool_mode: PoolMode,

    /// Maximum time to allow for establishing a new server connection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<u64>,

    /// Close idle connections that have been opened for longer than this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<u64>,

    /// Close server connections that have been opened for longer than this.
    /// Only applied to idle connections. If the connection is actively used for
    /// longer than this period, the pool will not interrupt it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_lifetime: Option<u64>,

    #[serde(default = "Pool::default_cleanup_server_connections")]
    pub cleanup_server_connections: bool,

    #[serde(default)] // False
    pub log_client_parameter_status_changes: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_name: Option<String>,

    #[serde(default = "Pool::default_server_host")]
    pub server_host: String,

    #[serde(default = "Pool::default_server_port")]
    pub server_port: u16,

    // The real name of the database on the server. If it is not specified, the pool name is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_database: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepared_statements_cache_size: Option<usize>,

    /// Override global scaling_warm_pool_ratio for this pool (0-100, percentage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scaling_warm_pool_ratio: Option<u32>,

    /// Override global scaling_fast_retries for this pool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scaling_fast_retries: Option<u32>,

    /// Maximum total server connections to this database across all users.
    /// 0 or None = disabled (default), each user pool works independently.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_db_connections: Option<u32>,

    /// Don't evict connections younger than this (milliseconds). Default: 5000.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_connection_lifetime: Option<u64>,

    /// Extra connections beyond max_db_connections, used as last resort. Default: 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve_pool_size: Option<u32>,

    /// Wait time (milliseconds) before using reserve pool. Default: 3000.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve_pool_timeout: Option<u64>,

    /// Minimum connections per user protected from coordinator eviction.
    /// Overrides user-level min_pool_size for eviction decisions only
    /// (does not trigger prewarm/replenish). Default: 0 (no protection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_guaranteed_pool_size: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_query: Option<AuthQueryConfig>,

    #[serde(
        default = "Pool::default_users",
        deserialize_with = "deserialize_users"
    )]
    pub users: Vec<User>,
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

    pub fn default_users() -> Vec<User> {
        Vec::new()
    }
    pub fn default_server_host() -> String {
        String::from("127.0.0.1")
    }

    pub fn default_cleanup_server_connections() -> bool {
        true
    }

    /// Resolve scaling config by merging pool-level overrides with general defaults.
    /// Anticipation/burst params are global-only by design (no per-pool override).
    pub fn resolve_scaling_config(
        &self,
        general: &crate::config::General,
    ) -> crate::pool::ScalingConfig {
        let ratio = self
            .scaling_warm_pool_ratio
            .unwrap_or(general.scaling_warm_pool_ratio);
        let retries = self
            .scaling_fast_retries
            .unwrap_or(general.scaling_fast_retries);
        crate::pool::ScalingConfig {
            warm_pool_ratio: ratio as f32 / 100.0,
            fast_retries: retries,
            max_anticipation_wait_ms: general.scaling_max_anticipation_wait_ms,
            max_parallel_creates: general.scaling_max_parallel_creates,
        }
    }

    pub async fn validate(&mut self) -> Result<(), Error> {
        // Validate scaling_warm_pool_ratio
        if let Some(ratio) = self.scaling_warm_pool_ratio {
            if ratio > 100 {
                return Err(Error::BadConfig(
                    "scaling_warm_pool_ratio must be 0-100".into(),
                ));
            }
        }

        // Validate pool coordinator settings
        if let Some(max) = self.max_db_connections {
            if max > 0 {
                let total_min: u32 = self.users.iter().filter_map(|u| u.min_pool_size).sum();
                if total_min > max {
                    return Err(Error::BadConfig(format!(
                        "sum of min_pool_size ({}) exceeds max_db_connections ({}); \
                         not all minimums can be satisfied simultaneously",
                        total_min, max
                    )));
                }
                if let Some(reserve) = self.reserve_pool_size {
                    if reserve > max {
                        log::warn!(
                            "reserve_pool_size ({}) exceeds max_db_connections ({}); \
                             PostgreSQL may receive up to {} connections",
                            reserve,
                            max,
                            max + reserve
                        );
                    }
                }

                for user in &self.users {
                    if user.pool_size > max {
                        log::warn!(
                            "user '{}' pool_size ({}) exceeds max_db_connections ({}); \
                             effectively capped at {}",
                            user.username,
                            user.pool_size,
                            max,
                            max
                        );
                    }
                }

                // min_connection_lifetime > idle_timeout: eviction will never trigger
                // because idle connections are closed by idle_timeout first.
                if let Some(min_lt) = self.min_connection_lifetime {
                    if let Some(idle) = self.idle_timeout {
                        if min_lt > idle && idle > 0 {
                            log::warn!(
                                "min_connection_lifetime ({}ms) > idle_timeout ({}ms); \
                                 idle connections will be closed before becoming evictable",
                                min_lt,
                                idle
                            );
                        }
                    }
                }

                // min_guaranteed_pool_size > any user's pool_size: user becomes
                // immune to eviction but cannot reach the guaranteed minimum.
                if let Some(guaranteed) = self.min_guaranteed_pool_size {
                    if guaranteed > 0 {
                        for user in &self.users {
                            if guaranteed > user.pool_size {
                                warn!(
                                    "min_guaranteed_pool_size ({}) > pool_size ({}) for user '{}'; \
                                     user is immune to eviction but cannot reach the guarantee",
                                    guaranteed,
                                    user.pool_size,
                                    user.username
                                );
                            }
                        }
                    }
                }
            }
        }

        // Validate username uniqueness
        let mut seen_usernames = HashSet::new();
        for user in &self.users {
            if !seen_usernames.insert(&user.username) {
                return Err(Error::BadConfig(format!(
                    "duplicate username '{}' in pool users",
                    user.username
                )));
            }
            user.validate().await?;
        }

        // Validate auth_query config
        if let Some(ref aq) = self.auth_query {
            if aq.query.is_empty() {
                return Err(Error::BadConfig("auth_query.query cannot be empty".into()));
            }
            if aq.user.is_empty() {
                return Err(Error::BadConfig("auth_query.user cannot be empty".into()));
            }
            // server_password without server_user makes no sense;
            // server_user without server_password is valid (PostgreSQL trust auth)
            if aq.server_password.is_some() && aq.server_user.is_none() {
                return Err(Error::BadConfig(
                    "auth_query: server_password requires server_user to be set".into(),
                ));
            }
            if aq.workers == 0 {
                return Err(Error::BadConfig("auth_query.workers must be > 0".into()));
            }
            if aq.min_pool_size > aq.pool_size {
                return Err(Error::BadConfig(
                    "auth_query: min_pool_size must be <= pool_size".into(),
                ));
            }
        }

        Ok(())
    }
}

impl Default for Pool {
    fn default() -> Pool {
        Pool {
            pool_mode: Self::default_pool_mode(),
            users: Vec::new(),
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
            scaling_warm_pool_ratio: None,
            scaling_fast_retries: None,
            max_db_connections: None,
            min_connection_lifetime: None,
            reserve_pool_size: None,
            reserve_pool_timeout: None,
            min_guaranteed_pool_size: None,
            auth_query: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthQueryConfig {
    /// SQL query to fetch credentials. Must return (username, password_hash).
    /// Use $1 for the username parameter.
    pub query: String,

    /// PostgreSQL user for executor connections (runs auth queries).
    pub user: String,

    /// Password for executor user (plaintext). Can be empty for trust mode.
    #[serde(default)]
    pub password: String,

    /// Database for executor connections (default: pool name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    /// Number of executor connections (default: 2).
    #[serde(default = "AuthQueryConfig::default_workers")]
    pub workers: u32,

    /// Backend user for data connections. If set, all dynamic users share
    /// one pool with this identity (dedicated mode). If not set, each dynamic
    /// user gets their own pool (passthrough mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_user: Option<String>,

    /// Backend password for dedicated server_user (plaintext).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_password: Option<String>,

    /// Pool size for dynamic user data connections (default: 40).
    #[serde(default = "AuthQueryConfig::default_data_pool_size")]
    pub pool_size: u32,

    /// Minimum connections to maintain per dynamic user pool (default: 0 = no prewarm).
    /// Only applies in passthrough mode (when server_user is not set).
    #[serde(default)]
    pub min_pool_size: u32,

    /// Max cache age for positive entries (default: "1h").
    #[serde(default = "AuthQueryConfig::default_cache_ttl")]
    pub cache_ttl: Duration,

    /// Cache TTL for "user not found" entries (default: "30s").
    #[serde(default = "AuthQueryConfig::default_cache_failure_ttl")]
    pub cache_failure_ttl: Duration,

    /// Min interval between re-fetches for same username on auth failure (default: "1s").
    #[serde(default = "AuthQueryConfig::default_min_interval")]
    pub min_interval: Duration,
}

impl AuthQueryConfig {
    fn default_workers() -> u32 {
        2
    }
    fn default_data_pool_size() -> u32 {
        40
    }
    fn default_cache_ttl() -> Duration {
        Duration::from_hours(1)
    }
    fn default_cache_failure_ttl() -> Duration {
        Duration::from_secs(30)
    }
    fn default_min_interval() -> Duration {
        Duration::from_secs(1)
    }

    /// Returns true if dedicated server_user mode is configured.
    pub fn is_dedicated_mode(&self) -> bool {
        self.server_user.is_some()
    }
}
