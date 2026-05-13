use bytes::{BufMut, BytesMut};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};

use crate::config::VERSION;

static TRACKED_PARAMETERS: Lazy<HashSet<String>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("client_encoding".to_string());
    set.insert("DateStyle".to_string());
    set.insert("TimeZone".to_string());
    set.insert("standard_conforming_strings".to_string());
    set.insert("application_name".to_string());
    set
});

/// Canonicalise a PostgreSQL session parameter name so that startup-time
/// lowercase forms (`timezone`, `datestyle`) match the
/// `ParameterStatus` casing PG sends back (`TimeZone`, `DateStyle`).
/// Used both by `ServerParameters::set_param` (where it has lived since
/// day one) and by `Server::startup` when it captures the operator-
/// managed key set: without the canonical form, `sync_parameters`
/// filters by exact-string match and a client startup value reported
/// as `TimeZone` would overwrite an operator value set as `timezone`.
pub fn canonicalize_param_name(key: String) -> String {
    // PostgreSQL GUC names are case-insensitive, so a client startup
    // value labelled `TIMEZONE` or `DateStyle` and an operator default
    // configured as `timezone` must collapse to the same canonical
    // string. Exact `==` lowercase would let a mixed-case client value
    // slip past `operator_managed_startup_keys` and overwrite the
    // operator default.
    if key.eq_ignore_ascii_case("timezone") {
        "TimeZone".to_string()
    } else if key.eq_ignore_ascii_case("datestyle") {
        "DateStyle".to_string()
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
    use super::canonicalize_param_name;

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
    fn canonicalize_passes_through_other_keys() {
        assert_eq!(
            canonicalize_param_name("application_name".to_string()),
            "application_name"
        );
        assert_eq!(canonicalize_param_name("FooBar".to_string()), "FooBar");
    }
}
