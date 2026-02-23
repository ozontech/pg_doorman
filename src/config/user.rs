//! PostgreSQL user configuration.

use serde_derive::{Deserialize, Serialize};

use crate::auth::jwt::load_jwt_pub_key;
use crate::errors::Error;
use crate::messages::JWT_PUB_KEY_PASSWORD_PREFIX;

use super::PoolMode;

/// PostgreSQL user.
#[derive(Clone, PartialEq, Hash, Eq, Serialize, Deserialize, Debug)]
pub struct User {
    pub username: String,
    pub password: String,
    pub pool_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_pool_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_mode: Option<PoolMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_lifetime: Option<u64>,
    // Override backend credentials. When omitted, passthrough auth is used:
    // pg_doorman reuses the client's MD5 hash or SCRAM ClientKey to authenticate.
    // Only needed when the backend PostgreSQL user differs from the pool username.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_password: Option<String>,
    // Pam auth
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_pam_service: Option<String>,
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
            auth_pam_service: None,
        }
    }
}

impl User {
    pub async fn validate(&self) -> Result<(), Error> {
        if self.password.starts_with(JWT_PUB_KEY_PASSWORD_PREFIX) {
            let jwt_pub_key_file = self
                .password
                .strip_prefix(JWT_PUB_KEY_PASSWORD_PREFIX)
                .unwrap()
                .to_string();
            load_jwt_pub_key(jwt_pub_key_file).await?;
        }
        if self.server_password.is_some() && self.server_username.is_none() {
            return Err(Error::BadConfig(
                "server_password requires server_username to be set".to_string(),
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
