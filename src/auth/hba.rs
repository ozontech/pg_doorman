use std::net::IpAddr;
use std::path::Path;
use std::{fs, str::FromStr};

use ipnet::IpNet;

/// Authentication method supported by our checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    Trust,
    Md5,
    ScramSha256,
    Reject,
    Other(String), // keep unrecognized for completeness
}

impl AuthMethod {
    fn from_token(tok: &str) -> Self {
        match tok.to_ascii_lowercase().as_str() {
            "trust" => AuthMethod::Trust,
            "md5" => AuthMethod::Md5,
            "scram-sha-256" | "scram_sha_256" | "scramsha256" => AuthMethod::ScramSha256,
            "reject" => AuthMethod::Reject,
            other => AuthMethod::Other(other.to_string()),
        }
    }
}

/// Matcher for database/user fields (supports keyword `all`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameMatcher {
    All,
    Name(String),
}

impl NameMatcher {
    fn from_token(tok: &str) -> Self {
        if tok.eq_ignore_ascii_case("all") {
            NameMatcher::All
        } else {
            NameMatcher::Name(tok.to_string())
        }
    }
    fn matches(&self, value: &str) -> bool {
        match self {
            NameMatcher::All => true,
            NameMatcher::Name(ref n) => n == value,
        }
    }
}

/// A single pg_hba.conf rule reduced to what we need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HbaRule {
    pub host_type: HostType,
    pub database: NameMatcher,
    pub user: NameMatcher,
    pub address: Option<IpNet>,
    pub method: AuthMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostType {
    Local,
    Host,
    HostSSL,
    HostNoSSL,
}

impl HostType {
    fn from_token(tok: &str) -> Option<Self> {
        match tok.to_ascii_lowercase().as_str() {
            "local" => Some(HostType::Local),
            "host" => Some(HostType::Host),
            "hostssl" => Some(HostType::HostSSL),
            "hostnossl" => Some(HostType::HostNoSSL),
            _ => None,
        }
    }

    fn matches_ssl(&self, ssl: bool) -> bool {
        match self {
            HostType::Local => true,
            HostType::Host => true,
            HostType::HostSSL => ssl,
            HostType::HostNoSSL => !ssl,
        }
    }
}

/// Result of `check_hba` evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckResult {
    /// No HBA rule matched for given connection parameters and auth type
    NotMatched,
    /// Explicitly forbidden by a matching `reject` rule
    Deny,
    /// Matched rule allows given auth type
    Allow,
    /// Matched rule with `trust` method (no password expected)
    Trust,
}

/// Parsed pg_hba set of rules, in order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PgHba {
    pub rules: Vec<HbaRule>,
}

// Human-readable formatting for pg_hba components
impl std::fmt::Display for NameMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameMatcher::All => f.write_str("all"),
            NameMatcher::Name(s) => f.write_str(s),
        }
    }
}

impl std::fmt::Display for HostType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            HostType::Local => "local",
            HostType::Host => "host",
            HostType::HostSSL => "hostssl",
            HostType::HostNoSSL => "hostnossl",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMethod::Trust => f.write_str("trust"),
            AuthMethod::Md5 => f.write_str("md5"),
            AuthMethod::ScramSha256 => f.write_str("scram-sha-256"),
            AuthMethod::Reject => f.write_str("reject"),
            AuthMethod::Other(s) => f.write_str(s),
        }
    }
}

impl std::fmt::Display for HbaRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.host_type {
            HostType::Local => {
                write!(
                    f,
                    "{} {} {} {}",
                    self.host_type, self.database, self.user, self.method
                )
            }
            _ => {
                if let Some(addr) = &self.address {
                    write!(
                        f,
                        "{} {} {} {} {}",
                        self.host_type, self.database, self.user, addr, self.method
                    )
                } else {
                    // address missing (unknown format when parsed) — emit without it
                    write!(
                        f,
                        "{} {} {} {}",
                        self.host_type, self.database, self.user, self.method
                    )
                }
            }
        }
    }
}

impl std::fmt::Display for PgHba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, rule) in self.rules.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{rule}")?;
        }
        Ok(())
    }
}

// Serde support: you can define this in TOML as a string (inline content),
// or as a table with either { path = "..." } or { content = "..." }.
// Examples:
//   hba = """
//   host all all 0.0.0.0/0 md5
//   hostssl all all 10.0.0.0/8 scram-sha-256
//   """
//   hba = { path = "./pg_hba.conf" }
//   hba = { content = "host all all 127.0.0.1/32 trust" }
impl<'de> serde::Deserialize<'de> for PgHba {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error as DeError, MapAccess, Visitor};
        use std::fmt;

        struct PgHbaVisitor;

        impl<'de> Visitor<'de> for PgHbaVisitor {
            type Value = PgHba;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string with pg_hba content or a map with { path = \"...\" } or { content = \"...\" }")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(PgHba::from_content(v))
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(PgHba::from_content(&v))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut path: Option<String> = None;
                let mut content: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "path" => {
                            if path.is_some() {
                                return Err(DeError::duplicate_field("path"));
                            }
                            path = Some(map.next_value()?);
                        }
                        "content" => {
                            if content.is_some() {
                                return Err(DeError::duplicate_field("content"));
                            }
                            content = Some(map.next_value()?);
                        }
                        other => {
                            // consume and ignore unknown
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                            return Err(DeError::unknown_field(other, &["path", "content"]));
                        }
                    }
                }

                if let Some(c) = content {
                    return Ok(PgHba::from_content(&c));
                }
                if let Some(p) = path {
                    let data = fs::read_to_string(&p).map_err(|e| {
                        DeError::custom(format!("failed to read hba file {p}: {e}"))
                    })?;
                    return Ok(PgHba::from_content(&data));
                }
                Err(DeError::custom(
                    "expected either 'path' or 'content' field for PgHba",
                ))
            }
        }

        deserializer.deserialize_any(PgHbaVisitor)
    }
}

impl PgHba {
    /// Parse from file path (utf-8 text expected)
    pub fn from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        Ok(Self::from_content(&content))
    }

    /// Parse from string content of a pg_hba.conf
    pub fn from_content(content: &str) -> Self {
        let mut rules = Vec::new();
        for raw_line in content.lines() {
            let line = strip_comments(raw_line).trim();
            if line.is_empty() {
                continue;
            }

            let tokens = shell_like_split(line);
            if tokens.is_empty() {
                continue;
            }

            // connection type
            let Some(ht) = HostType::from_token(&tokens[0]) else {
                continue;
            };

            // Minimal pg_hba format:
            // type  database  user  address  method  [options]
            // For local, address is omitted.
            // We ignore database/user/options in this component.

            // Ensure we have enough tokens to read method and optional address.
            // We'll map positions based on host type.
            // Parse database and user (common positions)
            if tokens.len() < 3 {
                continue;
            }
            let database = NameMatcher::from_token(&tokens[1]);
            let user = NameMatcher::from_token(&tokens[2]);

            let (method_idx, address_opt) = match ht {
                HostType::Local => {
                    // type database user method [options]
                    if tokens.len() < 4 {
                        continue;
                    }
                    let method_idx = 3;
                    (method_idx, None)
                }
                _ => {
                    // type database user address method [options]
                    if tokens.len() < 5 {
                        continue;
                    }
                    let addr_token = &tokens[3];
                    let address = parse_address(addr_token);
                    let method_idx = 4;
                    (method_idx, address)
                }
            };

            let method = AuthMethod::from_token(tokens[method_idx].as_str());

            rules.push(HbaRule {
                host_type: ht,
                database,
                user,
                address: address_opt,
                method,
            });
        }
        PgHba { rules }
    }

    /// Evaluate given connection parameters against parsed HBA rules.
    ///
    /// - `ip`: client IP address
    /// - `ssl`: whether the client connection is over SSL
    /// - `type_auth`: requested auth method name, e.g. "md5" or "scram-sha-256"
    /// - `username`: database user name
    /// - `database`: target database name
    ///
    /// Returns `CheckResult::Trust` when a matching `trust` rule is found,
    /// `CheckResult::Allow` when a matching rule with method equal to `type_auth` is found,
    /// `CheckResult::Deny` only when a matching `reject` rule is encountered,
    /// otherwise `CheckResult::NotMatched`.
    pub fn check_hba(
        &self,
        ip: IpAddr,
        ssl: bool,
        type_auth: &str,
        username: &str,
        database: &str,
    ) -> CheckResult {
        let want = match type_auth.to_ascii_lowercase().as_str() {
            "md5" => AuthMethod::Md5,
            "scram-sha-256" | "scram_sha_256" | "scramsha256" => AuthMethod::ScramSha256,
            _ => AuthMethod::Other(type_auth.to_string()),
        };

        for rule in &self.rules {
            // Skip local rules; they are intended only for unix-socket connections
            if matches!(rule.host_type, HostType::Local) {
                continue;
            }
            if !rule.host_type.matches_ssl(ssl) {
                continue;
            }
            if let Some(net) = &rule.address {
                if !net.contains(&ip) {
                    continue;
                }
            }
            // Database and user must match as well (supporting keyword `all`).
            if !rule.database.matches(database) || !rule.user.matches(username) {
                continue;
            }

            // First matching rule that applies decides.
            match rule.method {
                AuthMethod::Trust => return CheckResult::Trust,
                ref m if *m == want => return CheckResult::Allow,
                AuthMethod::Reject => return CheckResult::Deny,
                _ => continue, // different method: not a decision, keep searching
            }
        }
        CheckResult::NotMatched
    }
}

fn strip_comments(s: &str) -> &str {
    match s.find('#') {
        Some(idx) => &s[..idx],
        None => s,
    }
}

/// Very small splitter that treats consecutive whitespace as separators and supports
/// double-quoted tokens with spaces (like "db name"). It does not support escapes inside quotes.
fn shell_like_split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;

    for c in line.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_address(token: &str) -> Option<IpNet> {
    // token may be:
    // - a CIDR: 192.168.0.0/24 or 2001:db8::/32
    // - an IP + mask: 192.168.0.0 255.255.255.0 (but this would be two tokens; we don't support here)
    // - a single IP meaning /32 or /128 (not standard for pg_hba, so keep to CIDR)
    IpNet::from_str(token).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    const SAMPLE: &str = r#"
# comment
host all all 10.0.0.0/8 md5
hostssl all all 192.168.0.0/16 scram-sha-256
hostnossl all all 127.0.0.1/32 trust
"#;

    #[test]
    fn parse_and_check() {
        let hba = PgHba::from_content(SAMPLE);
        assert_eq!(hba.rules.len(), 3);

        // md5 allowed for 10.1.2.3 over non-ssl and ssl (host matches both)
        let ip = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        assert_eq!(
            hba.check_hba(ip, false, "md5", "alice", "app"),
            CheckResult::Allow
        );
        assert_eq!(
            hba.check_hba(ip, true, "md5", "alice", "app"),
            CheckResult::Allow
        );

        // scram allowed for 192.168.1.10 only with ssl
        let ip2 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(
            hba.check_hba(ip2, true, "scram-sha-256", "alice", "app"),
            CheckResult::Allow
        );
        assert_eq!(
            hba.check_hba(ip2, false, "scram-sha-256", "alice", "app"),
            CheckResult::NotMatched
        );

        // trust on localhost without ssl
        let ip3 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(
            hba.check_hba(ip3, false, "md5", "alice", "app"),
            CheckResult::Trust
        );
    }

    // ----- Serde tests -----
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Wrapper {
        hba: PgHba,
    }

    #[test]
    fn serde_inline_string() {
        let toml_in = r#"
            hba = """
            host all all 127.0.0.1/32 trust
            host all all 10.0.0.0/8 md5
            """
        "#;
        let cfg: Wrapper = toml::from_str(toml_in).expect("toml parse inline string");
        assert_eq!(cfg.hba.rules.len(), 2);
        // First rule trust for 127.0.0.1
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(
            cfg.hba.check_hba(ip, false, "md5", "alice", "app"),
            CheckResult::Trust
        );
        // Second rule md5 for 10.1.2.3
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        assert_eq!(
            cfg.hba.check_hba(ip2, false, "md5", "alice", "app"),
            CheckResult::Allow
        );
    }

    #[test]
    fn serde_map_content() {
        let toml_in = r#"
            hba = { content = "host all all 0.0.0.0/0 md5" }
        "#;
        let cfg: Wrapper = toml::from_str(toml_in).expect("toml parse map content");
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        assert_eq!(
            cfg.hba.check_hba(ip, false, "md5", "alice", "app"),
            CheckResult::Allow
        );
        assert_eq!(
            cfg.hba.check_hba(ip, true, "md5", "alice", "app"),
            CheckResult::Allow
        );
    }

    #[test]
    fn serde_map_path() {
        // Create a temporary file with HBA content
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};
        let mut path = std::env::temp_dir();
        let uniq = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("pg_doorman_test_hba_{uniq}.conf"));
        let content = "host all all 192.168.0.0/16 scram-sha-256\n";
        fs::write(&path, content).expect("write temp hba");

        let toml_in = format!(r#"hba = {{ path = "{}" }}"#, path.display());
        let cfg: Wrapper = toml::from_str(&toml_in).expect("toml parse map path");
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));
        assert_eq!(
            cfg.hba.check_hba(ip, true, "scram-sha-256", "alice", "app"),
            CheckResult::Allow
        );

        // Best-effort cleanup
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn serde_map_missing_fields_error() {
        // Missing both path and content should error
        let toml_in = r#"hba = {}"#;
        let err = toml::from_str::<Wrapper>(toml_in).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("expected either 'path' or 'content' field"),
            "actual: {msg}"
        );
    }

    #[test]
    fn serde_map_unknown_field_error() {
        let toml_in = r#"hba = { foo = "bar" }"#;
        let err = toml::from_str::<Wrapper>(toml_in).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown field"), "actual: {msg}");
    }

    #[test]
    fn serde_both_path_and_content_prefers_content() {
        // When both are present, our implementation prefers `content`
        // (no error; resolved after visiting all keys)
        let toml_in = r#"
            hba = { path = "/non/existent/should/not/be/read", content = "host all all 0.0.0.0/0 md5" }
        "#;
        let cfg: Wrapper = toml::from_str(toml_in).expect("toml parse both fields");
        let ip = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(
            cfg.hba.check_hba(ip, false, "md5", "alice", "app"),
            CheckResult::Allow
        );
    }

    #[test]
    fn display_formats_hba() {
        let hba = PgHba::from_content(SAMPLE);
        let s = hba.to_string();
        let expected = "host all all 10.0.0.0/8 md5\nhostssl all all 192.168.0.0/16 scram-sha-256\nhostnossl all all 127.0.0.1/32 trust";
        assert_eq!(s, expected);
    }
}
