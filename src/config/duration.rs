//! Duration type with human-readable parsing support.
//!
//! Supports parsing from:
//! - Numbers (interpreted as milliseconds): `5000`
//! - Strings with suffixes: `"5ms"`, `"5s"`, `"5m"`, `"5h"`, `"5d"`
//!
//! This provides backward compatibility with existing numeric configurations
//! while allowing more readable string formats.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Duration in milliseconds with human-readable parsing support.
///
/// # Supported formats
/// - Plain numbers: interpreted as milliseconds (e.g., `5000` = 5 seconds)
/// - `ms` suffix: milliseconds (e.g., `"5ms"` = 5 milliseconds)
/// - `s` suffix: seconds (e.g., `"5s"` = 5000 milliseconds)
/// - `m` suffix: minutes (e.g., `"5m"` = 300000 milliseconds)
/// - `h` suffix: hours (e.g., `"1h"` = 3600000 milliseconds)
/// - `d` suffix: days (e.g., `"1d"` = 86400000 milliseconds)
///
/// # Examples
/// ```yaml
/// connect_timeout: 3000      # 3 seconds (backward compatible)
/// connect_timeout: "3s"      # 3 seconds (human-readable)
/// connect_timeout: "3000ms"  # 3 seconds (explicit milliseconds)
/// idle_timeout: "5m"         # 5 minutes
/// server_lifetime: "1h"      # 1 hour
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Duration(pub u64);

impl Duration {
    /// Creates a new Duration from milliseconds.
    pub const fn from_millis(ms: u64) -> Self {
        Duration(ms)
    }

    /// Creates a new Duration from seconds.
    pub const fn from_secs(secs: u64) -> Self {
        Duration(secs * 1000)
    }

    /// Creates a new Duration from minutes.
    pub const fn from_mins(mins: u64) -> Self {
        Duration(mins * 60 * 1000)
    }

    /// Creates a new Duration from hours.
    pub const fn from_hours(hours: u64) -> Self {
        Duration(hours * 60 * 60 * 1000)
    }

    /// Returns the duration in milliseconds.
    pub const fn as_millis(&self) -> u64 {
        self.0
    }

    /// Returns the duration in seconds (truncated).
    pub const fn as_secs(&self) -> u64 {
        self.0 / 1000
    }

    /// Converts to std::time::Duration.
    ///
    /// This is the preferred way to use Duration values with tokio and std APIs.
    ///
    /// # Example
    /// ```ignore
    /// let timeout = config.general.connect_timeout.as_std();
    /// tokio::time::sleep(timeout).await;
    /// ```
    pub const fn as_std(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.0)
    }
}

impl From<u64> for Duration {
    fn from(ms: u64) -> Self {
        Duration(ms)
    }
}

impl From<Duration> for u64 {
    fn from(d: Duration) -> Self {
        d.0
    }
}

impl From<Duration> for std::time::Duration {
    fn from(d: Duration) -> Self {
        std::time::Duration::from_millis(d.0)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DurationVisitor;

        impl<'de> de::Visitor<'de> for DurationVisitor {
            type Value = Duration;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a duration like '5s', '100ms', '1h', '30m', '1d' or a number in milliseconds",
                )
            }

            fn visit_u64<E>(self, v: u64) -> Result<Duration, E> {
                Ok(Duration(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Duration, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(E::custom("duration cannot be negative"));
                }
                Ok(Duration(v as u64))
            }

            fn visit_str<E>(self, s: &str) -> Result<Duration, E>
            where
                E: de::Error,
            {
                parse_duration(s).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

impl Serialize for Duration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as number for backward compatibility
        serializer.serialize_u64(self.0)
    }
}

/// Parse a duration string into a Duration.
///
/// Supports:
/// - Plain numbers (milliseconds): "5000"
/// - Milliseconds: "5ms"
/// - Seconds: "5s"
/// - Minutes: "5m"
/// - Hours: "5h"
/// - Days: "5d"
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();

    // Try parsing as plain number first (backward compatibility)
    if let Ok(ms) = s.parse::<u64>() {
        return Ok(Duration(ms));
    }

    let s_lower = s.to_lowercase();

    // Parse with suffix
    let (num_str, multiplier) = if s_lower.ends_with("ms") {
        (&s[..s.len() - 2], 1u64)
    } else if s_lower.ends_with('s') {
        (&s[..s.len() - 1], 1000u64)
    } else if s_lower.ends_with('m') {
        (&s[..s.len() - 1], 60 * 1000u64)
    } else if s_lower.ends_with('h') {
        (&s[..s.len() - 1], 60 * 60 * 1000u64)
    } else if s_lower.ends_with('d') {
        (&s[..s.len() - 1], 24 * 60 * 60 * 1000u64)
    } else {
        return Err(format!(
            "invalid duration format: '{s}'. Expected a number or a string with suffix (ms, s, m, h, d)"
        ));
    };

    let num: u64 = num_str
        .trim()
        .parse()
        .map_err(|_| format!("invalid number in duration: '{num_str}'"))?;

    Ok(Duration(num * multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_plain_numbers() {
        // Backward compatibility: plain numbers are milliseconds
        assert_eq!(parse_duration("0").unwrap(), Duration(0));
        assert_eq!(parse_duration("100").unwrap(), Duration(100));
        assert_eq!(parse_duration("5000").unwrap(), Duration(5000));
        assert_eq!(parse_duration("300000").unwrap(), Duration(300000));
    }

    #[test]
    fn test_parse_duration_milliseconds() {
        assert_eq!(parse_duration("5ms").unwrap(), Duration(5));
        assert_eq!(parse_duration("100ms").unwrap(), Duration(100));
        assert_eq!(parse_duration("5000ms").unwrap(), Duration(5000));
        assert_eq!(parse_duration("5MS").unwrap(), Duration(5)); // case insensitive
        assert_eq!(parse_duration("5Ms").unwrap(), Duration(5)); // case insensitive
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("1s").unwrap(), Duration(1000));
        assert_eq!(parse_duration("5s").unwrap(), Duration(5000));
        assert_eq!(parse_duration("60s").unwrap(), Duration(60000));
        assert_eq!(parse_duration("5S").unwrap(), Duration(5000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("1m").unwrap(), Duration(60000));
        assert_eq!(parse_duration("5m").unwrap(), Duration(300000));
        assert_eq!(parse_duration("60m").unwrap(), Duration(3600000));
        assert_eq!(parse_duration("5M").unwrap(), Duration(300000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h").unwrap(), Duration(3600000));
        assert_eq!(parse_duration("5h").unwrap(), Duration(18000000));
        assert_eq!(parse_duration("24h").unwrap(), Duration(86400000));
        assert_eq!(parse_duration("5H").unwrap(), Duration(18000000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_days() {
        assert_eq!(parse_duration("1d").unwrap(), Duration(86400000));
        assert_eq!(parse_duration("7d").unwrap(), Duration(604800000));
        assert_eq!(parse_duration("1D").unwrap(), Duration(86400000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_with_whitespace() {
        assert_eq!(parse_duration("  5s  ").unwrap(), Duration(5000));
        assert_eq!(parse_duration(" 100 ").unwrap(), Duration(100));
        assert_eq!(parse_duration("  5 ms").unwrap(), Duration(5));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("-5s").is_err());
        assert!(parse_duration("5.5s").is_err()); // no float support
    }

    #[test]
    fn test_duration_constructors() {
        assert_eq!(Duration::from_millis(5000), Duration(5000));
        assert_eq!(Duration::from_secs(5), Duration(5000));
        assert_eq!(Duration::from_mins(5), Duration(300000));
        assert_eq!(Duration::from_hours(1), Duration(3600000));
    }

    #[test]
    fn test_duration_accessors() {
        let d = Duration(5500);
        assert_eq!(d.as_millis(), 5500);
        assert_eq!(d.as_secs(), 5); // truncated
    }

    #[test]
    fn test_duration_as_std() {
        let d = Duration(5000);
        let std_duration = d.as_std();
        assert_eq!(std_duration, std::time::Duration::from_millis(5000));
        assert_eq!(std_duration.as_millis(), 5000);

        // Test conversion via From trait
        let std_duration2: std::time::Duration = d.into();
        assert_eq!(std_duration2, std::time::Duration::from_millis(5000));
    }

    #[test]
    fn test_duration_from_into() {
        let d: Duration = 5000u64.into();
        assert_eq!(d, Duration(5000));

        let ms: u64 = d.into();
        assert_eq!(ms, 5000);
    }

    #[test]
    fn test_duration_display() {
        assert_eq!(format!("{}", Duration(5000)), "5000");
    }

    #[test]
    fn test_deserialize_from_number() {
        // Test YAML deserialization from number
        let yaml = "5000";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(5000));
    }

    #[test]
    fn test_deserialize_from_string() {
        // Test YAML deserialization from string
        let yaml = "\"5s\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(5000));

        let yaml = "\"100ms\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(100));

        let yaml = "\"5m\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(300000));
    }

    #[test]
    fn test_deserialize_from_toml_number() {
        // Test TOML deserialization from number
        #[derive(Deserialize)]
        struct Config {
            timeout: Duration,
        }

        let toml = "timeout = 5000";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.timeout, Duration(5000));
    }

    #[test]
    fn test_deserialize_from_toml_string() {
        // Test TOML deserialization from string
        #[derive(Deserialize)]
        struct Config {
            timeout: Duration,
        }

        let toml = "timeout = \"5s\"";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.timeout, Duration(5000));
    }

    #[test]
    fn test_serialize() {
        // Serialization should produce a number for backward compatibility
        let d = Duration(5000);
        let yaml = serde_yaml::to_string(&d).unwrap();
        assert_eq!(yaml.trim(), "5000");

        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "5000");
    }

    #[test]
    fn test_deserialize_negative_error() {
        let yaml = "-5000";
        let result: Result<Duration, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }
}
