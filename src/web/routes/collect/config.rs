use std::collections::HashMap;

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

/// Bind-address fields require a restart; everything else takes effect on
/// the next backend or `RELOAD`. Listed precisely so the UI can render
/// the right "restart_required" pill instead of marking everything
/// reloadable.
const IMMUTABLES: &[&str] = &["host", "port", "connect_timeout"];

/// Flatten a serde JSON value into dotted keys → string values. Operators
/// have asked for a coverage-complete `/api/config` so they can verify
/// TLS / auth_query / pool sizing / prepared cache / web settings during
/// an incident — the previous hand-written `From<&Config>` only exposed
/// host/port/connect_timeout/idle_timeout/shutdown_timeout plus pool
/// users/mode, which DBA P3#7 (codex review) flagged as too thin.
fn flatten_json(prefix: &str, value: &serde_json::Value, out: &mut HashMap<String, String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_json(&key, v, out);
            }
        }
        serde_json::Value::Array(arr) => {
            // Render arrays as comma-joined when every element is a leaf,
            // otherwise as `prefix.<index>` rows. Operators reading
            // `pools.app_db.users` want one row, not five.
            if arr.iter().all(|v| {
                !matches!(
                    v,
                    serde_json::Value::Object(_) | serde_json::Value::Array(_)
                )
            }) {
                let joined: Vec<String> = arr.iter().map(json_leaf_to_string).collect();
                out.insert(prefix.to_string(), joined.join(", "));
            } else {
                for (i, v) in arr.iter().enumerate() {
                    let key = format!("{prefix}.{i}");
                    flatten_json(&key, v, out);
                }
            }
        }
        _ => {
            out.insert(prefix.to_string(), json_leaf_to_string(value));
        }
    }
}

fn json_leaf_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn collect_config() -> ConfigDto {
    let config = get_config();

    let mut flat: HashMap<String, String> = HashMap::new();
    if let Ok(value) = serde_json::to_value(&config) {
        flatten_json("", &value, &mut flat);
    }
    // Drop noisy bookkeeping fields that operators do not act on.
    flat.retain(|k, _| !is_internal_key(k));

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

/// Drop fields that exist on the in-memory `Config` purely as state
/// (file path, parsed include list) and have no "what is the pooler
/// running with" meaning for an operator.
fn is_internal_key(key: &str) -> bool {
    matches!(key, "path") || key.starts_with("include.") || key == "include"
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

    /// Coverage check: the previous implementation only exposed
    /// host/port/connect_timeout/idle_timeout/shutdown_timeout plus
    /// pool users/mode. The flattened serde view now surfaces every
    /// field in `Config` — verify a representative sample so a future
    /// refactor that quietly trims keys gets caught.
    #[test]
    fn collect_config_exposes_operationally_relevant_fields() {
        let dto = super::collect_config();
        let keys: std::collections::HashSet<&str> =
            dto.config.iter().map(|e| e.key.as_str()).collect();
        // Spot-check four orthogonal areas DBA P3#7 called out:
        // TLS server-side, prepared cache size, web listener, shutdown
        // timeout.
        assert!(keys.contains("general.host"), "{keys:?}");
        assert!(keys.contains("general.shutdown_timeout"), "{keys:?}");
        assert!(keys.contains("general.server_tls_mode"), "{keys:?}");
        assert!(keys.contains("web.enabled"), "{keys:?}");
    }

    #[test]
    fn collect_config_drops_internal_keys() {
        let dto = super::collect_config();
        for entry in &dto.config {
            assert!(
                !super::is_internal_key(&entry.key),
                "internal key leaked: {entry:?}"
            );
        }
    }
}
