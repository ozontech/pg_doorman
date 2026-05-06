use crate::config::get_config;
use crate::web::routes::dto::{ConfigDto, ConfigEntry};

use super::now_unix_ms;

/// Returns `true` for configuration keys whose value should be masked in
/// `/api/config`. A key is secret if its trailing path segment (after the
/// last `.`) is exactly `password` or `secret`, or has any of the suffixes
/// `_password`, `_secret`, `_token`, `_key`.
///
/// The trailing-segment matching is so that `pools.foo.users.bar.password`
/// is recognised as secret, not just top-level `password`.
fn is_secret_key(key: &str) -> bool {
    let last_segment = key.rsplit('.').next().unwrap_or(key);
    matches!(last_segment, "password" | "secret")
        || last_segment.ends_with("_password")
        || last_segment.ends_with("_secret")
        || last_segment.ends_with("_token")
        || last_segment.ends_with("_key")
}

pub(crate) fn collect_config() -> ConfigDto {
    // Mirrors `show_config` in src/admin/show.rs:429 for the immutables list
    // (these are the only fields that require a restart to change).
    const IMMUTABLES: &[&str] = &["host", "port", "connect_timeout"];

    let config = get_config();
    let flat: std::collections::HashMap<String, String> = (&config).into();

    let mut entries: Vec<ConfigEntry> = flat
        .into_iter()
        .map(|(key, value)| {
            let value = if is_secret_key(&key) {
                "***".to_string()
            } else {
                value
            };
            let changeable = if IMMUTABLES.iter().any(|c| *c == key) {
                "no"
            } else {
                "yes"
            };
            ConfigEntry {
                key,
                value,
                default: "-",
                changeable,
            }
        })
        .collect();

    entries.sort_by(|a, b| a.key.cmp(&b.key));

    ConfigDto {
        ts: now_unix_ms(),
        config: entries,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn is_secret_key_top_level_password() {
        assert!(super::is_secret_key("password"));
        assert!(super::is_secret_key("admin_password"));
        assert!(super::is_secret_key("server_password"));
    }

    #[test]
    fn is_secret_key_top_level_secret() {
        assert!(super::is_secret_key("secret"));
        assert!(super::is_secret_key("talos_jwt_secret"));
    }

    #[test]
    fn is_secret_key_token_and_key_suffixes() {
        assert!(super::is_secret_key("api_token"));
        assert!(super::is_secret_key("private_key"));
    }

    #[test]
    fn is_secret_key_nested_password_path() {
        assert!(super::is_secret_key("pools.main.users.alice.password"));
        assert!(super::is_secret_key("users.app.api_token"));
    }

    #[test]
    fn is_secret_key_does_not_match_unrelated_keys() {
        assert!(!super::is_secret_key("host"));
        assert!(!super::is_secret_key("port"));
        assert!(!super::is_secret_key("connect_timeout"));
        assert!(!super::is_secret_key("pool_mode"));
        assert!(!super::is_secret_key("max_connections"));
    }

    #[test]
    fn is_secret_key_does_not_match_partial_substring() {
        // Substring "password" elsewhere in the key should not trigger masking.
        // Only exact equals or exact suffix counts.
        assert!(!super::is_secret_key("password_check_attempts"));
        assert!(!super::is_secret_key("not_a_secret_check"));
    }
}
