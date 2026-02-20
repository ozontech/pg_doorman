//! Configuration module for the PostgreSQL connection pooler.
//!
//! This module provides configuration parsing, validation, and management
//! for the connection pooler.

use arc_swap::ArcSwap;
use log::{error, info};
use once_cell::sync::Lazy;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use self::tls::{load_identity, TLSMode};
use crate::auth::hba::CheckResult;
use crate::errors::Error;
use crate::pool::{ClientServerMap, ConnectionPool};

// Sub-modules
mod address;
mod byte_size;
mod duration;
mod general;
mod include;
mod pool;
mod prometheus;
mod talos;
pub mod tls;
mod user;

#[cfg(test)]
mod tests;

// Re-exports
pub use address::{Address, PoolMode};
pub use byte_size::ByteSize;
pub use duration::Duration;
pub use general::General;
pub use include::{GeneralWithInclude, Include, ServerConfig};
pub use pool::Pool;
pub use prometheus::Prometheus;
pub use talos::Talos;
pub use user::User;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Configuration file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Yaml,
}

impl ConfigFormat {
    /// Detect configuration format from file path extension.
    /// Returns Yaml for .yaml/.yml files, Toml for everything else.
    pub fn detect(path: &str) -> Self {
        let path_lower = path.to_lowercase();
        if path_lower.ends_with(".yaml") || path_lower.ends_with(".yml") {
            ConfigFormat::Yaml
        } else {
            ConfigFormat::Toml
        }
    }
}

/// Parse configuration content based on format.
fn parse_config_content<T: serde::de::DeserializeOwned>(
    contents: &str,
    format: ConfigFormat,
) -> Result<T, Error> {
    match format {
        ConfigFormat::Toml => toml::from_str(contents)
            .map_err(|err| Error::BadConfig(format!("TOML parse error: {err}"))),
        ConfigFormat::Yaml => serde_yaml::from_str(contents)
            .map_err(|err| Error::BadConfig(format!("YAML parse error: {err}"))),
    }
}

/// Recursively remove null values from a JSON value.
/// TOML does not support null, so we strip them before conversion.
fn remove_json_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            for v in map.values_mut() {
                remove_json_nulls(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                remove_json_nulls(item);
            }
        }
        _ => {}
    }
}

/// Convert configuration content to TOML string for merging.
/// This allows mixing YAML and TOML files in include.files.
fn content_to_toml_string(contents: &str, format: ConfigFormat) -> Result<String, Error> {
    match format {
        ConfigFormat::Toml => Ok(contents.to_string()),
        ConfigFormat::Yaml => {
            // Parse YAML to serde_json::Value as intermediate format
            let mut yaml_value: serde_json::Value = serde_yaml::from_str(contents)
                .map_err(|err| Error::BadConfig(format!("YAML parse error: {err}")))?;
            // Remove null values — TOML does not support them
            remove_json_nulls(&mut yaml_value);
            // Convert JSON value to TOML string
            toml::to_string_pretty(&yaml_value)
                .map_err(|err| Error::BadConfig(format!("YAML to TOML conversion error: {err}")))
        }
    }
}

/// Globally available configuration.
static CONFIG: Lazy<ArcSwap<Config>> = Lazy::new(|| ArcSwap::from_pointee(Config::default()));

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
    #[serde(
        default = "Config::default_path",
        skip_serializing_if = "String::is_empty"
    )]
    pub path: String,

    // General and global settings.
    pub general: General,

    // Prometheus settings.
    #[serde(default = "Prometheus::empty")]
    pub prometheus: Prometheus,

    // Talos settings.
    #[serde(default = "Talos::empty", skip_serializing_if = "Talos::is_empty")]
    pub talos: Talos,

    // Connection pools.
    pub pools: HashMap<String, Pool>,

    // Include files.
    #[serde(
        default = "General::default_include",
        skip_serializing_if = "Include::is_empty"
    )]
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
            prometheus: Prometheus::empty(),
            pools: HashMap::default(),
            talos: Talos {
                keys: vec![],
                databases: vec![],
            },
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
                        format!("pools.{pool_name}.pool_mode"),
                        pool.pool_mode.to_string(),
                    ),
                    (
                        format!("pools.{pool_name:?}.users"),
                        pool.users
                            .iter()
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
        if self.general.hba.is_empty() {
            if let Some(pg_hba) = &self.general.pg_hba {
                info!("HBA config:\n{pg_hba}\n");
            } else {
                info!("HBA config: empty");
            }
        } else {
            info!("HBA config: {:?} (legacy mode via hba)", self.general.hba);
        }
        match self.general.tls_certificate.clone() {
            Some(tls_certificate) => {
                info!("TLS certificate: {tls_certificate}");

                if let Some(tls_private_key) = self.general.tls_private_key.clone() {
                    info!("TLS private key: {tls_private_key}");
                }
            }
            None => {
                info!("TLS support is disabled");
            }
        };

        for (pool_name, pool) in &self.pools {
            info!("[pool: {}] Pool mode: {}", pool_name, pool.pool_mode);
            info!(
                "[pool: {}] Server: {}:{}",
                pool_name, pool.server_host, pool.server_port
            );
            info!(
                "[pool: {}] Cleanup server connections: {}",
                pool_name, pool.cleanup_server_connections
            );
            info!(
                "[pool: {}] Connect timeout: {}ms",
                pool_name,
                pool.connect_timeout
                    .unwrap_or(self.general.connect_timeout.as_millis())
            );
            info!(
                "[pool: {}] Idle timeout: {}ms",
                pool_name,
                pool.idle_timeout
                    .unwrap_or(self.general.idle_timeout.as_millis())
            );
            info!(
                "[pool: {}] Server lifetime: {}ms",
                pool_name,
                pool.server_lifetime
                    .unwrap_or(self.general.server_lifetime.as_millis())
            );
            for (user_index, user) in pool.users.iter().enumerate() {
                info!(
                    "[pool: {}] User {}: {}",
                    pool_name, user_index, user.username
                );
                info!(
                    "[pool: {}] User {} pool size: {}",
                    pool_name, user_index, user.pool_size
                );
            }
        }
    }

    /// Validate the configuration.
    pub async fn validate(&mut self) -> Result<(), Error> {
        // Validate Talos
        self.talos.validate().await?;

        if self.general.tls_rate_limit_per_second < 100
            && self.general.tls_rate_limit_per_second != 0
        {
            return Err(Error::BadConfig(
                "tls rate limit should be > 100".to_string(),
            ));
        }
        if !self.general.tls_rate_limit_per_second.is_multiple_of(100) {
            return Err(Error::BadConfig(
                "tls rate limit should be multiple 100".to_string(),
            ));
        }

        // Validate mutual exclusion for HBA settings
        if self.general.pg_hba.is_some() && !self.general.hba.is_empty() {
            return Err(Error::BadConfig(
                "general.hba and general.pg_hba cannot be specified at the same time".to_string(),
            ));
        }

        // Validate prepared_statements
        if self.general.prepared_statements && self.general.prepared_statements_cache_size == 0 {
            return Err(Error::BadConfig("The value of prepared_statements_cache should be greater than 0 if prepared_statements are enabled".to_string()));
        }

        // Validate TLS
        {
            if self.general.tls_certificate.is_none() && self.general.tls_private_key.is_some() {
                return Err(Error::BadConfig(
                    "tls_private_key is set but tls_certificate is not".to_string(),
                ));
            }

            if self.general.tls_certificate.is_some() && self.general.tls_private_key.is_none() {
                return Err(Error::BadConfig(
                    "tls_certificate is set but tls_private_key is not".to_string(),
                ));
            }

            if let Some(tls_mode) = self.general.tls_mode.clone() {
                let mode = tls::TLSMode::from_string(tls_mode.as_str())?;
                if (self.general.tls_certificate.is_none()
                    || self.general.tls_private_key.is_none())
                    && (mode != TLSMode::Disable && mode != TLSMode::Allow)
                {
                    return Err(Error::BadConfig(format!(
                        "tls_mode is {mode} but tls_certificate or tls_private_key is not"
                    )));
                }
                if mode == tls::TLSMode::VerifyFull && self.general.tls_ca_cert.is_none() {
                    return Err(Error::BadConfig(format!(
                        "tls_mode is {mode} but tls_ca_cert is not set"
                    )));
                }
                #[cfg(not(target_os = "linux"))]
                if mode == tls::TLSMode::VerifyFull {
                    return Err(Error::BadConfig(
                        "tls_mode verify-full is supported only on linux".to_string(),
                    ));
                }
            }

            if let Some(tls_certificate) = self.general.tls_certificate.clone() {
                if let Some(tls_private_key) = self.general.tls_private_key.clone() {
                    match load_identity(Path::new(&tls_certificate), Path::new(&tls_private_key)) {
                        Ok(_) => (),
                        Err(err) => {
                            return Err(Error::BadConfig(format!(
                                "tls is incorrectly configured: {err:?}"
                            )));
                        }
                    }
                }
            };
        }

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
            return Err(Error::BadConfig(format!("Could not open '{path}': {err}")));
        }
    };
    match file.read_to_string(&mut contents).await {
        Ok(_) => (),
        Err(err) => {
            return Err(Error::BadConfig(format!(
                "Could not read config file: {err}"
            )));
        }
    };
    Ok(contents)
}

/// Parse the configuration file located at the path.
/// Supports both TOML (.toml) and YAML (.yaml, .yml) formats.
/// Format is auto-detected based on file extension.
pub async fn parse(path: &str) -> Result<(), Error> {
    let format = ConfigFormat::detect(path);

    // parse only include.files = ["./path/to/file",...]
    let include_only_config_contents = load_file(path).await?;
    let include_config: GeneralWithInclude =
        parse_config_content(&include_only_config_contents, format)?;

    // merge main with include files via serde-toml-merge.
    // Convert to TOML string first (for YAML files), then parse to toml::Value
    let main_toml_str = content_to_toml_string(&include_only_config_contents, format)?;
    let mut config_merged: toml::Value = main_toml_str
        .parse()
        .map_err(|err| Error::BadConfig(format!("Could not parse config file {path}: {err:?}")))?;

    for file in include_config.include.files {
        info!("Merge config with include file: {file}");
        let include_file_content = load_file(file.as_str()).await?;
        let include_format = ConfigFormat::detect(&file);
        let include_toml_str = content_to_toml_string(&include_file_content, include_format)?;
        let include_file_value: toml::Value = include_toml_str.parse().map_err(|err| {
            Error::BadConfig(format!("Could not parse include file {file}: {err:?}"))
        })?;
        config_merged = match serde_toml_merge::merge(config_merged, include_file_value) {
            Ok(value) => value,
            Err(err) => {
                return Err(Error::BadConfig(format!(
                    "Could not merge config file {file}: {err:?}"
                )));
            }
        };
    }

    let table = config_merged.as_table().unwrap();
    let mut config: Config = match toml::from_str(&table.to_string()) {
        Ok(config) => config,
        Err(err) => {
            return Err(Error::BadConfig(format!("Could not merge config: {err:?}")));
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
            error!("Config reload error: {err:?}");
            return Err(Error::BadConfig(format!("Config reload error: {err:?}")));
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

pub fn check_hba(
    ip: IpAddr,
    ssl: bool,
    type_auth: &str,
    username: &str,
    database: &str,
) -> CheckResult {
    let config = get_config();
    if let Some(ref pg) = config.general.pg_hba {
        return pg.check_hba(ip, ssl, type_auth, username, database);
    }
    if config.general.hba.is_empty() {
        return CheckResult::Allow;
    }
    if config.general.hba.iter().any(|net| net.contains(&ip)) {
        CheckResult::Allow
    } else {
        CheckResult::NotMatched
    }
}
