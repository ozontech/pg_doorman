//! PostgreSQL user configuration.

use serde_derive::{Deserialize, Serialize};

use crate::auth::jwt::load_jwt_pub_key;
use crate::messages::JWT_PUB_KEY_PASSWORD_PREFIX;
use crate::errors::Error;

use super::PoolMode;

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
    // Pam auth
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
