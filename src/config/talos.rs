//! Talos authentication configuration.

use serde_derive::{Deserialize, Serialize};

use crate::auth::talos::load_talos_pub_key;
use crate::errors::Error;

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug, Hash, Eq, Default)]
pub struct Talos {
    pub keys: Vec<String>,
    pub databases: Vec<String>,
}

impl Talos {
    pub async fn validate(&mut self) -> Result<(), Error> {
        for key_file in self.keys.iter() {
            load_talos_pub_key(key_file.to_string()).await?
        }
        Ok(())
    }
    pub fn empty() -> Self {
        Talos {
            keys: vec![],
            databases: vec![],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.databases.is_empty()
    }
}
