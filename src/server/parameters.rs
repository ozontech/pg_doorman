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

        server_parameters.set_param("client_encoding".to_string(), "UTF8".to_string(), false);
        server_parameters.set_param("DateStyle".to_string(), "ISO, MDY".to_string(), false);
        server_parameters.set_param("TimeZone".to_string(), "Etc/UTC".to_string(), false);
        server_parameters.set_param("server_version".to_string(), VERSION.to_string(), true);
        server_parameters.set_param("server_encoding".to_string(), "UTF-8".to_string(), true);
        server_parameters.set_param(
            "standard_conforming_strings".to_string(),
            "on".to_string(),
            false,
        );
        // (64 bit = on) as of PostgreSQL 10, this is always on.
        server_parameters.set_param("integer_datetimes".to_string(), "on".to_string(), false);
        server_parameters.set_param(
            "application_name".to_string(),
            "pg_doorman".to_string(),
            false,
        );

        server_parameters
    }

    /// If `startup` is false, then only tracked parameters will be set.
    pub fn set_param(&mut self, mut key: String, value: String, startup: bool) {
        // Startup parameters may come uncapitalized, while ParameterStatus uses canonical keys.
        if key == "timezone" {
            key = "TimeZone".to_string();
        } else if key == "datestyle" {
            key = "DateStyle".to_string();
        };

        if TRACKED_PARAMETERS.contains(&key) || startup {
            self.parameters.insert(key, value);
        }
    }

    pub fn set_from_hashmap(&mut self, parameters: HashMap<String, String>, startup: bool) {
        for (key, value) in parameters {
            self.set_param(key.to_string(), value.to_string(), startup);
        }
    }

    #[inline(always)]
    pub(crate) fn compare_params(&self, incoming_parameters: &ServerParameters) -> HashMap<String, String> {
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
