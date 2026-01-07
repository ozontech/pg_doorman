//! Include files and server configuration.

use serde_derive::{Deserialize, Serialize};

use super::General;

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

impl Include {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GeneralWithInclude {
    #[serde(default = "General::default_include")]
    pub include: Include,
}
