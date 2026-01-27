//! Duration type with human-readable parsing support.
//!
//! Supports parsing from:
//! - Numbers (interpreted as milliseconds for backward compatibility): `5000`
//! - Strings with suffixes: `"5us"`, `"5ms"`, `"0.1ms"`, `"5s"`, `"5m"`, `"5h"`, `"5d"`
//!
//! This provides backward compatibility with existing numeric configurations
//! while allowing more readable string formats including sub-millisecond precision.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Duration in microseconds with human-readable parsing support.
///
/// Internally stores microseconds to support sub-millisecond precision.
///
/// # Supported formats
/// - Plain numbers: interpreted as milliseconds for backward compatibility (e.g., `5000` = 5 seconds)
/// - `us` suffix: microseconds (e.g., `"100us"` = 100 microseconds)
/// - `ms` suffix: milliseconds, supports decimals (e.g., `"5ms"`, `"0.1ms"` = 100 microseconds)
/// - `s` suffix: seconds (e.g., `"5s"` = 5000 milliseconds)
/// - `m` suffix: minutes (e.g., `"5m"` = 300000 milliseconds)
/// - `h` suffix: hours (e.g., `"1h"` = 3600000 milliseconds)
/// - `d` suffix: days (e.g., `"1d"` = 86400000 milliseconds)
///
/// # Examples
/// ```yaml
/// connect_timeout: 3000        # 3 seconds (backward compatible, interpreted as ms)
/// connect_timeout: "3s"        # 3 seconds (human-readable)
/// connect_timeout: "3000ms"    # 3 seconds (explicit milliseconds)
/// clock_resolution: "0.1ms"    # 100 microseconds (sub-millisecond precision)
/// clock_resolution: "100us"    # 100 microseconds (explicit microseconds)
/// idle_timeout: "5m"           # 5 minutes
/// server_lifetime: "1h"        # 1 hour
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Duration(pub u64);

impl Duration {
    /// Creates a new Duration from microseconds.
    pub const fn from_micros(us: u64) -> Self {
        Duration(us)
    }

    /// Creates a new Duration from milliseconds.
    pub const fn from_millis(ms: u64) -> Self {
        Duration(ms * 1000)
    }

    /// Creates a new Duration from seconds.
    pub const fn from_secs(secs: u64) -> Self {
        Duration(secs * 1_000_000)
    }

    /// Creates a new Duration from minutes.
    pub const fn from_mins(mins: u64) -> Self {
        Duration(mins * 60 * 1_000_000)
    }

    /// Creates a new Duration from hours.
    pub const fn from_hours(hours: u64) -> Self {
        Duration(hours * 60 * 60 * 1_000_000)
    }

    /// Returns the duration in microseconds.
    pub const fn as_micros(&self) -> u64 {
        self.0
    }

    /// Returns the duration in milliseconds (truncated).
    pub const fn as_millis(&self) -> u64 {
        self.0 / 1000
    }

    /// Returns the duration in seconds (truncated).
    pub const fn as_secs(&self) -> u64 {
        self.0 / 1_000_000
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
        std::time::Duration::from_micros(self.0)
    }
}

impl From<u64> for Duration {
    /// Creates Duration from milliseconds (for backward compatibility).
    fn from(ms: u64) -> Self {
        Duration(ms * 1000)
    }
}

impl From<Duration> for u64 {
    /// Returns Duration in milliseconds (for backward compatibility).
    fn from(d: Duration) -> Self {
        d.0 / 1000
    }
}

impl From<Duration> for std::time::Duration {
    fn from(d: Duration) -> Self {
        std::time::Duration::from_micros(d.0)
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
                    "a duration like '5s', '100ms', '0.1ms', '100us', '1h', '30m', '1d' or a number in milliseconds",
                )
            }

            fn visit_u64<E>(self, v: u64) -> Result<Duration, E> {
                // Plain numbers are interpreted as milliseconds for backward compatibility
                // Convert to microseconds (internal representation)
                Ok(Duration(v * 1000))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Duration, E>
            where
                E: de::Error,
            {
                if v < 0 {
                    return Err(E::custom("duration cannot be negative"));
                }
                // Plain numbers are interpreted as milliseconds for backward compatibility
                // Convert to microseconds (internal representation)
                Ok(Duration(v as u64 * 1000))
            }

            fn visit_f64<E>(self, v: f64) -> Result<Duration, E>
            where
                E: de::Error,
            {
                if v < 0.0 {
                    return Err(E::custom("duration cannot be negative"));
                }
                // Plain numbers are interpreted as milliseconds for backward compatibility
                // Convert to microseconds (internal representation)
                Ok(Duration((v * 1000.0) as u64))
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
        // Serialize as milliseconds for backward compatibility
        serializer.serialize_u64(self.0 / 1000)
    }
}

/// Parse a duration string into a Duration (stored in microseconds).
///
/// Supports:
/// - Plain numbers (milliseconds for backward compatibility): "5000" = 5 seconds
/// - Microseconds: "100us" = 100 microseconds
/// - Milliseconds (supports decimals): "5ms", "0.1ms" = 100 microseconds
/// - Seconds: "5s" = 5,000,000 microseconds
/// - Minutes: "5m" = 300,000,000 microseconds
/// - Hours: "5h"
/// - Days: "5d"
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();

    // Try parsing as plain number first (backward compatibility - milliseconds)
    if let Ok(ms) = s.parse::<u64>() {
        return Ok(Duration(ms * 1000)); // Convert ms to us
    }

    // Try parsing as float for backward compatibility with decimal milliseconds
    if let Ok(ms) = s.parse::<f64>() {
        if ms < 0.0 {
            return Err("duration cannot be negative".to_string());
        }
        return Ok(Duration((ms * 1000.0) as u64)); // Convert ms to us
    }

    let s_lower = s.to_lowercase();

    // Parse with suffix - multipliers are in microseconds
    let (num_str, multiplier) = if s_lower.ends_with("us") {
        (&s[..s.len() - 2], 1u64) // microseconds
    } else if s_lower.ends_with("ms") {
        (&s[..s.len() - 2], 1000u64) // milliseconds -> microseconds
    } else if s_lower.ends_with('s') {
        (&s[..s.len() - 1], 1_000_000u64) // seconds -> microseconds
    } else if s_lower.ends_with('m') {
        (&s[..s.len() - 1], 60 * 1_000_000u64) // minutes -> microseconds
    } else if s_lower.ends_with('h') {
        (&s[..s.len() - 1], 60 * 60 * 1_000_000u64) // hours -> microseconds
    } else if s_lower.ends_with('d') {
        (&s[..s.len() - 1], 24 * 60 * 60 * 1_000_000u64) // days -> microseconds
    } else {
        return Err(format!(
            "invalid duration format: '{}'. Expected a number or a string with suffix (us, ms, s, m, h, d)",
            s
        ));
    };

    let num_str = num_str.trim();

    // Try parsing as integer first
    if let Ok(num) = num_str.parse::<u64>() {
        return Ok(Duration(num * multiplier));
    }

    // Try parsing as float for decimal support (e.g., "0.1ms")
    let num: f64 = num_str
        .parse()
        .map_err(|_| format!("invalid number in duration: '{}'", num_str))?;

    if num < 0.0 {
        return Err("duration cannot be negative".to_string());
    }

    Ok(Duration((num * multiplier as f64) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    // All Duration values are now stored in MICROSECONDS internally
    // 1 ms = 1,000 us
    // 1 s = 1,000,000 us
    // 1 m = 60,000,000 us
    // 1 h = 3,600,000,000 us
    // 1 d = 86,400,000,000 us

    #[test]
    fn test_parse_duration_plain_numbers() {
        // Backward compatibility: plain numbers are milliseconds, converted to microseconds
        assert_eq!(parse_duration("0").unwrap(), Duration(0));
        assert_eq!(parse_duration("100").unwrap(), Duration(100_000)); // 100ms = 100,000us
        assert_eq!(parse_duration("5000").unwrap(), Duration(5_000_000)); // 5000ms = 5,000,000us
        assert_eq!(parse_duration("300000").unwrap(), Duration(300_000_000)); // 300000ms = 300,000,000us
    }

    #[test]
    fn test_parse_duration_microseconds() {
        // New: microseconds suffix
        assert_eq!(parse_duration("100us").unwrap(), Duration(100));
        assert_eq!(parse_duration("1000us").unwrap(), Duration(1000));
        assert_eq!(parse_duration("100US").unwrap(), Duration(100)); // case insensitive
        assert_eq!(parse_duration("100Us").unwrap(), Duration(100)); // case insensitive
    }

    #[test]
    fn test_parse_duration_milliseconds() {
        // Milliseconds converted to microseconds
        assert_eq!(parse_duration("5ms").unwrap(), Duration(5_000)); // 5ms = 5,000us
        assert_eq!(parse_duration("100ms").unwrap(), Duration(100_000)); // 100ms = 100,000us
        assert_eq!(parse_duration("5000ms").unwrap(), Duration(5_000_000)); // 5000ms = 5,000,000us
        assert_eq!(parse_duration("5MS").unwrap(), Duration(5_000)); // case insensitive
        assert_eq!(parse_duration("5Ms").unwrap(), Duration(5_000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_milliseconds_decimal() {
        // New: decimal milliseconds for sub-millisecond precision
        assert_eq!(parse_duration("0.1ms").unwrap(), Duration(100)); // 0.1ms = 100us
        assert_eq!(parse_duration("0.5ms").unwrap(), Duration(500)); // 0.5ms = 500us
        assert_eq!(parse_duration("1.5ms").unwrap(), Duration(1500)); // 1.5ms = 1,500us
        assert_eq!(parse_duration("0.001ms").unwrap(), Duration(1)); // 0.001ms = 1us
    }

    #[test]
    fn test_parse_duration_seconds() {
        // Seconds converted to microseconds
        assert_eq!(parse_duration("1s").unwrap(), Duration(1_000_000)); // 1s = 1,000,000us
        assert_eq!(parse_duration("5s").unwrap(), Duration(5_000_000)); // 5s = 5,000,000us
        assert_eq!(parse_duration("60s").unwrap(), Duration(60_000_000)); // 60s = 60,000,000us
        assert_eq!(parse_duration("5S").unwrap(), Duration(5_000_000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_minutes() {
        // Minutes converted to microseconds
        assert_eq!(parse_duration("1m").unwrap(), Duration(60_000_000)); // 1m = 60,000,000us
        assert_eq!(parse_duration("5m").unwrap(), Duration(300_000_000)); // 5m = 300,000,000us
        assert_eq!(parse_duration("60m").unwrap(), Duration(3_600_000_000)); // 60m = 3,600,000,000us
        assert_eq!(parse_duration("5M").unwrap(), Duration(300_000_000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_hours() {
        // Hours converted to microseconds
        assert_eq!(parse_duration("1h").unwrap(), Duration(3_600_000_000)); // 1h = 3,600,000,000us
        assert_eq!(parse_duration("5h").unwrap(), Duration(18_000_000_000)); // 5h = 18,000,000,000us
        assert_eq!(parse_duration("24h").unwrap(), Duration(86_400_000_000)); // 24h = 86,400,000,000us
        assert_eq!(parse_duration("5H").unwrap(), Duration(18_000_000_000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_days() {
        // Days converted to microseconds
        assert_eq!(parse_duration("1d").unwrap(), Duration(86_400_000_000)); // 1d = 86,400,000,000us
        assert_eq!(parse_duration("7d").unwrap(), Duration(604_800_000_000)); // 7d = 604,800,000,000us
        assert_eq!(parse_duration("1D").unwrap(), Duration(86_400_000_000)); // case insensitive
    }

    #[test]
    fn test_parse_duration_with_whitespace() {
        assert_eq!(parse_duration("  5s  ").unwrap(), Duration(5_000_000));
        assert_eq!(parse_duration(" 100 ").unwrap(), Duration(100_000)); // 100ms = 100,000us
        assert_eq!(parse_duration("  5 ms").unwrap(), Duration(5_000));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("-5s").is_err());
        assert!(parse_duration("-0.1ms").is_err());
    }

    #[test]
    fn test_duration_constructors() {
        // All constructors convert to microseconds
        assert_eq!(Duration::from_micros(100), Duration(100));
        assert_eq!(Duration::from_millis(5), Duration(5_000)); // 5ms = 5,000us
        assert_eq!(Duration::from_secs(5), Duration(5_000_000)); // 5s = 5,000,000us
        assert_eq!(Duration::from_mins(5), Duration(300_000_000)); // 5m = 300,000,000us
        assert_eq!(Duration::from_hours(1), Duration(3_600_000_000)); // 1h = 3,600,000,000us
    }

    #[test]
    fn test_duration_accessors() {
        let d = Duration(5_500_000); // 5,500,000us = 5500ms = 5.5s
        assert_eq!(d.as_micros(), 5_500_000);
        assert_eq!(d.as_millis(), 5500); // truncated
        assert_eq!(d.as_secs(), 5); // truncated
    }

    #[test]
    fn test_duration_as_std() {
        let d = Duration(5_000_000); // 5,000,000us = 5s
        let std_duration = d.as_std();
        assert_eq!(std_duration, std::time::Duration::from_micros(5_000_000));
        assert_eq!(std_duration.as_millis(), 5000);

        // Test conversion via From trait
        let std_duration2: std::time::Duration = d.into();
        assert_eq!(std_duration2, std::time::Duration::from_micros(5_000_000));
    }

    #[test]
    fn test_duration_as_std_submillisecond() {
        // Test sub-millisecond precision
        let d = Duration(100); // 100us = 0.1ms
        let std_duration = d.as_std();
        assert_eq!(std_duration, std::time::Duration::from_micros(100));
        assert_eq!(std_duration.as_micros(), 100);
    }

    #[test]
    fn test_duration_from_into() {
        // From<u64> interprets as milliseconds for backward compatibility
        let d: Duration = 5000u64.into();
        assert_eq!(d, Duration(5_000_000)); // 5000ms = 5,000,000us

        // Into<u64> returns milliseconds for backward compatibility
        let ms: u64 = d.into();
        assert_eq!(ms, 5000);
    }

    #[test]
    fn test_duration_display() {
        // Display shows internal microseconds value
        assert_eq!(format!("{}", Duration(5_000_000)), "5000000");
    }

    #[test]
    fn test_deserialize_from_number() {
        // Test YAML deserialization from number (interpreted as milliseconds)
        let yaml = "5000";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(5_000_000)); // 5000ms = 5,000,000us
    }

    #[test]
    fn test_deserialize_from_string() {
        // Test YAML deserialization from string
        let yaml = "\"5s\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(5_000_000)); // 5s = 5,000,000us

        let yaml = "\"100ms\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(100_000)); // 100ms = 100,000us

        let yaml = "\"5m\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(300_000_000)); // 5m = 300,000,000us

        // New: microseconds
        let yaml = "\"100us\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(100)); // 100us

        // New: decimal milliseconds
        let yaml = "\"0.1ms\"";
        let d: Duration = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(d, Duration(100)); // 0.1ms = 100us
    }

    #[test]
    fn test_deserialize_from_toml_number() {
        // Test TOML deserialization from number (interpreted as milliseconds)
        #[derive(Deserialize)]
        struct Config {
            timeout: Duration,
        }

        let toml = "timeout = 5000";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.timeout, Duration(5_000_000)); // 5000ms = 5,000,000us
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
        assert_eq!(config.timeout, Duration(5_000_000)); // 5s = 5,000,000us

        let toml = "timeout = \"0.1ms\"";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.timeout, Duration(100)); // 0.1ms = 100us
    }

    #[test]
    fn test_serialize() {
        // Serialization should produce milliseconds for backward compatibility
        let d = Duration(5_000_000); // 5,000,000us = 5000ms
        let yaml = serde_yaml::to_string(&d).unwrap();
        assert_eq!(yaml.trim(), "5000"); // serialized as milliseconds

        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "5000"); // serialized as milliseconds
    }

    #[test]
    fn test_serialize_submillisecond() {
        // Sub-millisecond values serialize to 0 (truncated to milliseconds)
        let d = Duration(100); // 100us = 0.1ms
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "0"); // 100us / 1000 = 0ms (truncated)
    }

    #[test]
    fn test_deserialize_negative_error() {
        let yaml = "-5000";
        let result: Result<Duration, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }
}
