use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Leader,
    Sync,
    Async,
    Any,
}

impl Role {
    pub fn is_valid(role_str: &str) -> bool {
        matches!(
            role_str.to_lowercase().as_str(),
            "leader" | "sync" | "async" | "any"
        )
    }

    pub fn from_str(s: &str) -> Result<Self, ConfigError> {
        match s.to_lowercase().as_str() {
            "leader" => Ok(Role::Leader),
            "sync" => Ok(Role::Sync),
            "async" => Ok(Role::Async),
            "any" => Ok(Role::Any),
            _ => Err(ConfigError::InvalidRole(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TlsConfig {
    pub ca_cert: Option<String>,
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
    pub skip_verify: Option<bool>,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            ca_cert: None,
            client_cert: None,
            client_key: None,
            skip_verify: Some(false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortConfig {
    pub listen: String,
    pub roles: Vec<String>,
    pub host_port: u16,
    #[serde(default)]
    pub max_lag_in_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub hosts: Vec<String>,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    pub ports: HashMap<String, PortConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Cluster update interval in seconds (default: 3)
    #[serde(default = "default_cluster_update_interval")]
    pub cluster_update_interval: u64,
    /// HTTP listen address for health checks and metrics (default: "127.0.0.1:8009")
    #[serde(default = "default_listen_address")]
    pub listen_address: String,
    pub clusters: HashMap<String, ClusterConfig>,
}

fn default_cluster_update_interval() -> u64 {
    3
}

fn default_listen_address() -> String {
    "127.0.0.1:8009".to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    IoError(String),
    ParseError(String),
    InvalidRole(String),
    InvalidHost(String),
    DuplicateHost(String),
    DuplicateListen(String),
    EmptyHosts(String),
    EmptyRoles(String),
    EmptyPorts(String),
    InvalidListenAddress(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::IoError(e) => write!(f, "IO error: {}", e),
            ConfigError::ParseError(e) => write!(f, "Parse error: {}", e),
            ConfigError::InvalidRole(r) => write!(
                f,
                "Invalid role '{}'. Allowed roles: leader, sync, async, any",
                r
            ),
            ConfigError::InvalidHost(h) => write!(
                f,
                "Invalid host '{}'. Only http:// and https:// schemes are allowed",
                h
            ),
            ConfigError::DuplicateHost(h) => write!(f, "Duplicate host: {}", h),
            ConfigError::DuplicateListen(l) => write!(f, "Duplicate listen address: {}", l),
            ConfigError::EmptyHosts(c) => write!(f, "Cluster '{}' has no hosts defined", c),
            ConfigError::EmptyRoles(p) => write!(f, "Port '{}' has no roles defined", p),
            ConfigError::EmptyPorts(c) => write!(f, "Cluster '{}' has no ports defined", c),
            ConfigError::InvalidListenAddress(a) => write!(f, "Invalid listen address: {}", a),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path).map_err(|e| ConfigError::IoError(e.to_string()))?;
        Self::from_str(&content)
    }

    pub fn from_str(content: &str) -> Result<Self, ConfigError> {
        let config: Config =
            serde_yaml::from_str(content).map_err(|e| ConfigError::ParseError(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut all_listen_addresses: HashSet<String> = HashSet::new();

        for (cluster_name, cluster) in &self.clusters {
            // Validate hosts are not empty
            if cluster.hosts.is_empty() {
                return Err(ConfigError::EmptyHosts(cluster_name.clone()));
            }

            // Validate hosts: only http/https, no duplicates
            let mut seen_hosts: HashSet<String> = HashSet::new();
            for host in &cluster.hosts {
                // Check scheme
                if !host.starts_with("http://") && !host.starts_with("https://") {
                    return Err(ConfigError::InvalidHost(host.clone()));
                }

                // Check for duplicates within cluster
                let normalized = host.to_lowercase();
                if seen_hosts.contains(&normalized) {
                    return Err(ConfigError::DuplicateHost(host.clone()));
                }
                seen_hosts.insert(normalized);
            }

            // Validate ports are not empty
            if cluster.ports.is_empty() {
                return Err(ConfigError::EmptyPorts(cluster_name.clone()));
            }

            // Validate ports
            for (port_name, port_config) in &cluster.ports {
                // Validate roles are not empty
                if port_config.roles.is_empty() {
                    return Err(ConfigError::EmptyRoles(port_name.clone()));
                }

                // Validate each role
                for role in &port_config.roles {
                    if !Role::is_valid(role) {
                        return Err(ConfigError::InvalidRole(role.clone()));
                    }
                }

                // Validate listen address format
                let listen = &port_config.listen;
                if listen.parse::<SocketAddr>().is_err() {
                    return Err(ConfigError::InvalidListenAddress(listen.clone()));
                }

                // Check for duplicate listen addresses across all clusters
                if all_listen_addresses.contains(listen) {
                    return Err(ConfigError::DuplicateListen(listen.clone()));
                }
                all_listen_addresses.insert(listen.clone());
            }
        }

        Ok(())
    }
}

// Diff types for detecting configuration changes
#[derive(Debug, Clone, PartialEq)]
pub enum ClusterDiff {
    Added(String, ClusterConfig),
    Removed(String),
    HostsChanged(String, Vec<String>, Vec<String>), // cluster_name, old_hosts, new_hosts
    PortsChanged(
        String,
        HashMap<String, PortConfig>,
        HashMap<String, PortConfig>,
    ),
    TlsChanged(String),
}

#[derive(Debug, Clone)]
pub struct ConfigDiff {
    pub changes: Vec<ClusterDiff>,
}

impl ConfigDiff {
    pub fn compute(old: &Config, new: &Config) -> Self {
        let mut changes = Vec::new();

        // Find removed clusters
        for cluster_name in old.clusters.keys() {
            if !new.clusters.contains_key(cluster_name) {
                changes.push(ClusterDiff::Removed(cluster_name.clone()));
            }
        }

        // Find added or modified clusters
        for (cluster_name, new_cluster) in &new.clusters {
            match old.clusters.get(cluster_name) {
                None => {
                    changes.push(ClusterDiff::Added(
                        cluster_name.clone(),
                        new_cluster.clone(),
                    ));
                }
                Some(old_cluster) => {
                    // Check hosts changes
                    if old_cluster.hosts != new_cluster.hosts {
                        changes.push(ClusterDiff::HostsChanged(
                            cluster_name.clone(),
                            old_cluster.hosts.clone(),
                            new_cluster.hosts.clone(),
                        ));
                    }

                    // Check ports changes
                    if !ports_equal(&old_cluster.ports, &new_cluster.ports) {
                        changes.push(ClusterDiff::PortsChanged(
                            cluster_name.clone(),
                            old_cluster.ports.clone(),
                            new_cluster.ports.clone(),
                        ));
                    }

                    // Check TLS changes
                    if !tls_equal(&old_cluster.tls, &new_cluster.tls) {
                        changes.push(ClusterDiff::TlsChanged(cluster_name.clone()));
                    }
                }
            }
        }

        ConfigDiff { changes }
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }
}

fn ports_equal(a: &HashMap<String, PortConfig>, b: &HashMap<String, PortConfig>) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (key, val_a) in a {
        match b.get(key) {
            None => return false,
            Some(val_b) => {
                if val_a.listen != val_b.listen
                    || val_a.roles != val_b.roles
                    || val_a.host_port != val_b.host_port
                    || val_a.max_lag_in_bytes != val_b.max_lag_in_bytes
                {
                    return false;
                }
            }
        }
    }
    true
}

fn tls_equal(a: &Option<TlsConfig>, b: &Option<TlsConfig>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
        (Some(tls_a), Some(tls_b)) => {
            tls_a.ca_cert == tls_b.ca_cert
                && tls_a.client_cert == tls_b.client_cert
                && tls_a.client_key == tls_b.client_key
                && tls_a.skip_verify == tls_b.skip_verify
        }
    }
}

// Repository for managing configuration with hot-reload support
pub struct ConfigRepository {
    config: ArcSwap<Config>,
    config_path: String,
}

impl ConfigRepository {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let config = Config::from_file(&path)?;
        Ok(Self {
            config: ArcSwap::from_pointee(config),
            config_path: path_str,
        })
    }

    pub fn get(&self) -> Arc<Config> {
        self.config.load_full()
    }

    pub fn reload(&self) -> Result<ConfigDiff, ConfigError> {
        let new_config = Config::from_file(&self.config_path)?;
        let old_config = self.config.load();
        let diff = ConfigDiff::compute(&old_config, &new_config);

        if diff.has_changes() {
            self.config.store(Arc::new(new_config));
        }

        Ok(diff)
    }

    #[allow(dead_code)]
    pub fn config_path(&self) -> &str {
        &self.config_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_roles() {
        assert!(Role::is_valid("leader"));
        assert!(Role::is_valid("Leader"));
        assert!(Role::is_valid("LEADER"));
        assert!(Role::is_valid("sync"));
        assert!(Role::is_valid("async"));
        assert!(Role::is_valid("any"));
        assert!(!Role::is_valid("master")); // master is not valid, use leader
        assert!(!Role::is_valid("replica"));
        assert!(!Role::is_valid("invalid"));
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!(Role::from_str("leader").unwrap(), Role::Leader);
        assert_eq!(Role::from_str("SYNC").unwrap(), Role::Sync);
        assert_eq!(Role::from_str("Async").unwrap(), Role::Async);
        assert_eq!(Role::from_str("any").unwrap(), Role::Any);
        assert!(Role::from_str("invalid").is_err());
    }

    #[test]
    fn test_valid_config() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
      - "https://192.168.0.2:8008"
    tls:
      ca_cert: "/path/to/ca.crt"
      client_cert: "/path/to/client.crt"
      client_key: "/path/to/client.key"
      skip_verify: false
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
      any:
        listen: "127.0.0.1:6432"
        roles: ["any"]
        host_port: 6432
        max_lag_in_bytes: 16777216
"#;
        let config = Config::from_str(yaml);
        assert!(config.is_ok(), "Config should be valid: {:?}", config.err());
    }

    #[test]
    fn test_invalid_role() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["invalid_role"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::InvalidRole(_))));
    }

    #[test]
    fn test_invalid_host_scheme() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "ftp://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::InvalidHost(_))));
    }

    #[test]
    fn test_duplicate_hosts() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::DuplicateHost(_))));
    }

    #[test]
    fn test_duplicate_listen_same_cluster() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
      replica:
        listen: "127.0.0.1:5432"
        roles: ["sync"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::DuplicateListen(_))));
    }

    #[test]
    fn test_duplicate_listen_different_clusters() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
  two:
    hosts:
      - "http://192.168.0.2:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::DuplicateListen(_))));
    }

    #[test]
    fn test_empty_hosts() {
        let yaml = r#"
clusters:
  one:
    hosts: []
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::EmptyHosts(_))));
    }

    #[test]
    fn test_empty_roles() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: []
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::EmptyRoles(_))));
    }

    #[test]
    fn test_invalid_listen_address() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "invalid_address"
        roles: ["leader"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(matches!(config, Err(ConfigError::InvalidListenAddress(_))));
    }

    #[test]
    fn test_config_diff_no_changes() {
        let yaml = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let config1 = Config::from_str(yaml).unwrap();
        let config2 = Config::from_str(yaml).unwrap();
        let diff = ConfigDiff::compute(&config1, &config2);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_config_diff_cluster_added() {
        let yaml1 = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let yaml2 = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
  two:
    hosts:
      - "http://192.168.0.2:8008"
    ports:
      master:
        listen: "127.0.0.1:5433"
        roles: ["leader"]
        host_port: 6432
"#;
        let config1 = Config::from_str(yaml1).unwrap();
        let config2 = Config::from_str(yaml2).unwrap();
        let diff = ConfigDiff::compute(&config1, &config2);
        assert!(diff.has_changes());
        assert!(diff
            .changes
            .iter()
            .any(|c| matches!(c, ClusterDiff::Added(name, _) if name == "two")));
    }

    #[test]
    fn test_config_diff_cluster_removed() {
        let yaml1 = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
  two:
    hosts:
      - "http://192.168.0.2:8008"
    ports:
      master:
        listen: "127.0.0.1:5433"
        roles: ["leader"]
        host_port: 6432
"#;
        let yaml2 = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let config1 = Config::from_str(yaml1).unwrap();
        let config2 = Config::from_str(yaml2).unwrap();
        let diff = ConfigDiff::compute(&config1, &config2);
        assert!(diff.has_changes());
        assert!(diff
            .changes
            .iter()
            .any(|c| matches!(c, ClusterDiff::Removed(name) if name == "two")));
    }

    #[test]
    fn test_config_diff_hosts_changed() {
        let yaml1 = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let yaml2 = r#"
clusters:
  one:
    hosts:
      - "http://192.168.0.1:8008"
      - "http://192.168.0.3:8008"
    ports:
      master:
        listen: "127.0.0.1:5432"
        roles: ["leader"]
        host_port: 6432
"#;
        let config1 = Config::from_str(yaml1).unwrap();
        let config2 = Config::from_str(yaml2).unwrap();
        let diff = ConfigDiff::compute(&config1, &config2);
        assert!(diff.has_changes());
        assert!(diff
            .changes
            .iter()
            .any(|c| matches!(c, ClusterDiff::HostsChanged(name, _, _) if name == "one")));
    }

    #[test]
    fn test_multiple_clusters() {
        let yaml = r#"
clusters:
  production:
    hosts:
      - "https://prod1.example.com:8008"
      - "https://prod2.example.com:8008"
      - "https://prod3.example.com:8008"
    tls:
      ca_cert: "/etc/ssl/ca.crt"
      skip_verify: false
    ports:
      primary:
        listen: "0.0.0.0:5432"
        roles: ["leader"]
        host_port: 6432
      replicas:
        listen: "0.0.0.0:5433"
        roles: ["sync", "async"]
        host_port: 6432
        max_lag_in_bytes: 16777216
  staging:
    hosts:
      - "http://staging1.example.com:8008"
    ports:
      all:
        listen: "0.0.0.0:5434"
        roles: ["any"]
        host_port: 6432
"#;
        let config = Config::from_str(yaml);
        assert!(config.is_ok(), "Config should be valid: {:?}", config.err());
        let config = config.unwrap();
        assert_eq!(config.clusters.len(), 2);
        assert!(config.clusters.contains_key("production"));
        assert!(config.clusters.contains_key("staging"));
    }
}
