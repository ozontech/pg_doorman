//! Connection pool configuration.

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::errors::Error;

use super::{PoolMode, User};

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

    #[serde(default = "Pool::default_users", deserialize_with = "deserialize_users")]
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

    pub async fn validate(&mut self) -> Result<(), Error> {
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
        }
    }
}
