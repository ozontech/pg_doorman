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

/// Returns `true` for keys that live inside a `startup_parameters`
/// cascade (`general.startup_parameters.<param>` or
/// `pools.<name>.startup_parameters.<param>`). These values are
/// operator-supplied and may carry tenant identifiers, audit routing
/// tags, or accidental secrets - the same redaction contract that
/// applies to `/api/pools` for anonymous viewers also applies here.
fn is_startup_parameter_key(key: &str) -> bool {
    key.contains(".startup_parameters.") || key.starts_with("startup_parameters.")
}

/// Listener-bind and runtime-construction fields require a restart;
/// everything else takes effect on the next backend or `RELOAD`. Listed
/// as full flattened keys so the UI renders the right "restart_required"
/// pill — the previous bare `["host", "port", "connect_timeout"]` table
/// never matched against flattened keys like `general.host` or
/// `web.host`, so nothing was marked immutable (codex review MED #5).
/// `worker_threads`, `unix_socket_dir`, and `backlog` shape the tokio
/// runtime and the listener socket at process start; a SIGHUP cannot
/// rebuild those.
const IMMUTABLES: &[&str] = &[
    "general.host",
    "general.port",
    "general.worker_threads",
    "general.unix_socket_dir",
    "general.backlog",
    "web.host",
    "web.port",
];

fn is_immutable_key(key: &str) -> bool {
    IMMUTABLES.contains(&key)
}

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

pub(crate) fn collect_config(reveal_startup_values: bool) -> ConfigDto {
    let config = get_config();

    let mut flat: HashMap<String, String> = HashMap::new();
    if let Ok(value) = serde_json::to_value(&config) {
        flatten_json("", &value, &mut flat);
    }
    flat.retain(|k, _| !is_internal_key(k));

    // Diff against `Config::default()` so the UI can show what is at
    // its built-in default vs. what an operator changed via the config
    // file. The defaults map is computed once per request — cheap, and
    // a stable comparison surface for codex DBA P3#7.
    let mut defaults: HashMap<String, String> = HashMap::new();
    if let Ok(value) = serde_json::to_value(crate::config::Config::default()) {
        flatten_json("", &value, &mut defaults);
    }

    let mut entries: Vec<ConfigEntry> = flat
        .into_iter()
        .map(|(key, value)| {
            let secret = is_secret_key(&key);
            let startup_redact = !reveal_startup_values && is_startup_parameter_key(&key);
            let mask = secret || startup_redact;
            let value = if mask { "***".to_string() } else { value };
            let default = defaults
                .get(&key)
                .map(|d| if mask { "***".to_string() } else { d.clone() })
                .unwrap_or_else(|| "-".to_string());
            let changeable = if is_immutable_key(&key) { "no" } else { "yes" };
            let doc = lookup_doc(&key);
            ConfigEntry {
                key,
                value,
                default,
                changeable,
                doc,
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

/// Map a flattened config key to (section, field-name) suitable for
/// `FieldsData::try_field`. Returns `None` for nested keys we have no
/// doc for (e.g. the integers inside a pools.<name>.users array).
fn key_to_section_field(key: &str) -> Option<(&'static str, &str)> {
    if let Some(rest) = key.strip_prefix("general.") {
        return Some(("general", rest));
    }
    if let Some(rest) = key.strip_prefix("web.") {
        return Some(("web", rest));
    }
    if let Some(rest) = key.strip_prefix("pools.") {
        // pools.<name>.users.<i>.<field>  → ("user", <field>)
        // pools.<name>.auth_query.<field> → ("auth_query", <field>)
        // pools.<name>.<field>            → ("pool", <field>)
        let (_name, tail) = rest.split_once('.')?;
        if let Some(user_field) = tail.strip_prefix("users.") {
            // skip past the index
            let (_idx, f) = user_field.split_once('.')?;
            return Some(("user", f));
        }
        if let Some(aq_field) = tail.strip_prefix("auth_query.") {
            return Some(("auth_query", aq_field));
        }
        return Some(("pool", tail));
    }
    None
}

/// EN documentation string for a config key, or empty if not in fields.yaml.
fn lookup_doc(key: &str) -> String {
    let Some((section, field)) = key_to_section_field(key) else {
        return String::new();
    };
    crate::app::generate::annotated::FIELDS
        .try_field(section, field)
        .and_then(|f| f.config.as_ref())
        .map(|i18n| i18n.get(false).trim().to_string())
        .unwrap_or_default()
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

    #[test]
    fn is_immutable_key_matches_bind_addresses() {
        assert!(super::is_immutable_key("general.host"));
        assert!(super::is_immutable_key("general.port"));
        assert!(super::is_immutable_key("web.host"));
        assert!(super::is_immutable_key("web.port"));
    }

    #[test]
    fn is_immutable_key_matches_runtime_construction_fields() {
        assert!(super::is_immutable_key("general.worker_threads"));
        assert!(super::is_immutable_key("general.unix_socket_dir"));
        assert!(super::is_immutable_key("general.backlog"));
    }

    #[test]
    fn is_immutable_key_rejects_reloadable_fields() {
        assert!(!super::is_immutable_key("general.idle_timeout"));
        assert!(!super::is_immutable_key("general.shutdown_timeout"));
        assert!(!super::is_immutable_key("general.connect_timeout"));
        assert!(!super::is_immutable_key("pools.app_db.server_host"));
        assert!(!super::is_immutable_key("pools.app_db.users.0.username"));
        // Bare segment must not match a flattened key.
        assert!(!super::is_immutable_key("host"));
        assert!(!super::is_immutable_key("port"));
    }

    /// Coverage check: the previous implementation only exposed
    /// host/port/connect_timeout/idle_timeout/shutdown_timeout plus
    /// pool users/mode. The flattened serde view now surfaces every
    /// field in `Config` — verify a representative sample so a future
    /// refactor that quietly trims keys gets caught.
    #[test]
    fn collect_config_exposes_operationally_relevant_fields() {
        let dto = super::collect_config(true);
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
        let dto = super::collect_config(true);
        for entry in &dto.config {
            assert!(
                !super::is_internal_key(&entry.key),
                "internal key leaked: {entry:?}"
            );
        }
    }
}
