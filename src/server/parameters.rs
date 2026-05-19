use bytes::{BufMut, BytesMut};
use once_cell::sync::Lazy;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::config::VERSION;

static TRACKED_PARAMETERS: Lazy<HashSet<String>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("client_encoding".to_string());
    set.insert("DateStyle".to_string());
    set.insert("IntervalStyle".to_string());
    set.insert("TimeZone".to_string());
    set.insert("standard_conforming_strings".to_string());
    set.insert("application_name".to_string());
    set
});

/// Canonicalise a PostgreSQL session parameter name. PG GUC lookups are
/// case-insensitive, so pg_doorman needs one normalised form per name
/// for every internal compare-by-key path (operator_managed key set,
/// cascade merge, dynamic-pool overlay hash, admin/Web read model). The
/// rule:
///
/// * Tracked parameters (`TRACKED_PARAMETERS`) return their fixed
///   spelling — the same casing PG reports back in `ParameterStatus`.
///   This keeps `sync_parameters` aligned with what the client expects
///   to see at the wire.
/// * Every other GUC is folded to ASCII lower case. PG itself accepts
///   any casing, but the cascade and admin views need a stable form so
///   `general.work_mem` plus `pool.Work_Mem` collapse to one entry
///   instead of shipping both rows in `StartupMessage`.
pub fn canonicalize_param_name(key: String) -> String {
    for tracked in TRACKED_PARAMETERS.iter() {
        if key.eq_ignore_ascii_case(tracked) {
            return tracked.clone();
        }
    }
    if key.chars().any(|c| c.is_ascii_uppercase()) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

#[derive(Debug, Clone)]
pub struct ServerParameters {
    // Kept `pub(crate)` to preserve current internal usage patterns during refactor.
    pub(crate) parameters: HashMap<String, String>,
}

impl Default for ServerParameters {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerParameters {
    pub fn new() -> Self {
        ServerParameters {
            parameters: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
    }

    pub fn admin() -> Self {
        let mut server_parameters = ServerParameters {
            parameters: HashMap::new(),
        };

        server_parameters.set_param("client_encoding", "UTF8", false);
        server_parameters.set_param("DateStyle", "ISO, MDY", false);
        server_parameters.set_param("TimeZone", "Etc/UTC", false);
        server_parameters.set_param("server_version", VERSION, true);
        server_parameters.set_param("server_encoding", "UTF-8", true);
        server_parameters.set_param("standard_conforming_strings", "on", false);
        // (64 bit = on) as of PostgreSQL 10, this is always on.
        server_parameters.set_param("integer_datetimes", "on", false);
        server_parameters.set_param("application_name", "pg_doorman", false);

        server_parameters
    }

    /// If `startup` is false, then only tracked parameters will be set.
    pub fn set_param(&mut self, key: impl Into<String>, value: impl Into<String>, startup: bool) {
        let key = canonicalize_param_name(key.into());
        let value = value.into();

        if TRACKED_PARAMETERS.contains(&key) || startup {
            self.parameters.insert(key, value);
        }
    }

    pub fn set_from_hashmap(&mut self, parameters: &HashMap<String, String>, startup: bool) {
        for (key, value) in parameters {
            self.set_param(key, value, startup);
        }
    }

    #[inline(always)]
    pub(crate) fn compare_params(
        &self,
        incoming_parameters: &ServerParameters,
    ) -> HashMap<String, String> {
        let mut diff = HashMap::new();

        for key in TRACKED_PARAMETERS.iter() {
            if let Some(incoming_value) = incoming_parameters.parameters.get(key) {
                if let Some(value) = self.parameters.get(key) {
                    if value != incoming_value {
                        diff.insert(key.to_string(), incoming_value.to_string());
                    }
                }
            }
        }

        diff
    }

    pub fn get_application_name(&self) -> &String {
        // Can unwrap because we set it in the constructor.
        self.parameters.get("application_name").unwrap()
    }

    pub fn as_hashmap(&self) -> HashMap<String, String> {
        self.parameters.clone()
    }

    /// Sorted snapshot of every session parameter pg_doorman has observed
    /// for this client — startup packet + every `SET` it has issued.
    /// Used as part of the prepared-statement cache key so two clients
    /// of the same `user@db` pool with different `search_path` /
    /// `DateStyle` / `TimeZone` etc. do not collapse onto one server-
    /// side prepared statement (whose plan was built for the first
    /// client's GUC state).
    ///
    /// `application_name` is intentionally excluded — it does not
    /// influence the planner, so including it would fragment the
    /// prepared-statement cache without correctness benefit.
    pub fn as_btreemap_for_planner(&self) -> BTreeMap<String, String> {
        self.parameters
            .iter()
            .filter(|(k, _)| k != &"application_name")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn add_parameter_message(key: &str, value: &str, buffer: &mut BytesMut) {
        buffer.put_u8(b'S');

        // 4 is len of i32, plus null terminators.
        let len = 4 + key.len() + 1 + value.len() + 1;

        buffer.put_i32(len as i32);
        buffer.put_slice(key.as_bytes());
        buffer.put_u8(0);
        buffer.put_slice(value.as_bytes());
        buffer.put_u8(0);
    }
}

impl From<&ServerParameters> for BytesMut {
    fn from(server_parameters: &ServerParameters) -> Self {
        let mut bytes = BytesMut::new();

        for (key, value) in &server_parameters.parameters {
            ServerParameters::add_parameter_message(key, value, &mut bytes);
        }

        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::{canonicalize_param_name, ServerParameters};

    #[test]
    fn canonicalize_timezone_matches_any_case() {
        assert_eq!(canonicalize_param_name("timezone".to_string()), "TimeZone");
        assert_eq!(canonicalize_param_name("TIMEZONE".to_string()), "TimeZone");
        assert_eq!(canonicalize_param_name("TimeZone".to_string()), "TimeZone");
        assert_eq!(canonicalize_param_name("TimezONE".to_string()), "TimeZone");
    }

    #[test]
    fn canonicalize_datestyle_matches_any_case() {
        assert_eq!(
            canonicalize_param_name("datestyle".to_string()),
            "DateStyle"
        );
        assert_eq!(
            canonicalize_param_name("DATESTYLE".to_string()),
            "DateStyle"
        );
        assert_eq!(
            canonicalize_param_name("DateStyle".to_string()),
            "DateStyle"
        );
    }

    #[test]
    fn canonicalize_intervalstyle_matches_any_case() {
        assert_eq!(
            canonicalize_param_name("intervalstyle".to_string()),
            "IntervalStyle"
        );
        assert_eq!(
            canonicalize_param_name("INTERVALSTYLE".to_string()),
            "IntervalStyle"
        );
    }

    #[test]
    fn canonicalize_lowercases_non_tracked_keys() {
        // PG GUC lookup is case-insensitive, so untracked names must
        // collapse to one canonical form too, otherwise a `work_mem`
        // baseline and a `Work_Mem` pool override become two rows on
        // the wire instead of one cascaded value.
        assert_eq!(canonicalize_param_name("work_mem".to_string()), "work_mem");
        assert_eq!(canonicalize_param_name("Work_Mem".to_string()), "work_mem");
        assert_eq!(canonicalize_param_name("WORK_MEM".to_string()), "work_mem");
        assert_eq!(
            canonicalize_param_name("statement_timeout".to_string()),
            "statement_timeout"
        );
        assert_eq!(
            canonicalize_param_name("Statement_Timeout".to_string()),
            "statement_timeout"
        );
    }

    #[test]
    fn as_btreemap_for_planner_drops_application_name() {
        // application_name is identity metadata, not planner input — folding
        // it into the prepared-statement cache key would shatter the cache
        // by client without any correctness benefit.
        let mut sp = ServerParameters::new();
        sp.set_param("application_name", "client-A", true);
        sp.set_param("search_path", "schema_a", true);
        let view = sp.as_btreemap_for_planner();
        assert!(view.contains_key("search_path"));
        assert!(!view.contains_key("application_name"));
        assert_eq!(view.get("search_path"), Some(&"schema_a".to_string()));
    }

    #[test]
    fn as_btreemap_for_planner_iterates_in_key_order() {
        // BTreeMap is sorted by key on iteration, which is what makes the
        // resulting hash stable across clients that happened to populate
        // the same parameter set in different order.
        let mut sp = ServerParameters::new();
        sp.set_param("z_param", "z", true);
        sp.set_param("a_param", "a", true);
        sp.set_param("m_param", "m", true);
        let keys: Vec<String> = sp.as_btreemap_for_planner().keys().cloned().collect();
        assert_eq!(keys, vec!["a_param", "m_param", "z_param"]);
    }
}
