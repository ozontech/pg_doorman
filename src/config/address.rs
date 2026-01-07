//! Address and PoolMode definitions for PostgreSQL connection pooling.

use serde_derive::{Deserialize, Serialize};
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::stats::AddressStats;

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
        write!(f, "{str}")
    }
}

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
}
