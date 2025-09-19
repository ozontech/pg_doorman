use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{IpAddr};
use std::str::FromStr;

/// Represents a connection type in pg_hba.conf
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConnectionType {
    /// Local Unix domain socket connections
    Local,
    /// TCP/IP connections (encrypted or non-encrypted)
    Host,
    /// SSL-encrypted TCP/IP connections only
    Hostssl,
    /// Non-SSL TCP/IP connections only
    Hostnossl,
    /// Local connections over Unix domain sockets with specific authentication
    Hostgssenc,
    /// Local connections over Unix domain sockets without GSS encryption
    Hostnogssenc,
}

impl FromStr for ConnectionType {
    type Err = PgHbaError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "local" => Ok(ConnectionType::Local),
            "host" => Ok(ConnectionType::Host),
            "hostssl" => Ok(ConnectionType::Hostssl),
            "hostnossl" => Ok(ConnectionType::Hostnossl),
            "hostgssenc" => Ok(ConnectionType::Hostgssenc),
            "hostnogssenc" => Ok(ConnectionType::Hostnogssenc),
            _ => Err(PgHbaError::InvalidConnectionType(s.to_string())),
        }
    }
}

impl fmt::Display for ConnectionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ConnectionType::Local => "local",
            ConnectionType::Host => "host",
            ConnectionType::Hostssl => "hostssl",
            ConnectionType::Hostnossl => "hostnossl",
            ConnectionType::Hostgssenc => "hostgssenc",
            ConnectionType::Hostnogssenc => "hostnogssenc",
        };
        write!(f, "{}", s)
    }
}

/// Represents an authentication method in pg_hba.conf
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AuthMethod {
    /// Trust authentication (no password required)
    Trust,
    /// Reject authentication
    Reject,
    /// MD5 password authentication
    Md5,
    /// SCRAM-SHA-256 password authentication
    ScramSha256,
    /// GSS/Kerberos authentication
    Gss,
    /// SSPI authentication (Windows)
    Sspi,
    /// Ident authentication
    Ident,
    /// Peer authentication
    Peer,
    /// LDAP authentication
    Ldap,
    /// RADIUS authentication
    Radius,
    /// Certificate authentication
    Cert,
    /// PAM authentication
    Pam,
    /// BSD authentication
    Bsd,
    /// Password authentication (generic)
    Password,
}

impl FromStr for AuthMethod {
    type Err = PgHbaError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "trust" => Ok(AuthMethod::Trust),
            "reject" => Ok(AuthMethod::Reject),
            "md5" => Ok(AuthMethod::Md5),
            "scram-sha-256" => Ok(AuthMethod::ScramSha256),
            "gss" => Ok(AuthMethod::Gss),
            "sspi" => Ok(AuthMethod::Sspi),
            "ident" => Ok(AuthMethod::Ident),
            "peer" => Ok(AuthMethod::Peer),
            "ldap" => Ok(AuthMethod::Ldap),
            "radius" => Ok(AuthMethod::Radius),
            "cert" => Ok(AuthMethod::Cert),
            "pam" => Ok(AuthMethod::Pam),
            "bsd" => Ok(AuthMethod::Bsd),
            "password" => Ok(AuthMethod::Password),
            _ => Err(PgHbaError::InvalidAuthMethod(s.to_string())),
        }
    }
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            AuthMethod::Trust => "trust",
            AuthMethod::Reject => "reject",
            AuthMethod::Md5 => "md5",
            AuthMethod::ScramSha256 => "scram-sha-256",
            AuthMethod::Gss => "gss",
            AuthMethod::Sspi => "sspi",
            AuthMethod::Ident => "ident",
            AuthMethod::Peer => "peer",
            AuthMethod::Ldap => "ldap",
            AuthMethod::Radius => "radius",
            AuthMethod::Cert => "cert",
            AuthMethod::Pam => "pam",
            AuthMethod::Bsd => "bsd",
            AuthMethod::Password => "password",
        };
        write!(f, "{}", s)
    }
}

/// Represents an address specification in pg_hba.conf
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Address {
    /// All addresses
    All,
    /// Specific IP address with optional CIDR mask
    Ip { addr: IpAddr, mask: Option<u8> },
    /// Hostname
    Hostname(String),
    /// Same host as server
    Samehost,
    /// Same network as server
    Samenet,
}

impl FromStr for Address {
    type Err = PgHbaError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "all" => Ok(Address::All),
            "samehost" => Ok(Address::Samehost),
            "samenet" => Ok(Address::Samenet),
            _ => {
                // Try to parse as IP address with optional CIDR
                if let Some(slash_pos) = s.find('/') {
                    let addr_str = &s[..slash_pos];
                    let mask_str = &s[slash_pos + 1..];

                    let addr = addr_str
                        .parse::<IpAddr>()
                        .map_err(|_| PgHbaError::InvalidAddress(s.to_string()))?;
                    let mask = mask_str
                        .parse::<u8>()
                        .map_err(|_| PgHbaError::InvalidAddress(s.to_string()))?;

                    Ok(Address::Ip {
                        addr,
                        mask: Some(mask),
                    })
                } else if let Ok(addr) = s.parse::<IpAddr>() {
                    Ok(Address::Ip { addr, mask: None })
                } else {
                    // Treat as hostname
                    Ok(Address::Hostname(s.to_string()))
                }
            }
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::All => write!(f, "all"),
            Address::Ip {
                addr,
                mask: Some(mask),
            } => write!(f, "{}/{}", addr, mask),
            Address::Ip { addr, mask: None } => write!(f, "{}", addr),
            Address::Hostname(hostname) => write!(f, "{}", hostname),
            Address::Samehost => write!(f, "samehost"),
            Address::Samenet => write!(f, "samenet"),
        }
    }
}

/// Represents a single entry in pg_hba.conf
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PgHbaEntry {
    /// Type of connection
    pub connection_type: ConnectionType,
    /// Database name(s)
    pub database: Vec<String>,
    /// User name(s)
    pub user: Vec<String>,
    /// Address specification (only for non-local connections)
    pub address: Option<Address>,
    /// Authentication method
    pub auth_method: AuthMethod,
    /// Additional auth options as key-value pairs
    pub auth_options: Vec<(String, String)>,
}

/// Represents a parsed pg_hba.conf file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PgHbaConfig {
    /// List of entries in order
    pub entries: Vec<PgHbaEntry>,
}

/// Errors that can occur during pg_hba parsing
#[derive(Debug, Clone)]
pub enum PgHbaError {
    InvalidConnectionType(String),
    InvalidAuthMethod(String),
    InvalidAddress(String),
    InvalidFormat(String),
    IoError(String),
}

impl fmt::Display for PgHbaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgHbaError::InvalidConnectionType(s) => write!(f, "Invalid connection type: {}", s),
            PgHbaError::InvalidAuthMethod(s) => write!(f, "Invalid auth method: {}", s),
            PgHbaError::InvalidAddress(s) => write!(f, "Invalid address: {}", s),
            PgHbaError::InvalidFormat(s) => write!(f, "Invalid format: {}", s),
            PgHbaError::IoError(s) => write!(f, "IO error: {}", s),
        }
    }
}

impl std::error::Error for PgHbaError {}

impl PgHbaConfig {
    /// Check client connection against pg_hba rules and return matching AuthMethod
    pub fn check_in_hba(
        &self,
        client_addr: IpAddr,
        username: &str,
        dbname: &str,
        tls: bool,
    ) -> AuthMethod {
        // Iterate through entries in order to find the first match
        for entry in &self.entries {
            // Check connection type compatibility
            let connection_matches = match entry.connection_type {
                ConnectionType::Local => false, // TCP connections are never "local"
                ConnectionType::Host => true,   // Any TCP connection
                ConnectionType::Hostssl => tls, // Only if TLS is enabled
                ConnectionType::Hostnossl => !tls, // Only if TLS is disabled
                ConnectionType::Hostgssenc => false, // GSS encryption not supported
                ConnectionType::Hostnogssenc => true, // No GSS encryption
            };

            if !connection_matches {
                continue;
            }

            // Check address match
            if let Some(ref address) = entry.address {
                if !self.address_matches(address, client_addr) {
                    continue;
                }
            }

            // Check database match
            if !self.name_matches(&entry.database, dbname) {
                continue;
            }

            // Check user match
            if !self.name_matches(&entry.user, username) {
                continue;
            }

            // All conditions match, return the auth method
            return entry.auth_method.clone();
        }

        // No matching rule found, default to reject
        AuthMethod::Reject
    }

    /// Check if an address matches the client IP
    fn address_matches(&self, address: &Address, client_addr: IpAddr) -> bool {
        match address {
            Address::All => true,
            Address::Ip { addr, mask } => {
                match (addr, client_addr, mask) {
                    (IpAddr::V4(rule_addr), IpAddr::V4(client_addr), Some(mask)) => {
                        let rule_addr_u32 = u32::from(*rule_addr);
                        let client_addr_u32 = u32::from(client_addr);
                        let mask_bits = *mask;

                        if mask_bits > 32 {
                            return false;
                        }

                        let network_mask = if mask_bits == 0 {
                            0
                        } else {
                            !0u32 << (32 - mask_bits)
                        };

                        (rule_addr_u32 & network_mask) == (client_addr_u32 & network_mask)
                    }
                    (IpAddr::V6(rule_addr), IpAddr::V6(client_addr), Some(mask)) => {
                        let rule_addr_bytes = rule_addr.octets();
                        let client_addr_bytes = client_addr.octets();
                        let mask_bits = *mask;

                        if mask_bits > 128 {
                            return false;
                        }

                        let full_bytes = mask_bits / 8;
                        let remaining_bits = mask_bits % 8;

                        // Check full bytes
                        for i in 0..full_bytes as usize {
                            if rule_addr_bytes[i] != client_addr_bytes[i] {
                                return false;
                            }
                        }

                        // Check remaining bits
                        if remaining_bits > 0 {
                            let byte_idx = full_bytes as usize;
                            if byte_idx < 16 {
                                let mask_byte = !0u8 << (8 - remaining_bits);
                                if (rule_addr_bytes[byte_idx] & mask_byte)
                                    != (client_addr_bytes[byte_idx] & mask_byte)
                                {
                                    return false;
                                }
                            }
                        }

                        true
                    }
                    (rule_addr, client_addr, None) => rule_addr == &client_addr,
                    _ => false, // IPv4 vs IPv6 mismatch
                }
            }
            Address::Hostname(_) => {
                // Hostname resolution not implemented for simplicity
                // In a real implementation, this would require reverse DNS lookup
                false
            }
            Address::Samehost => {
                // Samehost means connecting from the same host as the server
                // This is complex to implement and would require knowing the server's addresses
                false
            }
            Address::Samenet => {
                // Samenet means connecting from the same subnet as the server
                // This is complex to implement and would require knowing the server's network
                false
            }
        }
    }

    /// Check if a name matches against a list of allowed names
    fn name_matches(&self, allowed_names: &[String], name: &str) -> bool {
        for allowed_name in allowed_names {
            if allowed_name == "all" || allowed_name == name {
                return true;
            }
        }
        false
    }

    /// Parse a pg_hba.conf file from string content
    pub fn parse(content: &str) -> Result<Self, PgHbaError> {
        let mut entries = Vec::new();

        for (line_num, line) in content.lines().enumerate() {
            // Skip empty lines and comments
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // Parse the line
            match Self::parse_line(trimmed) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    return Err(PgHbaError::InvalidFormat(format!(
                        "Line {}: {}",
                        line_num + 1,
                        e
                    )))
                }
            }
        }

        Ok(PgHbaConfig { entries })
    }

    /// Parse a single line of pg_hba.conf
    fn parse_line(line: &str) -> Result<PgHbaEntry, PgHbaError> {
        let parts: Vec<&str> = line.split_whitespace().collect();

        if parts.len() < 4 {
            return Err(PgHbaError::InvalidFormat(
                "Insufficient number of fields".to_string(),
            ));
        }

        let connection_type = parts[0].parse::<ConnectionType>()?;
        let database = Self::parse_name_list(parts[1]);
        let user = Self::parse_name_list(parts[2]);

        let (address, auth_start_idx) = match connection_type {
            ConnectionType::Local => (None, 3),
            _ => {
                let address = parts[3].parse::<Address>()?;
                (Some(address), 4)
            }
        };

        if parts.len() <= auth_start_idx {
            return Err(PgHbaError::InvalidFormat(
                "Missing authentication method".to_string(),
            ));
        }

        let auth_method = parts[auth_start_idx].parse::<AuthMethod>()?;

        // Parse auth options (key=value pairs)
        let mut auth_options = Vec::new();
        for &part in parts.iter().skip(auth_start_idx + 1) {
            if let Some(eq_pos) = part.find('=') {
                let key = part[..eq_pos].to_string();
                let value = part[eq_pos + 1..].to_string();
                auth_options.push((key, value));
            }
        }

        Ok(PgHbaEntry {
            connection_type,
            database,
            user,
            address,
            auth_method,
            auth_options,
        })
    }

    /// Parse a comma-separated list of names
    fn parse_name_list(s: &str) -> Vec<String> {
        s.split(',').map(|name| name.trim().to_string()).collect()
    }

    /// Load pg_hba.conf from file
    pub fn from_file(path: &str) -> Result<Self, PgHbaError> {
        let content =
            std::fs::read_to_string(path).map_err(|e| PgHbaError::IoError(e.to_string()))?;
        Self::parse(&content)
    }

    /// Save pg_hba.conf to file
    pub fn to_file(&self, path: &str) -> Result<(), PgHbaError> {
        let content = self.to_string();
        std::fs::write(path, content).map_err(|e| PgHbaError::IoError(e.to_string()))?;
        Ok(())
    }

    /// Convert to string format
    pub fn to_string(&self) -> String {
        let mut lines = Vec::new();

        for entry in &self.entries {
            let mut parts = Vec::new();

            parts.push(entry.connection_type.to_string());
            parts.push(entry.database.join(","));
            parts.push(entry.user.join(","));

            if let Some(ref address) = entry.address {
                parts.push(address.to_string());
            }

            parts.push(entry.auth_method.to_string());

            for (key, value) in &entry.auth_options {
                parts.push(format!("{}={}", key, value));
            }

            lines.push(parts.join("\t"));
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_local() {
        let content = "local   all             all                                     trust";
        let config = PgHbaConfig::parse(content).unwrap();

        assert_eq!(config.entries.len(), 1);
        let entry = &config.entries[0];

        assert_eq!(entry.connection_type, ConnectionType::Local);
        assert_eq!(entry.database, vec!["all"]);
        assert_eq!(entry.user, vec!["all"]);
        assert_eq!(entry.address, None);
        assert_eq!(entry.auth_method, AuthMethod::Trust);
    }

    #[test]
    fn test_parse_host_with_ip() {
        let content = "host    all             all             127.0.0.1/32            trust";
        let config = PgHbaConfig::parse(content).unwrap();

        assert_eq!(config.entries.len(), 1);
        let entry = &config.entries[0];

        assert_eq!(entry.connection_type, ConnectionType::Host);
        assert_eq!(entry.database, vec!["all"]);
        assert_eq!(entry.user, vec!["all"]);
        assert_eq!(
            entry.address,
            Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                mask: Some(32)
            })
        );
        assert_eq!(entry.auth_method, AuthMethod::Trust);
    }

    #[test]
    fn test_parse_multiple_entries() {
        let content = r#"
# Comment line
local   all             all                                     trust
host    all             all             127.0.0.1/32            trust
host    mydb            user1,user2     192.168.1.0/24          md5
"#;
        let config = PgHbaConfig::parse(content).unwrap();

        assert_eq!(config.entries.len(), 3);
    }

    #[test]
    fn test_to_string() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["mydb".to_string()],
            user: vec!["user1".to_string(), "user2".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
                mask: Some(24),
            }),
            auth_method: AuthMethod::Md5,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };
        let result = config.to_string();

        assert!(result.contains("host\tmydb\tuser1,user2\t192.168.1.0/24\tmd5"));
    }

    // Tests for check_in_hba method

    #[test]
    fn test_check_in_hba_basic_host_match() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["mydb".to_string()],
            user: vec!["testuser".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
                mask: Some(24),
            }),
            auth_method: AuthMethod::Md5,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match
        let result = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
            "testuser",
            "mydb",
            false,
        );
        assert_eq!(result, AuthMethod::Md5);
    }

    #[test]
    fn test_check_in_hba_connection_type_host() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::All),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Host should match both TLS and non-TLS
        let result_tls =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", true);
        assert_eq!(result_tls, AuthMethod::Trust);

        let result_no_tls =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", false);
        assert_eq!(result_no_tls, AuthMethod::Trust);
    }

    #[test]
    fn test_check_in_hba_connection_type_hostssl() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Hostssl,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::All),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match only with TLS
        let result_tls =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", true);
        assert_eq!(result_tls, AuthMethod::Trust);

        // Should not match without TLS
        let result_no_tls =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", false);
        assert_eq!(result_no_tls, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_connection_type_hostnossl() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Hostnossl,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::All),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should not match with TLS
        let result_tls =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", true);
        assert_eq!(result_tls, AuthMethod::Reject);

        // Should match without TLS
        let result_no_tls =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", false);
        assert_eq!(result_no_tls, AuthMethod::Trust);
    }

    #[test]
    fn test_check_in_hba_connection_type_local() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Local,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: None,
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Local connections should never match TCP connections
        let result =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", false);
        assert_eq!(result, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_address_all() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::All),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match any IP address
        let result_v4 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            "user",
            "db",
            false,
        );
        assert_eq!(result_v4, AuthMethod::Trust);

        let result_v6 = config.check_in_hba("2001:db8::1".parse().unwrap(), "user", "db", false);
        assert_eq!(result_v6, AuthMethod::Trust);
    }

    #[test]
    fn test_check_in_hba_ipv4_cidr_matching() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
                mask: Some(24),
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match addresses in the 192.168.1.0/24 network
        let result_match = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
            "user",
            "db",
            false,
        );
        assert_eq!(result_match, AuthMethod::Trust);

        // Should not match addresses outside the network
        let result_no_match = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(192, 168, 2, 100)),
            "user",
            "db",
            false,
        );
        assert_eq!(result_no_match, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_ipv4_exact_match() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                mask: None,
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match exact IP
        let result_match =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", false);
        assert_eq!(result_match, AuthMethod::Trust);

        // Should not match different IP
        let result_no_match =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), "user", "db", false);
        assert_eq!(result_no_match, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_ipv6_cidr_matching() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Ip {
                addr: "2001:db8::".parse().unwrap(),
                mask: Some(32),
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match addresses in the 2001:db8::/32 network
        let result_match = config.check_in_hba("2001:db8::1".parse().unwrap(), "user", "db", false);
        assert_eq!(result_match, AuthMethod::Trust);

        // Should not match addresses outside the network
        let result_no_match =
            config.check_in_hba("2001:db9::1".parse().unwrap(), "user", "db", false);
        assert_eq!(result_no_match, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_ipv4_ipv6_mismatch() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
                mask: Some(24),
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // IPv6 client should not match IPv4 rule
        let result = config.check_in_hba("2001:db8::1".parse().unwrap(), "user", "db", false);
        assert_eq!(result, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_hostname_address() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Hostname("example.com".to_string())),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Hostname resolution is not implemented, should not match
        let result =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", false);
        assert_eq!(result, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_samehost_samenet() {
        let entries = vec![
            PgHbaEntry {
                connection_type: ConnectionType::Host,
                database: vec!["all".to_string()],
                user: vec!["all".to_string()],
                address: Some(Address::Samehost),
                auth_method: AuthMethod::Trust,
                auth_options: vec![],
            },
            PgHbaEntry {
                connection_type: ConnectionType::Host,
                database: vec!["all".to_string()],
                user: vec!["all".to_string()],
                address: Some(Address::Samenet),
                auth_method: AuthMethod::Md5,
                auth_options: vec![],
            },
        ];

        let config = PgHbaConfig { entries };

        // Samehost and samenet are not implemented, should not match
        let result =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "user", "db", false);
        assert_eq!(result, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_database_matching() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["mydb".to_string(), "testdb".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::All),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match allowed databases
        let result_match1 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user",
            "mydb",
            false,
        );
        assert_eq!(result_match1, AuthMethod::Trust);

        let result_match2 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user",
            "testdb",
            false,
        );
        assert_eq!(result_match2, AuthMethod::Trust);

        // Should not match other databases
        let result_no_match = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user",
            "otherdb",
            false,
        );
        assert_eq!(result_no_match, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_user_matching() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["user1".to_string(), "user2".to_string()],
            address: Some(Address::All),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match allowed users
        let result_match1 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user1",
            "db",
            false,
        );
        assert_eq!(result_match1, AuthMethod::Trust);

        let result_match2 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user2",
            "db",
            false,
        );
        assert_eq!(result_match2, AuthMethod::Trust);

        // Should not match other users
        let result_no_match = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user3",
            "db",
            false,
        );
        assert_eq!(result_no_match, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_all_keyword() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::All),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should match any database and user when using "all"
        let result = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "anyuser",
            "anydb",
            false,
        );
        assert_eq!(result, AuthMethod::Trust);
    }

    #[test]
    fn test_check_in_hba_first_match_wins() {
        let entries = vec![
            PgHbaEntry {
                connection_type: ConnectionType::Host,
                database: vec!["all".to_string()],
                user: vec!["all".to_string()],
                address: Some(Address::Ip {
                    addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
                    mask: Some(24),
                }),
                auth_method: AuthMethod::Trust,
                auth_options: vec![],
            },
            PgHbaEntry {
                connection_type: ConnectionType::Host,
                database: vec!["all".to_string()],
                user: vec!["all".to_string()],
                address: Some(Address::All),
                auth_method: AuthMethod::Md5,
                auth_options: vec![],
            },
        ];

        let config = PgHbaConfig { entries };

        // Should match the first rule (Trust) for IPs in 192.168.1.0/24
        let result = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
            "user",
            "db",
            false,
        );
        assert_eq!(result, AuthMethod::Trust);

        // Should match the second rule (Md5) for other IPs
        let result2 =
            config.check_in_hba(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), "user", "db", false);
        assert_eq!(result2, AuthMethod::Md5);
    }

    #[test]
    fn test_check_in_hba_no_match_returns_reject() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Hostssl,
            database: vec!["specificdb".to_string()],
            user: vec!["specificuser".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)),
                mask: Some(8),
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should return Reject when no rules match
        let result = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            "otheruser",
            "otherdb",
            false,
        );
        assert_eq!(result, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_multiple_auth_methods() {
        let entries = vec![
            PgHbaEntry {
                connection_type: ConnectionType::Host,
                database: vec!["db1".to_string()],
                user: vec!["all".to_string()],
                address: Some(Address::All),
                auth_method: AuthMethod::Trust,
                auth_options: vec![],
            },
            PgHbaEntry {
                connection_type: ConnectionType::Host,
                database: vec!["db2".to_string()],
                user: vec!["all".to_string()],
                address: Some(Address::All),
                auth_method: AuthMethod::Md5,
                auth_options: vec![],
            },
            PgHbaEntry {
                connection_type: ConnectionType::Host,
                database: vec!["db3".to_string()],
                user: vec!["all".to_string()],
                address: Some(Address::All),
                auth_method: AuthMethod::ScramSha256,
                auth_options: vec![],
            },
        ];

        let config = PgHbaConfig { entries };

        // Test different auth methods
        let result1 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user",
            "db1",
            false,
        );
        assert_eq!(result1, AuthMethod::Trust);

        let result2 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user",
            "db2",
            false,
        );
        assert_eq!(result2, AuthMethod::Md5);

        let result3 = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "user",
            "db3",
            false,
        );
        assert_eq!(result3, AuthMethod::ScramSha256);
    }

    #[test]
    fn test_check_in_hba_edge_case_zero_mask() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
                mask: Some(0),
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // With mask 0, should match any IPv4 address
        let result = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(10, 20, 30, 40)),
            "user",
            "db",
            false,
        );
        assert_eq!(result, AuthMethod::Trust);
    }

    #[test]
    fn test_check_in_hba_invalid_mask_ipv4() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Ip {
                addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
                mask: Some(33), // Invalid mask for IPv4
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should not match with invalid mask
        let result = config.check_in_hba(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            "user",
            "db",
            false,
        );
        assert_eq!(result, AuthMethod::Reject);
    }

    #[test]
    fn test_check_in_hba_invalid_mask_ipv6() {
        let entry = PgHbaEntry {
            connection_type: ConnectionType::Host,
            database: vec!["all".to_string()],
            user: vec!["all".to_string()],
            address: Some(Address::Ip {
                addr: "2001:db8::".parse().unwrap(),
                mask: Some(129), // Invalid mask for IPv6
            }),
            auth_method: AuthMethod::Trust,
            auth_options: vec![],
        };

        let config = PgHbaConfig {
            entries: vec![entry],
        };

        // Should not match with invalid mask
        let result = config.check_in_hba("2001:db8::1".parse().unwrap(), "user", "db", false);
        assert_eq!(result, AuthMethod::Reject);
    }
}
