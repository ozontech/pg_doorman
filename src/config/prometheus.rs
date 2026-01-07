//! Prometheus metrics configuration.

use serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Prometheus {
    #[serde(default = "Prometheus::default_host")]
    pub host: String,
    #[serde(default = "Prometheus::default_port")]
    pub port: u16,
    #[serde(default = "Prometheus::default_enable")]
    pub enabled: bool,
}

impl Prometheus {
    pub fn empty() -> Prometheus {
        Prometheus {
            host: Self::default_host(),
            port: Self::default_port(),
            enabled: Self::default_enable(),
        }
    }
    pub fn default_host() -> String {
        "0.0.0.0".to_string()
    }
    pub fn default_port() -> u16 {
        9127
    }
    pub fn default_enable() -> bool {
        false
    }
}
