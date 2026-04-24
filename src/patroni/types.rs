use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ClusterResponse {
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    pub name: String,
    #[serde(deserialize_with = "deserialize_role")]
    pub role: Role,
    pub state: String,
    pub host: String,
    pub port: u16,
    pub api_url: Option<String>,
    pub lag: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Leader,
    SyncStandby,
    Replica,
    Other(String),
}

fn deserialize_role<'de, D>(deserializer: D) -> Result<Role, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(match s.as_str() {
        "leader" => Role::Leader,
        "sync_standby" => Role::SyncStandby,
        "replica" => Role::Replica,
        other => Role::Other(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_cluster_response() {
        let json = r#"{
            "members": [
                {
                    "name": "pg-primary",
                    "role": "leader",
                    "state": "running",
                    "host": "10.0.0.1",
                    "port": 5432,
                    "api_url": "http://10.0.0.1:8008/patroni",
                    "lag": 0
                },
                {
                    "name": "pg-replica",
                    "role": "replica",
                    "state": "streaming",
                    "host": "10.0.0.2",
                    "port": 5432,
                    "api_url": "http://10.0.0.2:8008/patroni",
                    "lag": 10
                }
            ]
        }"#;

        let cluster: ClusterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(cluster.members.len(), 2);

        let primary = &cluster.members[0];
        assert_eq!(primary.name, "pg-primary");
        assert_eq!(primary.role, Role::Leader);
        assert_eq!(primary.state, "running");
        assert_eq!(primary.host, "10.0.0.1");
        assert_eq!(primary.port, 5432);
        assert_eq!(
            primary.api_url.as_deref(),
            Some("http://10.0.0.1:8008/patroni")
        );
        assert_eq!(primary.lag, Some(0));

        let replica = &cluster.members[1];
        assert_eq!(replica.name, "pg-replica");
        assert_eq!(replica.role, Role::Replica);
        assert_eq!(replica.lag, Some(10));
    }

    #[test]
    fn parse_empty_members() {
        let json = r#"{ "members": [] }"#;
        let cluster: ClusterResponse = serde_json::from_str(json).unwrap();
        assert!(cluster.members.is_empty());
    }

    #[test]
    fn parse_unknown_role() {
        let json = r#"{
            "members": [
                {
                    "name": "pg-standby-leader",
                    "role": "standby_leader",
                    "state": "running",
                    "host": "10.0.0.3",
                    "port": 5432
                }
            ]
        }"#;

        let cluster: ClusterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(cluster.members.len(), 1);
        assert_eq!(
            cluster.members[0].role,
            Role::Other("standby_leader".to_string())
        );
    }

    #[test]
    fn parse_missing_optional_fields() {
        let json = r#"{
            "members": [
                {
                    "name": "pg-replica",
                    "role": "sync_standby",
                    "state": "streaming",
                    "host": "10.0.0.4",
                    "port": 5433
                }
            ]
        }"#;

        let cluster: ClusterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(cluster.members.len(), 1);

        let member = &cluster.members[0];
        assert_eq!(member.role, Role::SyncStandby);
        assert!(member.api_url.is_none());
        assert!(member.lag.is_none());
    }
}
