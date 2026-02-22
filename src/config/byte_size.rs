//! ByteSize type with human-readable parsing support.
//!
//! Supports parsing from:
//! - Numbers (interpreted as bytes): `1048576`
//! - Strings with suffixes: `"1KB"`, `"1K"`, `"1MB"`, `"1M"`, `"1GB"`, `"1G"`
//!
//! This provides backward compatibility with existing numeric configurations
//! while allowing more readable string formats.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Byte size with human-readable parsing support.
///
/// # Supported formats
/// - Plain numbers: interpreted as bytes (e.g., `1048576` = 1 MB)
/// - `B` suffix: bytes (e.g., `"1024B"` = 1024 bytes)
/// - `K` or `KB` suffix: kilobytes (e.g., `"1K"` or `"1KB"` = 1024 bytes)
/// - `M` or `MB` suffix: megabytes (e.g., `"1M"` or `"1MB"` = 1048576 bytes)
/// - `G` or `GB` suffix: gigabytes (e.g., `"1G"` or `"1GB"` = 1073741824 bytes)
///
/// Note: Uses binary prefixes (1 KB = 1024 bytes, not 1000 bytes).
///
/// # Examples
/// ```yaml
/// max_memory_usage: 268435456    # 256 MB (backward compatible)
/// max_memory_usage: "256MB"      # 256 MB (human-readable)
/// max_memory_usage: "256M"       # 256 MB (short form)
/// unix_socket_buffer_size: "1MB" # 1 MB
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ByteSize(pub u64);

impl ByteSize {
    /// Creates a new ByteSize from bytes.
    pub const fn from_bytes(bytes: u64) -> Self {
        ByteSize(bytes)
    }

    /// Creates a new ByteSize from kilobytes.
    pub const fn from_kb(kb: u64) -> Self {
        ByteSize(kb * 1024)
    }

    /// Creates a new ByteSize from megabytes.
    pub const fn from_mb(mb: u64) -> Self {
        ByteSize(mb * 1024 * 1024)
    }

    /// Creates a new ByteSize from gigabytes.
    pub const fn from_gb(gb: u64) -> Self {
        ByteSize(gb * 1024 * 1024 * 1024)
    }

    /// Returns the size in bytes.
    pub const fn as_bytes(&self) -> u64 {
        self.0
    }

    /// Returns the size in bytes as usize.
    /// Panics if the value doesn't fit in usize on 32-bit platforms.
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }

    /// Returns the size in kilobytes (truncated).
    pub const fn as_kb(&self) -> u64 {
        self.0 / 1024
    }

    /// Returns the size in megabytes (truncated).
    pub const fn as_mb(&self) -> u64 {
        self.0 / (1024 * 1024)
    }

    /// Returns the size in gigabytes (truncated).
    pub const fn as_gb(&self) -> u64 {
        self.0 / (1024 * 1024 * 1024)
    }
}

impl From<u64> for ByteSize {
    fn from(bytes: u64) -> Self {
        ByteSize(bytes)
    }
}

impl From<usize> for ByteSize {
    fn from(bytes: usize) -> Self {
        ByteSize(bytes as u64)
    }
}

impl From<ByteSize> for u64 {
    fn from(b: ByteSize) -> Self {
        b.0
    }
}

impl From<ByteSize> for usize {
    fn from(b: ByteSize) -> Self {
        b.0 as usize
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ByteSizeVisitor;

        impl<'de> de::Visitor<'de> for ByteSizeVisitor {
            type Value = ByteSize;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a byte size like '5MB', '1G', '512K' or a number in bytes")
            }

            fn visit_u64<E>(self, v: u64) -> Result<ByteSize, E> {
                Ok(ByteSize(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<ByteSize, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(E::custom("byte size cannot be negative"));
                }
                Ok(ByteSize(v as u64))
            }

            fn visit_str<E>(self, s: &str) -> Result<ByteSize, E>
            where
                E: de::Error,
            {
                parse_byte_size(s).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(ByteSizeVisitor)
    }
}

impl Serialize for ByteSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as number for backward compatibility
        serializer.serialize_u64(self.0)
    }
}

/// Parse a byte size string into a ByteSize.
///
/// Supports:
/// - Plain numbers (bytes): "1048576"
/// - Bytes: "1024B"
/// - Kilobytes: "1K", "1KB"
/// - Megabytes: "1M", "1MB"
/// - Gigabytes: "1G", "1GB"
///
/// Case-insensitive (e.g., "1mb", "1MB", "1Mb" are all valid).
fn parse_byte_size(s: &str) -> Result<ByteSize, String> {
    let s = s.trim();

    // Try parsing as plain number first (backward compatibility)
    if let Ok(bytes) = s.parse::<u64>() {
        return Ok(ByteSize(bytes));
    }

    let s_lower = s.to_lowercase();

    // Parse with suffix (case-insensitive)
    // Order matters: check longer suffixes first
    let (num_str, multiplier) = if s_lower.ends_with("gb") {
        (&s[..s.len() - 2], 1024u64 * 1024 * 1024)
    } else if s_lower.ends_with('g') {
        (&s[..s.len() - 1], 1024u64 * 1024 * 1024)
    } else if s_lower.ends_with("mb") {
        (&s[..s.len() - 2], 1024u64 * 1024)
    } else if s_lower.ends_with('m') {
        (&s[..s.len() - 1], 1024u64 * 1024)
    } else if s_lower.ends_with("kb") {
        (&s[..s.len() - 2], 1024u64)
    } else if s_lower.ends_with('k') {
        (&s[..s.len() - 1], 1024u64)
    } else if s_lower.ends_with('b') {
        (&s[..s.len() - 1], 1u64)
    } else {
        return Err(format!(
            "invalid byte size format: '{s}'. Expected a number or a string with suffix (B, K, KB, M, MB, G, GB)"
        ));
    };

    let num: u64 = num_str
        .trim()
        .parse()
        .map_err(|_| format!("invalid number in byte size: '{num_str}'"))?;

    Ok(ByteSize(num * multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_byte_size_plain_numbers() {
        // Backward compatibility: plain numbers are bytes
        assert_eq!(parse_byte_size("0").unwrap(), ByteSize(0));
        assert_eq!(parse_byte_size("1024").unwrap(), ByteSize(1024));
        assert_eq!(parse_byte_size("1048576").unwrap(), ByteSize(1048576));
        assert_eq!(parse_byte_size("268435456").unwrap(), ByteSize(268435456));
    }

    #[test]
    fn test_parse_byte_size_bytes() {
        assert_eq!(parse_byte_size("1024B").unwrap(), ByteSize(1024));
        assert_eq!(parse_byte_size("1024b").unwrap(), ByteSize(1024)); // case insensitive
    }

    #[test]
    fn test_parse_byte_size_kilobytes() {
        assert_eq!(parse_byte_size("1K").unwrap(), ByteSize(1024));
        assert_eq!(parse_byte_size("1KB").unwrap(), ByteSize(1024));
        assert_eq!(parse_byte_size("1k").unwrap(), ByteSize(1024)); // case insensitive
        assert_eq!(parse_byte_size("1kb").unwrap(), ByteSize(1024)); // case insensitive
        assert_eq!(parse_byte_size("1Kb").unwrap(), ByteSize(1024)); // case insensitive
        assert_eq!(parse_byte_size("512K").unwrap(), ByteSize(512 * 1024));
    }

    #[test]
    fn test_parse_byte_size_megabytes() {
        assert_eq!(parse_byte_size("1M").unwrap(), ByteSize(1048576));
        assert_eq!(parse_byte_size("1MB").unwrap(), ByteSize(1048576));
        assert_eq!(parse_byte_size("1m").unwrap(), ByteSize(1048576)); // case insensitive
        assert_eq!(parse_byte_size("1mb").unwrap(), ByteSize(1048576)); // case insensitive
        assert_eq!(parse_byte_size("1Mb").unwrap(), ByteSize(1048576)); // case insensitive
        assert_eq!(
            parse_byte_size("256M").unwrap(),
            ByteSize(256 * 1024 * 1024)
        );
        assert_eq!(
            parse_byte_size("256MB").unwrap(),
            ByteSize(256 * 1024 * 1024)
        );
    }

    #[test]
    fn test_parse_byte_size_gigabytes() {
        assert_eq!(parse_byte_size("1G").unwrap(), ByteSize(1073741824));
        assert_eq!(parse_byte_size("1GB").unwrap(), ByteSize(1073741824));
        assert_eq!(parse_byte_size("1g").unwrap(), ByteSize(1073741824)); // case insensitive
        assert_eq!(parse_byte_size("1gb").unwrap(), ByteSize(1073741824)); // case insensitive
        assert_eq!(parse_byte_size("1Gb").unwrap(), ByteSize(1073741824)); // case insensitive
        assert_eq!(
            parse_byte_size("2G").unwrap(),
            ByteSize(2 * 1024 * 1024 * 1024)
        );
    }

    #[test]
    fn test_parse_byte_size_with_whitespace() {
        assert_eq!(parse_byte_size("  1MB  ").unwrap(), ByteSize(1048576));
        assert_eq!(parse_byte_size(" 1024 ").unwrap(), ByteSize(1024));
        assert_eq!(parse_byte_size("  1 K").unwrap(), ByteSize(1024));
    }

    #[test]
    fn test_parse_byte_size_invalid() {
        assert!(parse_byte_size("").is_err());
        assert!(parse_byte_size("abc").is_err());
        assert!(parse_byte_size("5x").is_err());
        assert!(parse_byte_size("-5MB").is_err());
        assert!(parse_byte_size("5.5MB").is_err()); // no float support
        assert!(parse_byte_size("5TB").is_err()); // TB not supported
    }

    #[test]
    fn test_byte_size_constructors() {
        assert_eq!(ByteSize::from_bytes(1024), ByteSize(1024));
        assert_eq!(ByteSize::from_kb(1), ByteSize(1024));
        assert_eq!(ByteSize::from_mb(1), ByteSize(1048576));
        assert_eq!(ByteSize::from_gb(1), ByteSize(1073741824));
    }

    #[test]
    fn test_byte_size_accessors() {
        let b = ByteSize(1048576); // 1 MB
        assert_eq!(b.as_bytes(), 1048576);
        assert_eq!(b.as_usize(), 1048576);
        assert_eq!(b.as_kb(), 1024);
        assert_eq!(b.as_mb(), 1);
        assert_eq!(b.as_gb(), 0); // truncated

        let b = ByteSize(1073741824); // 1 GB
        assert_eq!(b.as_gb(), 1);
    }

    #[test]
    fn test_byte_size_from_into() {
        let b: ByteSize = 1024u64.into();
        assert_eq!(b, ByteSize(1024));

        let b: ByteSize = 1024usize.into();
        assert_eq!(b, ByteSize(1024));

        let bytes: u64 = ByteSize(1024).into();
        assert_eq!(bytes, 1024);

        let bytes: usize = ByteSize(1024).into();
        assert_eq!(bytes, 1024);
    }

    #[test]
    fn test_byte_size_display() {
        assert_eq!(format!("{}", ByteSize(1048576)), "1048576");
    }

    #[test]
    fn test_deserialize_from_number() {
        // Test YAML deserialization from number
        let yaml = "1048576";
        let b: ByteSize = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(b, ByteSize(1048576));
    }

    #[test]
    fn test_deserialize_from_string() {
        // Test YAML deserialization from string
        let yaml = "\"1MB\"";
        let b: ByteSize = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(b, ByteSize(1048576));

        let yaml = "\"1M\"";
        let b: ByteSize = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(b, ByteSize(1048576));

        let yaml = "\"256MB\"";
        let b: ByteSize = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(b, ByteSize(256 * 1024 * 1024));

        let yaml = "\"1G\"";
        let b: ByteSize = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(b, ByteSize(1073741824));
    }

    #[test]
    fn test_deserialize_from_toml_number() {
        // Test TOML deserialization from number
        #[derive(Deserialize)]
        struct Config {
            size: ByteSize,
        }

        let toml = "size = 1048576";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.size, ByteSize(1048576));
    }

    #[test]
    fn test_deserialize_from_toml_string() {
        // Test TOML deserialization from string
        #[derive(Deserialize)]
        struct Config {
            size: ByteSize,
        }

        let toml = "size = \"1MB\"";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.size, ByteSize(1048576));
    }

    #[test]
    fn test_serialize() {
        // Serialization should produce a number for backward compatibility
        let b = ByteSize(1048576);
        let yaml = serde_yaml::to_string(&b).unwrap();
        assert_eq!(yaml.trim(), "1048576");

        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, "1048576");
    }

    #[test]
    fn test_deserialize_negative_error() {
        let yaml = "-1048576";
        let result: Result<ByteSize, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_real_world_values() {
        // Test values commonly used in pg_doorman config
        assert_eq!(parse_byte_size("256MB").unwrap(), ByteSize(268435456)); // max_memory_usage default
        assert_eq!(parse_byte_size("1MB").unwrap(), ByteSize(1048576)); // unix_socket_buffer_size default
        assert_eq!(parse_byte_size("8MB").unwrap(), ByteSize(8 * 1024 * 1024)); // worker_stack_size default
    }
}
