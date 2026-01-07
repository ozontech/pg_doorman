//! Connection pool configuration.

use serde_derive::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use crate::errors::Error;

use super::{PoolMode, User};

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

    #[serde(default = "Pool::default_users")]
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

    pub fn default_users() -> BTreeMap<String, User> {
        BTreeMap::default()
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
