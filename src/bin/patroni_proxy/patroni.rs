use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// HTTP request timeout for Patroni API
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Duration in seconds for which a host stays in the blacklist (down upstreams)
const BLACKLIST_DURATION_SECS: u64 = 10;

/// Patroni cluster member role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Cluster leader (primary)
    Leader,
    /// Synchronous replica
    Sync,
    /// Asynchronous replica
    Async,
}

impl Role {
    /// Convert Patroni role string to Role enum
    pub fn from_patroni_role(role: &str) -> Option<Role> {
        match role {
            "leader" => Some(Role::Leader),
            "sync_standby" => Some(Role::Sync),
            "replica" => Some(Role::Async),
            _ => None,
        }
    }

    /// Convert Role enum to Patroni role string
    #[allow(dead_code)]
    pub fn to_patroni_role(self) -> &'static str {
        match self {
            Role::Leader => "leader",
            Role::Sync => "sync_standby",
            Role::Async => "replica",
        }
    }
}

/// Cluster member tags (optional fields)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberTags {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clonefrom: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noloadbalance: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replicatefrom: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nosync: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nofailover: Option<bool>,
    /// Additional user-defined tags
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Patroni cluster member
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub role: String,
    pub state: String,
    pub api_url: String,
    pub host: String,
    pub port: u16,
    /// Timeline value in Patroni may have inconsistent typing - stored as generic JSON
    #[serde(default)]
    pub timeline: Value,
    /// Lag field is present on replicas and may be absent - stored as generic JSON
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag: Option<Value>,
    /// Optional tags object; may be absent; fields inside are also optional
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<MemberTags>,
}

impl Member {
    /// Get member role as enum
    pub fn get_role(&self) -> Option<Role> {
        Role::from_patroni_role(&self.role)
    }
}

/// Patroni cluster
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cluster {
    pub members: Vec<Member>,
    pub scope: String,
}

/// Patroni API errors
#[derive(Debug)]
pub enum PatroniError {
    /// HTTP request error
    HttpError(reqwest::Error),
    /// JSON parsing error
    ParseError(serde_json::Error),
    /// All hosts are unavailable
    AllHostsUnavailable,
    /// Request timeout
    Timeout,
}

impl std::fmt::Display for PatroniError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatroniError::HttpError(e) => write!(f, "HTTP error: {e}"),
            PatroniError::ParseError(e) => write!(f, "Parse error: {e}"),
            PatroniError::AllHostsUnavailable => write!(f, "All hosts unavailable"),
            PatroniError::Timeout => write!(f, "Request timeout"),
        }
    }
}

impl std::error::Error for PatroniError {}

impl From<reqwest::Error> for PatroniError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            PatroniError::Timeout
        } else {
            PatroniError::HttpError(e)
        }
    }
}

impl From<serde_json::Error> for PatroniError {
    fn from(e: serde_json::Error) -> Self {
        PatroniError::ParseError(e)
    }
}

/// Blacklist of unavailable hosts
struct HostBlacklist {
    /// Map of host -> time when added to blacklist
    entries: RwLock<HashMap<String, Instant>>,
}

impl HostBlacklist {
    fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Check if host is in the blacklist
    fn is_blacklisted(&self, host: &str) -> bool {
        let entries = self.entries.read().unwrap();
        if let Some(added_at) = entries.get(host) {
            added_at.elapsed().as_secs() < BLACKLIST_DURATION_SECS
        } else {
            false
        }
    }

    /// Add host to the blacklist
    fn add(&self, host: &str) {
        let mut entries = self.entries.write().unwrap();
        entries.insert(host.to_string(), Instant::now());
    }

    /// Remove host from the blacklist (on successful connection)
    fn remove(&self, host: &str) {
        let mut entries = self.entries.write().unwrap();
        entries.remove(host);
    }

    /// Clean up expired entries
    fn cleanup(&self) {
        let mut entries = self.entries.write().unwrap();
        entries.retain(|_, added_at| added_at.elapsed().as_secs() < BLACKLIST_DURATION_SECS);
    }
}

/// Client for Patroni API
pub struct PatroniClient {
    /// HTTP client
    client: reqwest::Client,
    /// Blacklist of unavailable hosts
    blacklist: HostBlacklist,
}

impl PatroniClient {
    /// Create a new Patroni API client
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;

        Ok(Self {
            client,
            blacklist: HostBlacklist::new(),
        })
    }

    /// Fetch cluster information from the specified URL
    ///
    /// # Arguments
    /// * `url` - base Patroni API URL (e.g., http://192.168.0.1:8008)
    pub async fn fetch_cluster(&self, url: &str) -> Result<Cluster, PatroniError> {
        let cluster_url = format!("{}/cluster", url.trim_end_matches('/'));
        let response = self.client.get(&cluster_url).send().await?;
        let cluster: Cluster = response.json().await?;
        Ok(cluster)
    }

    /// Fetch cluster members by iterating through hosts
    ///
    /// Optimizations:
    /// - Hosts that don't respond are added to blacklist for 10 seconds
    /// - If all hosts are blacklisted, try all hosts from the blacklist
    ///
    /// # Arguments
    /// * `hosts` - list of base Patroni API URLs
    pub async fn fetch_members(&self, hosts: &[String]) -> Result<Vec<Member>, PatroniError> {
        // Clean up expired blacklist entries
        self.blacklist.cleanup();

        // Split hosts into available and blacklisted
        let mut available: Vec<&String> = Vec::new();
        let mut blacklisted: Vec<&String> = Vec::new();

        for host in hosts {
            if self.blacklist.is_blacklisted(host) {
                blacklisted.push(host);
            } else {
                available.push(host);
            }
        }

        // Determine iteration order: available first, then blacklisted
        // If all are blacklisted - try all
        let all_in_blacklist = available.is_empty();

        // First try available hosts (or all if all are blacklisted)
        let first_batch = if all_in_blacklist {
            &blacklisted
        } else {
            &available
        };

        for host in first_batch {
            match self.fetch_cluster(host).await {
                Ok(cluster) => {
                    // Successfully got response - remove from blacklist
                    self.blacklist.remove(host);
                    return Ok(cluster.members);
                }
                Err(e) => {
                    // Add to blacklist
                    tracing::warn!("Failed to fetch cluster from {}: {}", host, e);
                    self.blacklist.add(host);
                }
            }
        }

        // If not all were blacklisted, try hosts from the blacklist
        if !all_in_blacklist {
            for host in &blacklisted {
                match self.fetch_cluster(host).await {
                    Ok(cluster) => {
                        self.blacklist.remove(host);
                        return Ok(cluster.members);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch cluster from {}: {}", host, e);
                        self.blacklist.add(host);
                    }
                }
            }
        }

        Err(PatroniError::AllHostsUnavailable)
    }
}

impl Default for PatroniClient {
    fn default() -> Self {
        Self::new().expect("Failed to create PatroniClient")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_from_patroni_role() {
        assert_eq!(Role::from_patroni_role("leader"), Some(Role::Leader));
        assert_eq!(Role::from_patroni_role("sync_standby"), Some(Role::Sync));
        assert_eq!(Role::from_patroni_role("replica"), Some(Role::Async));
        assert_eq!(Role::from_patroni_role("unknown"), None);
    }

    #[test]
    fn test_role_to_patroni_role() {
        assert_eq!(Role::Leader.to_patroni_role(), "leader");
        assert_eq!(Role::Sync.to_patroni_role(), "sync_standby");
        assert_eq!(Role::Async.to_patroni_role(), "replica");
    }

    #[test]
    fn test_member_deserialization() {
        let json = r#"{
            "name": "node1",
            "role": "leader",
            "state": "running",
            "api_url": "http://192.168.0.1:8008/patroni",
            "host": "192.168.0.1",
            "port": 5432,
            "timeline": 1
        }"#;

        let member: Member = serde_json::from_str(json).unwrap();
        assert_eq!(member.name, "node1");
        assert_eq!(member.role, "leader");
        assert_eq!(member.get_role(), Some(Role::Leader));
        assert_eq!(member.host, "192.168.0.1");
        assert_eq!(member.port, 5432);
    }

    #[test]
    fn test_member_with_tags() {
        let json = r#"{
            "name": "node2",
            "role": "replica",
            "state": "running",
            "api_url": "http://192.168.0.2:8008/patroni",
            "host": "192.168.0.2",
            "port": 5432,
            "timeline": 1,
            "lag": 0,
            "tags": {
                "noloadbalance": false,
                "clonefrom": true
            }
        }"#;

        let member: Member = serde_json::from_str(json).unwrap();
        assert_eq!(member.name, "node2");
        assert_eq!(member.get_role(), Some(Role::Async));
        assert!(member.tags.is_some());
        let tags = member.tags.unwrap();
        assert_eq!(tags.noloadbalance, Some(false));
        assert_eq!(tags.clonefrom, Some(true));
    }

    #[test]
    fn test_cluster_deserialization() {
        let json = r#"{
            "scope": "my_cluster",
            "members": [
                {
                    "name": "node1",
                    "role": "leader",
                    "state": "running",
                    "api_url": "http://192.168.0.1:8008/patroni",
                    "host": "192.168.0.1",
                    "port": 5432,
                    "timeline": 1
                },
                {
                    "name": "node2",
                    "role": "sync_standby",
                    "state": "running",
                    "api_url": "http://192.168.0.2:8008/patroni",
                    "host": "192.168.0.2",
                    "port": 5432,
                    "timeline": 1,
                    "lag": 0
                }
            ]
        }"#;

        let cluster: Cluster = serde_json::from_str(json).unwrap();
        assert_eq!(cluster.scope, "my_cluster");
        assert_eq!(cluster.members.len(), 2);
        assert_eq!(cluster.members[0].get_role(), Some(Role::Leader));
        assert_eq!(cluster.members[1].get_role(), Some(Role::Sync));
    }

    #[test]
    fn test_blacklist() {
        let blacklist = HostBlacklist::new();

        assert!(!blacklist.is_blacklisted("host1"));

        blacklist.add("host1");
        assert!(blacklist.is_blacklisted("host1"));

        blacklist.remove("host1");
        assert!(!blacklist.is_blacklisted("host1"));
    }
}
