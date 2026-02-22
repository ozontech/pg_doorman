//! Annotated config generation with comments and field documentation.
//!
//! This module generates fully documented configuration files (TOML and YAML)
//! with inline comments for every field. Field descriptions are loaded from
//! `fields.yaml` — the single source of truth for all config documentation.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::config::{Config, ConfigFormat, Pool, PoolMode, Prometheus, User};

// ---------------------------------------------------------------------------
// YAML field descriptions — single source of truth
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct I18n {
    pub en: String,
    pub ru: String,
}

impl I18n {
    pub(crate) fn get(&self, russian: bool) -> &str {
        if russian {
            &self.ru
        } else {
            &self.en
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct FieldDesc {
    #[serde(default)]
    pub config: Option<I18n>,
    /// Rich description for reference documentation (EN only).
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct FieldsMap {
    pub general: HashMap<String, FieldDesc>,
    pub pool: HashMap<String, FieldDesc>,
    pub user: HashMap<String, FieldDesc>,
    pub prometheus: HashMap<String, FieldDesc>,
}

#[derive(Deserialize)]
pub(crate) struct FieldsData {
    pub sections: HashMap<String, I18n>,
    pub texts: HashMap<String, I18n>,
    pub fields: FieldsMap,
}

impl FieldsData {
    pub(crate) fn field(&self, section: &str, name: &str) -> &FieldDesc {
        let map = match section {
            "general" => &self.fields.general,
            "pool" => &self.fields.pool,
            "user" => &self.fields.user,
            "prometheus" => &self.fields.prometheus,
            _ => panic!("Unknown section: {section}"),
        };
        map.get(name)
            .unwrap_or_else(|| panic!("Unknown field: {section}.{name}"))
    }

    pub(crate) fn text(&self, key: &str) -> &I18n {
        self.texts
            .get(key)
            .unwrap_or_else(|| panic!("Unknown text: {key}"))
    }

    pub(crate) fn section_title(&self, key: &str) -> &I18n {
        self.sections
            .get(key)
            .unwrap_or_else(|| panic!("Unknown section: {key}"))
    }
}

pub(crate) static FIELDS: LazyLock<FieldsData> = LazyLock::new(|| {
    serde_yaml::from_str(include_str!("fields.yaml")).expect("Failed to parse fields.yaml")
});

// ---------------------------------------------------------------------------
// Helper functions for writing field comments from YAML
// ---------------------------------------------------------------------------

/// Write field config description from YAML (no default line).
fn write_field_desc(w: &mut ConfigWriter, indent: usize, section: &str, field: &str) {
    let desc = FIELDS.field(section, field);
    if let Some(ref config) = desc.config {
        let text = config.get(w.russian);
        let text = text.trim_end();
        if !text.is_empty() {
            for line in text.split('\n') {
                w.comment(indent, line);
            }
        }
    }
}

/// Write field config description + default from YAML.
fn write_field_comment(w: &mut ConfigWriter, indent: usize, section: &str, field: &str) {
    write_field_desc(w, indent, section, field);
    let desc = FIELDS.field(section, field);
    if let Some(ref default) = desc.default {
        w.comment(indent, &format!("Default: {default}"));
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate a reference config with example data (no PG connection needed).
pub fn generate_reference_config(format: ConfigFormat, russian: bool) -> String {
    let mut config = Config {
        path: String::new(),
        ..Config::default()
    };
    config.general.port = 6432;

    let pool = Pool {
        pool_mode: PoolMode::Transaction,
        server_host: "127.0.0.1".to_string(),
        server_port: 5432,
        server_database: None,
        connect_timeout: None,
        idle_timeout: None,
        server_lifetime: None,
        cleanup_server_connections: true,
        log_client_parameter_status_changes: false,
        application_name: None,
        prepared_statements_cache_size: None,
        users: vec![User {
            username: "app_user".to_string(),
            password: "md5dd9a0f26a4302744db881776a09bbfad".to_string(),
            pool_size: 40,
            min_pool_size: None,
            pool_mode: None,
            server_lifetime: None,
            server_username: None,
            server_password: None,
            auth_pam_service: None,
        }],
    };

    let mut pools = BTreeMap::new();
    pools.insert("exampledb".to_string(), pool);
    config.pools = pools.into_iter().collect();

    generate_annotated_config(&config, format, russian)
}

/// Generate annotated config string with comments for all fields.
pub fn generate_annotated_config(config: &Config, format: ConfigFormat, russian: bool) -> String {
    let mut w = ConfigWriter::new(format, russian);

    write_header(&mut w);
    write_include_section(&mut w);
    write_general_section(&mut w, config);
    write_prometheus_section(&mut w, &config.prometheus);
    write_talos_section(&mut w);
    write_pools_section(&mut w, config);

    w.output
}

// ---------------------------------------------------------------------------
// ConfigWriter — abstraction over TOML/YAML formatting differences
// ---------------------------------------------------------------------------

struct ConfigWriter {
    format: ConfigFormat,
    russian: bool,
    output: String,
}

impl ConfigWriter {
    fn new(format: ConfigFormat, russian: bool) -> Self {
        Self {
            format,
            russian,
            output: String::with_capacity(8 * 1024),
        }
    }

    fn blank(&mut self) {
        self.output.push('\n');
    }

    /// Select English or Russian text.
    fn t<'a>(&self, en: &'a str, ru: &'a str) -> &'a str {
        if self.russian {
            ru
        } else {
            en
        }
    }

    /// Write a comment line with given indent level (2 spaces per level for YAML).
    fn comment(&mut self, indent: usize, text: &str) {
        let prefix = self.indent_str(indent);
        if text.is_empty() {
            let _ = writeln!(self.output, "{prefix}#");
        } else {
            let _ = writeln!(self.output, "{prefix}# {text}");
        }
    }

    /// Write a commented-out key=value pair.
    fn commented_kv(&mut self, indent: usize, key: &str, value: &str) {
        let prefix = self.indent_str(indent);
        match self.format {
            ConfigFormat::Toml => {
                let _ = writeln!(self.output, "{prefix}# {key} = {value}");
            }
            ConfigFormat::Yaml => {
                let _ = writeln!(self.output, "{prefix}# {key}: {value}");
            }
        }
    }

    /// Write a section header.
    fn section(&mut self, indent: usize, name: &str) {
        match self.format {
            ConfigFormat::Toml => {
                let _ = writeln!(self.output, "[{name}]");
            }
            ConfigFormat::Yaml => {
                let prefix = self.indent_str(indent);
                let _ = writeln!(self.output, "{prefix}{name}:");
            }
        }
    }

    /// Write a key=value pair at given indent level.
    fn kv(&mut self, indent: usize, key: &str, value: &str) {
        let prefix = self.indent_str(indent);
        match self.format {
            ConfigFormat::Toml => {
                let _ = writeln!(self.output, "{prefix}{key} = {value}");
            }
            ConfigFormat::Yaml => {
                let _ = writeln!(self.output, "{prefix}{key}: {value}");
            }
        }
    }

    /// Write a separator line.
    fn separator(&mut self, indent: usize, title: &str) {
        let prefix = self.indent_str(indent);
        let _ = writeln!(self.output, "{prefix}# {:-<74}", "");
        let _ = writeln!(self.output, "{prefix}# {title}");
        let _ = writeln!(self.output, "{prefix}# {:-<74}", "");
    }

    /// Write a major section separator.
    fn major_separator(&mut self, title: &str) {
        let prefix = self.indent_str(0);
        let _ = writeln!(self.output, "{prefix}# {:#<76}", "");
        let _ = writeln!(self.output, "{prefix}# {title}");
        let _ = writeln!(self.output, "{prefix}# {:#<76}", "");
    }

    fn indent_str(&self, level: usize) -> String {
        match self.format {
            ConfigFormat::Toml => String::new(),
            ConfigFormat::Yaml => "  ".repeat(level),
        }
    }

    fn str_val(&self, s: &str) -> String {
        format!("\"{s}\"")
    }

    fn bool_val(&self, b: bool) -> String {
        b.to_string()
    }

    fn num_val<T: std::fmt::Display>(&self, n: T) -> String {
        n.to_string()
    }

    fn subsection(&mut self, name: &str) {
        match self.format {
            ConfigFormat::Toml => {
                let _ = writeln!(self.output, "[{name}]");
            }
            ConfigFormat::Yaml => {}
        }
    }

    fn empty_array(&self) -> String {
        "[]".to_string()
    }

    fn field_indent(&self) -> usize {
        match self.format {
            ConfigFormat::Toml => 0,
            ConfigFormat::Yaml => 1,
        }
    }

    fn pool_field_indent(&self) -> usize {
        match self.format {
            ConfigFormat::Toml => 0,
            ConfigFormat::Yaml => 2,
        }
    }

    fn user_field_indent(&self) -> usize {
        match self.format {
            ConfigFormat::Toml => 0,
            ConfigFormat::Yaml => 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Section writers
// ---------------------------------------------------------------------------

fn write_header(w: &mut ConfigWriter) {
    let f = &*FIELDS;
    let format_name = match w.format {
        ConfigFormat::Toml => "TOML",
        ConfigFormat::Yaml => "YAML",
    };
    w.comment(
        0,
        &format!(
            "pg_doorman {format_name} {}",
            w.t("configuration", "конфигурация")
        ),
    );
    w.comment(
        0,
        "============================================================================",
    );
    w.comment(0, f.text("header_warning").get(w.russian));
    w.comment(0, f.text("header_yaml_recommended").get(w.russian));
    w.comment(0, f.text("header_toml_compat").get(w.russian));
    w.comment(0, f.text("header_auto_detect").get(w.russian));
    w.comment(
        0,
        "============================================================================",
    );

    if w.format == ConfigFormat::Yaml {
        w.comment(0, "");
        w.comment(0, f.text("human_readable_title").get(w.russian));
        w.comment(
            0,
            "============================================================================",
        );
        w.comment(0, f.text("human_readable_desc").get(w.russian));
        w.comment(0, f.text("human_readable_compat").get(w.russian));
        w.comment(0, "");
        w.comment(0, f.text("duration_title").get(w.russian));
        w.comment(0, f.text("duration_plain").get(w.russian));
        w.comment(0, "  - \"Nms\" : milliseconds (e.g., \"100ms\")");
        w.comment(0, f.text("duration_s").get(w.russian));
        w.comment(0, f.text("duration_m").get(w.russian));
        w.comment(0, f.text("duration_h").get(w.russian));
        w.comment(0, f.text("duration_d").get(w.russian));
        w.comment(0, "");
        w.comment(0, f.text("byte_size_title").get(w.russian));
        w.comment(0, f.text("byte_size_plain").get(w.russian));
        w.comment(0, "  - \"NB\"  : bytes (e.g., \"1024B\")");
        w.comment(
            0,
            "  - \"NK\" or \"NKB\" : kilobytes (e.g., \"1K\" or \"1KB\" = 1024 bytes)",
        );
        w.comment(
            0,
            "  - \"NM\" or \"NMB\" : megabytes (e.g., \"1M\" or \"1MB\" = 1048576 bytes)",
        );
        w.comment(
            0,
            "  - \"NG\" or \"NGB\" : gigabytes (e.g., \"1G\" or \"1GB\" = 1073741824 bytes)",
        );
        w.comment(0, "");
        w.comment(0, f.text("example_title").get(w.russian));
        w.comment(0, f.text("example_connect_timeout").get(w.russian));
        w.comment(0, f.text("example_idle_timeout").get(w.russian));
        w.comment(0, f.text("example_max_memory").get(w.russian));
        w.comment(
            0,
            "============================================================================",
        );
    }
    w.blank();
}

fn write_include_section(w: &mut ConfigWriter) {
    let f = &*FIELDS;
    w.comment(0, f.text("include_desc").get(w.russian));
    w.comment(0, f.text("include_merge").get(w.russian));
    w.section(0, "include");
    let fi = w.field_indent();
    match w.format {
        ConfigFormat::Toml => {
            w.commented_kv(
                fi,
                "files",
                "[\"/etc/pg_doorman/pools.toml\", \"/etc/pg_doorman/hba.toml\"]",
            );
        }
        ConfigFormat::Yaml => {
            w.comment(fi, "files:");
            w.comment(fi, "  - \"/etc/pg_doorman/pools.yaml\"");
            w.comment(fi, "  - \"/etc/pg_doorman/hba.yaml\"");
        }
    }
    w.blank();
}

fn write_general_section(w: &mut ConfigWriter, config: &Config) {
    let f = &*FIELDS;
    let g = &config.general;
    w.major_separator(f.text("general_title").get(w.russian));
    w.section(0, "general");

    let fi = w.field_indent();

    // --- Network Settings ---
    w.separator(fi, f.section_title("network").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "host");
    w.kv(fi, "host", &w.str_val(&g.host));
    w.blank();

    write_field_comment(w, fi, "general", "port");
    w.kv(fi, "port", &w.num_val(g.port));
    w.blank();

    write_field_comment(w, fi, "general", "backlog");
    w.kv(fi, "backlog", &w.num_val(g.backlog));
    w.blank();

    // --- Connection Timeouts ---
    w.separator(fi, f.section_title("timeouts").get(w.russian));
    w.blank();

    write_field_desc(w, fi, "general", "connect_timeout");
    write_duration_value(
        w,
        fi,
        "connect_timeout",
        g.connect_timeout.as_millis(),
        "3s",
        "3000 ms",
    );

    write_field_desc(w, fi, "general", "query_wait_timeout");
    write_duration_value(
        w,
        fi,
        "query_wait_timeout",
        g.query_wait_timeout.as_millis(),
        "5s",
        "5000 ms",
    );

    write_field_desc(w, fi, "general", "idle_timeout");
    write_duration_value(
        w,
        fi,
        "idle_timeout",
        g.idle_timeout.as_millis(),
        "5m",
        "300000 ms",
    );

    write_field_desc(w, fi, "general", "server_lifetime");
    write_duration_value(
        w,
        fi,
        "server_lifetime",
        g.server_lifetime.as_millis(),
        "5m",
        "300000 ms",
    );

    write_field_desc(w, fi, "general", "retain_connections_time");
    write_duration_value(
        w,
        fi,
        "retain_connections_time",
        g.retain_connections_time.as_millis(),
        "30s",
        "30000 ms",
    );

    write_field_comment(w, fi, "general", "retain_connections_max");
    w.kv(
        fi,
        "retain_connections_max",
        &w.num_val(g.retain_connections_max),
    );
    w.blank();

    write_field_desc(w, fi, "general", "server_idle_check_timeout");
    write_duration_value(
        w,
        fi,
        "server_idle_check_timeout",
        g.server_idle_check_timeout.as_millis(),
        "60s",
        "",
    );

    write_field_desc(w, fi, "general", "shutdown_timeout");
    write_duration_value(
        w,
        fi,
        "shutdown_timeout",
        g.shutdown_timeout.as_millis(),
        "10s",
        "10000 ms",
    );

    write_field_desc(w, fi, "general", "proxy_copy_data_timeout");
    write_duration_value(
        w,
        fi,
        "proxy_copy_data_timeout",
        g.proxy_copy_data_timeout.as_millis(),
        "15s",
        "15000 ms",
    );

    // --- TCP Settings ---
    w.separator(fi, f.section_title("tcp").get(w.russian));
    w.blank();

    write_field_desc(w, fi, "general", "tcp_keepalives_idle");
    w.comment(fi, "Default: 5");
    w.kv(fi, "tcp_keepalives_idle", &w.num_val(g.tcp_keepalives_idle));
    w.comment(fi, "Default: 5");
    w.kv(
        fi,
        "tcp_keepalives_interval",
        &w.num_val(g.tcp_keepalives_interval),
    );
    w.comment(fi, "Default: 5");
    w.kv(
        fi,
        "tcp_keepalives_count",
        &w.num_val(g.tcp_keepalives_count),
    );
    w.blank();

    write_field_comment(w, fi, "general", "tcp_so_linger");
    w.kv(fi, "tcp_so_linger", &w.num_val(g.tcp_so_linger));
    w.blank();

    write_field_comment(w, fi, "general", "tcp_no_delay");
    w.kv(fi, "tcp_no_delay", &w.bool_val(g.tcp_no_delay));
    w.blank();

    write_field_comment(w, fi, "general", "tcp_user_timeout");
    w.kv(fi, "tcp_user_timeout", &w.num_val(g.tcp_user_timeout));
    w.blank();

    write_field_desc(w, fi, "general", "unix_socket_buffer_size");
    write_byte_size_value(
        w,
        fi,
        "unix_socket_buffer_size",
        g.unix_socket_buffer_size.as_bytes(),
        "1MB",
        "1048576 bytes",
    );

    // --- Connection Limits ---
    w.separator(fi, f.section_title("limits").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "max_connections");
    w.kv(fi, "max_connections", &w.num_val(g.max_connections));
    w.blank();

    write_field_comment(w, fi, "general", "max_concurrent_creates");
    w.kv(
        fi,
        "max_concurrent_creates",
        &w.num_val(g.max_concurrent_creates),
    );
    w.blank();

    write_field_desc(w, fi, "general", "max_memory_usage");
    write_byte_size_value(
        w,
        fi,
        "max_memory_usage",
        g.max_memory_usage.as_bytes(),
        "256MB",
        "268435456 bytes",
    );

    // --- Logging ---
    w.separator(fi, f.section_title("logging").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "log_client_connections");
    w.kv(
        fi,
        "log_client_connections",
        &w.bool_val(g.log_client_connections),
    );
    w.blank();

    write_field_comment(w, fi, "general", "log_client_disconnections");
    w.kv(
        fi,
        "log_client_disconnections",
        &w.bool_val(g.log_client_disconnections),
    );
    w.blank();

    write_field_comment(w, fi, "general", "syslog_prog_name");
    if let Some(ref name) = g.syslog_prog_name {
        w.kv(fi, "syslog_prog_name", &w.str_val(name));
    } else {
        w.commented_kv(fi, "syslog_prog_name", &w.str_val("pg_doorman"));
    }
    w.blank();

    // --- Worker Settings ---
    w.separator(fi, f.section_title("workers").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "worker_threads");
    w.kv(fi, "worker_threads", &w.num_val(g.worker_threads));
    w.blank();

    write_field_comment(w, fi, "general", "worker_cpu_affinity_pinning");
    w.kv(
        fi,
        "worker_cpu_affinity_pinning",
        &w.bool_val(g.worker_cpu_affinity_pinning),
    );
    w.blank();

    // Tokio runtime settings note
    write_field_desc(w, fi, "general", "tokio_settings_note");
    w.blank();

    // worker_stack_size (optional)
    match w.format {
        ConfigFormat::Toml => {
            w.comment(
                fi,
                w.t(
                    "Stack size for each worker thread (in bytes).",
                    "Размер стека каждого рабочего потока (в байтах).",
                ),
            );
            w.comment(
                fi,
                w.t(
                    "Default: not set (uses tokio's default)",
                    "По умолчанию: не задан (используется значение tokio)",
                ),
            );
        }
        ConfigFormat::Yaml => {
            write_field_desc(w, fi, "general", "worker_stack_size");
            w.comment(
                fi,
                "Supports human-readable format: \"8MB\", \"8M\", or 8388608 (bytes)",
            );
            w.comment(
                fi,
                w.t(
                    "Default: not set (uses tokio's default)",
                    "По умолчанию: не задан (используется значение tokio)",
                ),
            );
        }
    }
    if let Some(ref stack_size) = g.worker_stack_size {
        match w.format {
            ConfigFormat::Toml => w.kv(fi, "worker_stack_size", &w.num_val(stack_size.as_bytes())),
            ConfigFormat::Yaml => w.kv(fi, "worker_stack_size", &w.str_val("8MB")),
        }
    } else {
        match w.format {
            ConfigFormat::Toml => w.commented_kv(fi, "worker_stack_size", "8388608"),
            ConfigFormat::Yaml => w.commented_kv(fi, "worker_stack_size", "\"8MB\""),
        }
    }
    w.blank();

    write_field_desc(w, fi, "general", "max_blocking_threads");
    w.comment(
        fi,
        w.t(
            "Default: not set (uses tokio's default)",
            "По умолчанию: не задан (используется значение tokio)",
        ),
    );
    if let Some(val) = g.max_blocking_threads {
        w.kv(fi, "max_blocking_threads", &w.num_val(val));
    } else {
        w.commented_kv(fi, "max_blocking_threads", "64");
    }
    w.blank();

    write_field_desc(w, fi, "general", "tokio_global_queue_interval");
    w.comment(
        fi,
        w.t(
            "Default: not set (uses tokio's default)",
            "По умолчанию: не задан (используется значение tokio)",
        ),
    );
    if let Some(val) = g.tokio_global_queue_interval {
        w.kv(fi, "tokio_global_queue_interval", &w.num_val(val));
    } else {
        w.commented_kv(fi, "tokio_global_queue_interval", "5");
    }
    w.blank();

    write_field_desc(w, fi, "general", "tokio_event_interval");
    w.comment(
        fi,
        w.t(
            "Default: not set (uses tokio's default)",
            "По умолчанию: не задан (используется значение tokio)",
        ),
    );
    if let Some(val) = g.tokio_event_interval {
        w.kv(fi, "tokio_event_interval", &w.num_val(val));
    } else {
        w.commented_kv(fi, "tokio_event_interval", "1");
    }
    w.blank();

    // --- Pool Behavior ---
    w.separator(fi, f.section_title("pool_behavior").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "server_round_robin");
    w.kv(fi, "server_round_robin", &w.bool_val(g.server_round_robin));
    w.blank();

    write_field_comment(w, fi, "general", "sync_server_parameters");
    w.kv(
        fi,
        "sync_server_parameters",
        &w.bool_val(g.sync_server_parameters),
    );
    w.blank();

    write_field_desc(w, fi, "general", "message_size_to_be_stream");
    write_byte_size_value(
        w,
        fi,
        "message_size_to_be_stream",
        g.message_size_to_be_stream.as_bytes(),
        "1MB",
        "1048576 bytes",
    );

    write_field_comment(w, fi, "general", "pooler_check_query");
    w.kv(fi, "pooler_check_query", &w.str_val(&g.pooler_check_query));
    w.blank();

    // --- Prepared Statements ---
    w.separator(fi, f.section_title("prepared").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "prepared_statements");
    w.kv(
        fi,
        "prepared_statements",
        &w.bool_val(g.prepared_statements),
    );
    w.blank();

    write_field_comment(w, fi, "general", "prepared_statements_cache_size");
    w.kv(
        fi,
        "prepared_statements_cache_size",
        &w.num_val(g.prepared_statements_cache_size),
    );
    w.blank();

    write_field_comment(w, fi, "general", "client_prepared_statements_cache_size");
    w.kv(
        fi,
        "client_prepared_statements_cache_size",
        &w.num_val(g.client_prepared_statements_cache_size),
    );
    w.blank();

    // --- Admin Console ---
    w.separator(fi, f.section_title("admin").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "admin_username");
    w.kv(fi, "admin_username", &w.str_val(&g.admin_username));
    w.blank();

    write_field_comment(w, fi, "general", "admin_password");
    w.kv(fi, "admin_password", &w.str_val(&g.admin_password));
    w.blank();

    // --- TLS Settings (Client-facing) ---
    w.separator(fi, f.section_title("tls_client").get(w.russian));
    w.blank();

    write_field_desc(w, fi, "general", "tls_certificate");
    if let Some(ref cert) = g.tls_certificate {
        w.kv(fi, "tls_certificate", &w.str_val(cert));
    } else {
        w.commented_kv(fi, "tls_certificate", "\"/etc/pg_doorman/server.crt\"");
    }
    w.blank();

    write_field_desc(w, fi, "general", "tls_private_key");
    if let Some(ref key) = g.tls_private_key {
        w.kv(fi, "tls_private_key", &w.str_val(key));
    } else {
        w.commented_kv(fi, "tls_private_key", "\"/etc/pg_doorman/server.key\"");
    }
    w.blank();

    write_field_desc(w, fi, "general", "tls_ca_cert");
    if let Some(ref ca) = g.tls_ca_cert {
        w.kv(fi, "tls_ca_cert", &w.str_val(ca));
    } else {
        w.commented_kv(fi, "tls_ca_cert", "\"/etc/pg_doorman/ca.crt\"");
    }
    w.blank();

    write_field_comment(w, fi, "general", "tls_mode");
    if let Some(ref mode) = g.tls_mode {
        w.kv(fi, "tls_mode", &w.str_val(mode));
    } else {
        w.kv(fi, "tls_mode", &w.str_val("allow"));
    }
    w.blank();

    write_field_comment(w, fi, "general", "tls_rate_limit_per_second");
    w.kv(
        fi,
        "tls_rate_limit_per_second",
        &w.num_val(g.tls_rate_limit_per_second),
    );
    w.blank();

    // --- TLS Settings (Server-facing) ---
    w.separator(fi, f.section_title("tls_server").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "server_tls");
    w.kv(fi, "server_tls", &w.bool_val(g.server_tls));
    w.blank();

    write_field_comment(w, fi, "general", "verify_server_certificate");
    w.kv(
        fi,
        "verify_server_certificate",
        &w.bool_val(g.verify_server_certificate),
    );
    w.blank();

    // --- Daemon Mode ---
    w.separator(fi, f.section_title("daemon").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "daemon_pid_file");
    w.commented_kv(fi, "daemon_pid_file", &w.str_val(&g.daemon_pid_file));
    w.blank();

    // --- Access Control (Legacy) ---
    w.separator(fi, f.section_title("hba_legacy").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "general", "hba");
    w.kv(fi, "hba", &w.empty_array());
    w.blank();

    // --- Access Control (pg_hba) ---
    w.separator(fi, f.section_title("hba").get(w.russian));
    w.blank();

    w.comment(fi, f.text("pg_hba_desc").get(w.russian));
    w.comment(fi, f.text("pg_hba_formats").get(w.russian));
    w.comment(fi, "");
    write_pg_hba_examples(w, fi);
    w.comment(fi, "");
    w.comment(fi, f.text("pg_hba_rule_format").get(w.russian));
    w.comment(fi, f.text("pg_hba_types").get(w.russian));
    w.comment(fi, f.text("pg_hba_methods").get(w.russian));
    w.comment(fi, "");
    w.comment(fi, f.text("pg_hba_trust_1").get(w.russian));
    w.comment(fi, f.text("pg_hba_trust_2").get(w.russian));
    w.comment(fi, f.text("pg_hba_trust_3").get(w.russian));
    w.comment(fi, "");
    write_pg_hba_rule_examples(w, fi);
    w.blank();
}

fn write_pg_hba_examples(w: &mut ConfigWriter, fi: usize) {
    match w.format {
        ConfigFormat::Toml => {
            w.comment(fi, "1. Inline multiline string:");
            w.comment(fi, "   pg_hba = \"\"\"");
            w.comment(fi, "   host all all 0.0.0.0/0 md5");
            w.comment(fi, "   hostssl all all 10.0.0.0/8 scram-sha-256");
            w.comment(fi, "   local all all trust");
            w.comment(fi, "   \"\"\"");
            w.comment(fi, "");
            w.comment(fi, "2. Path to external file:");
            w.comment(fi, "   pg_hba = { path = \"/etc/pg_doorman/pg_hba.conf\" }");
            w.comment(fi, "");
            w.comment(fi, "3. Inline content in map format:");
            w.comment(
                fi,
                "   pg_hba = { content = \"host all all 127.0.0.1/32 trust\" }",
            );
        }
        ConfigFormat::Yaml => {
            w.comment(fi, "1. Inline multiline string:");
            w.comment(fi, "   pg_hba: |");
            w.comment(fi, "     host all all 0.0.0.0/0 md5");
            w.comment(fi, "     hostssl all all 10.0.0.0/8 scram-sha-256");
            w.comment(fi, "     local all all trust");
            w.comment(fi, "");
            w.comment(fi, "2. Path to external file:");
            w.comment(fi, "   pg_hba:");
            w.comment(fi, "     path: \"/etc/pg_doorman/pg_hba.conf\"");
            w.comment(fi, "");
            w.comment(fi, "3. Inline content in map format:");
            w.comment(fi, "   pg_hba:");
            w.comment(fi, "     content: \"host all all 127.0.0.1/32 trust\"");
        }
    }
}

fn write_pg_hba_rule_examples(w: &mut ConfigWriter, fi: usize) {
    let f = &*FIELDS;
    match w.format {
        ConfigFormat::Toml => {
            w.comment(fi, f.text("pg_hba_example_title").get(w.russian));
            w.comment(fi, "pg_hba = \"\"\"");
            w.comment(fi, "# Allow local connections without password");
            w.comment(fi, "local all all trust");
            w.comment(fi, "# Require SSL and SCRAM for internal network");
            w.comment(fi, "hostssl all all 10.0.0.0/8 scram-sha-256");
            w.comment(fi, "# Allow MD5 auth from anywhere");
            w.comment(fi, "host all all 0.0.0.0/0 md5");
            w.comment(fi, "# Reject all other connections");
            w.comment(fi, "host all all 0.0.0.0/0 reject");
            w.comment(fi, "\"\"\"");
        }
        ConfigFormat::Yaml => {
            w.comment(fi, f.text("pg_hba_example_title").get(w.russian));
            w.comment(fi, "pg_hba: |");
            w.comment(fi, "  # Allow local connections without password");
            w.comment(fi, "  local all all trust");
            w.comment(fi, "  # Require SSL and SCRAM for internal network");
            w.comment(fi, "  hostssl all all 10.0.0.0/8 scram-sha-256");
            w.comment(fi, "  # Allow MD5 auth from anywhere");
            w.comment(fi, "  host all all 0.0.0.0/0 md5");
            w.comment(fi, "  # Reject all other connections");
            w.comment(fi, "  host all all 0.0.0.0/0 reject");
        }
    }
}

fn write_prometheus_section(w: &mut ConfigWriter, prom: &Prometheus) {
    let f = &*FIELDS;
    w.major_separator(f.text("prometheus_title").get(w.russian));
    w.section(0, "prometheus");
    let fi = w.field_indent();

    write_field_comment(w, fi, "prometheus", "enabled");
    w.kv(fi, "enabled", &w.bool_val(prom.enabled));
    w.blank();

    write_field_comment(w, fi, "prometheus", "host");
    w.kv(fi, "host", &w.str_val(&prom.host));
    w.blank();

    write_field_comment(w, fi, "prometheus", "port");
    w.kv(fi, "port", &w.num_val(prom.port));
    w.blank();
}

fn write_talos_section(w: &mut ConfigWriter) {
    let f = &*FIELDS;
    w.major_separator(f.text("talos_title").get(w.russian));
    w.comment(0, f.text("talos_desc").get(w.russian));
    match w.format {
        ConfigFormat::Toml => {
            w.comment(0, "[talos]");
            w.comment(0, &format!("# {}", f.text("talos_keys").get(w.russian)));
            w.comment(
                0,
                "keys = [\"/etc/pg_doorman/talos/public-key-1.pem\", \"/etc/pg_doorman/talos/public-key-2.pem\"]",
            );
            w.comment(
                0,
                &format!("# {}", f.text("talos_databases").get(w.russian)),
            );
            w.comment(0, "databases = [\"talos_db1\", \"talos_db2\"]");
        }
        ConfigFormat::Yaml => {
            w.comment(0, "talos:");
            w.comment(0, &format!("  # {}", f.text("talos_keys").get(w.russian)));
            w.comment(0, "  keys:");
            w.comment(0, "    - \"/etc/pg_doorman/talos/public-key-1.pem\"");
            w.comment(0, "    - \"/etc/pg_doorman/talos/public-key-2.pem\"");
            w.comment(
                0,
                &format!("  # {}", f.text("talos_databases").get(w.russian)),
            );
            w.comment(0, "  databases:");
            w.comment(0, "    - \"talos_db1\"");
            w.comment(0, "    - \"talos_db2\"");
        }
    }
    w.blank();
}

fn write_pools_section(w: &mut ConfigWriter, config: &Config) {
    let f = &*FIELDS;
    w.major_separator(f.text("pools_title").get(w.russian));
    w.comment(0, f.text("pools_desc").get(w.russian));
    w.comment(0, f.text("pools_names").get(w.russian));
    w.section(0, "pools");
    w.blank();

    // Sort pools by name for deterministic output
    let mut pool_names: Vec<&String> = config.pools.keys().collect();
    pool_names.sort();

    for pool_name in pool_names {
        let pool = &config.pools[pool_name];
        write_single_pool(w, pool_name, pool);
    }

    write_pool_examples(w);
}

fn write_single_pool(w: &mut ConfigWriter, pool_name: &str, pool: &Pool) {
    let f = &*FIELDS;
    let fi = w.pool_field_indent();

    w.comment(fi, f.text("pool_example_title").get(w.russian));
    match w.format {
        ConfigFormat::Toml => {
            w.subsection(&format!("pools.{pool_name}"));
        }
        ConfigFormat::Yaml => {
            let _ = writeln!(w.output, "  {pool_name}:");
        }
    }

    // --- Server Connection Settings ---
    w.separator(fi, f.section_title("pool_server").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "pool", "server_host");
    w.kv(fi, "server_host", &w.str_val(&pool.server_host));
    w.blank();

    write_field_comment(w, fi, "pool", "server_port");
    w.kv(fi, "server_port", &w.num_val(pool.server_port));
    w.blank();

    write_field_desc(w, fi, "pool", "server_database");
    if let Some(ref db) = pool.server_database {
        w.kv(fi, "server_database", &w.str_val(db));
    } else {
        w.commented_kv(fi, "server_database", "\"actual_db_name\"");
    }
    w.blank();

    // --- Pool Settings ---
    w.separator(fi, f.section_title("pool_settings").get(w.russian));
    w.blank();

    write_field_comment(w, fi, "pool", "pool_mode");
    w.kv(fi, "pool_mode", &w.str_val(&pool.pool_mode.to_string()));
    w.blank();

    write_field_desc(w, fi, "pool", "connect_timeout");
    if let Some(val) = pool.connect_timeout {
        w.kv(fi, "connect_timeout", &w.num_val(val));
    } else {
        w.commented_kv(fi, "connect_timeout", "5000");
    }
    w.blank();

    write_field_desc(w, fi, "pool", "idle_timeout");
    if let Some(val) = pool.idle_timeout {
        w.kv(fi, "idle_timeout", &w.num_val(val));
    } else {
        w.commented_kv(fi, "idle_timeout", "300000");
    }
    w.blank();

    write_field_desc(w, fi, "pool", "server_lifetime");
    if let Some(val) = pool.server_lifetime {
        w.kv(fi, "server_lifetime", &w.num_val(val));
    } else {
        w.commented_kv(fi, "server_lifetime", "300000");
    }
    w.blank();

    write_field_comment(w, fi, "pool", "cleanup_server_connections");
    w.kv(
        fi,
        "cleanup_server_connections",
        &w.bool_val(pool.cleanup_server_connections),
    );
    w.blank();

    write_field_desc(w, fi, "pool", "prepared_statements_cache_size");
    if let Some(val) = pool.prepared_statements_cache_size {
        w.kv(fi, "prepared_statements_cache_size", &w.num_val(val));
    } else {
        w.commented_kv(fi, "prepared_statements_cache_size", "8192");
    }
    w.blank();

    // --- Application Settings ---
    w.separator(fi, f.section_title("pool_app").get(w.russian));
    w.blank();

    write_field_desc(w, fi, "pool", "application_name");
    if let Some(ref name) = pool.application_name {
        w.kv(fi, "application_name", &w.str_val(name));
    } else {
        w.commented_kv(fi, "application_name", "\"my_application\"");
    }
    w.blank();

    write_field_comment(w, fi, "pool", "log_client_parameter_status_changes");
    w.kv(
        fi,
        "log_client_parameter_status_changes",
        &w.bool_val(pool.log_client_parameter_status_changes),
    );
    w.blank();

    write_pool_users(w, pool_name, &pool.users);
}

fn write_pool_users(w: &mut ConfigWriter, pool_name: &str, users: &[User]) {
    let f = &*FIELDS;
    let fi = w.pool_field_indent();
    let ui = w.user_field_indent();

    match w.format {
        ConfigFormat::Toml => {
            w.separator(fi, f.section_title("pool_users_toml").get(w.russian));
            let en_msg = format!("Users are defined with numeric indices: [pools.{pool_name}.users.0], [pools.{pool_name}.users.1], etc.");
            let ru_msg = format!("Пользователи задаются с числовыми индексами: [pools.{pool_name}.users.0], [pools.{pool_name}.users.1] и т.д.");
            w.comment(fi, w.t(&en_msg, &ru_msg));
            w.comment(fi, f.text("pool_users_unique").get(w.russian));
        }
        ConfigFormat::Yaml => {
            w.separator(fi, f.section_title("pool_users_yaml").get(w.russian));
            w.comment(fi, f.text("pool_users_yaml_array").get(w.russian));
            w.comment(fi, f.text("pool_users_unique").get(w.russian));
        }
    }

    match w.format {
        ConfigFormat::Toml => {
            for (i, user) in users.iter().enumerate() {
                w.blank();
                w.subsection(&format!("pools.{pool_name}.users.{i}"));
                write_user_fields_toml(w, user, ui);
            }
        }
        ConfigFormat::Yaml => {
            let prefix = "  ".repeat(fi);
            let _ = writeln!(w.output, "{prefix}users:");
            for user in users {
                write_user_fields_yaml(w, user);
            }
        }
    }
}

fn write_user_fields_toml(w: &mut ConfigWriter, user: &User, fi: usize) {
    write_field_desc(w, fi, "user", "username");
    w.kv(fi, "username", &w.str_val(&user.username));
    w.blank();

    write_field_desc(w, fi, "user", "password");
    w.kv(fi, "password", &w.str_val(&user.password));
    w.blank();

    write_field_comment(w, fi, "user", "pool_size");
    w.kv(fi, "pool_size", &w.num_val(user.pool_size));
    w.blank();

    write_field_desc(w, fi, "user", "min_pool_size");
    if let Some(val) = user.min_pool_size {
        w.kv(fi, "min_pool_size", &w.num_val(val));
    } else {
        w.commented_kv(fi, "min_pool_size", "5");
    }
    w.blank();

    write_field_desc(w, fi, "user", "pool_mode");
    if let Some(ref mode) = user.pool_mode {
        w.kv(fi, "pool_mode", &w.str_val(&mode.to_string()));
    } else {
        w.commented_kv(fi, "pool_mode", "\"session\"");
    }
    w.blank();

    write_field_desc(w, fi, "user", "server_lifetime");
    if let Some(val) = user.server_lifetime {
        w.kv(fi, "server_lifetime", &w.num_val(val));
    } else {
        w.commented_kv(fi, "server_lifetime", "600000");
    }
    w.blank();

    // IMPORTANT: server_username/server_password with prominent docs
    write_server_credentials_comment(w, fi);
    if let Some(ref su) = user.server_username {
        w.kv(fi, "server_username", &w.str_val(su));
    } else {
        w.commented_kv(fi, "server_username", "\"actual_pg_user\"");
    }
    if let Some(ref sp) = user.server_password {
        w.kv(fi, "server_password", &w.str_val(sp));
    } else {
        w.commented_kv(fi, "server_password", "\"actual_pg_password\"");
    }
    w.blank();

    write_field_desc(w, fi, "user", "auth_pam_service");
    if let Some(ref pam) = user.auth_pam_service {
        w.kv(fi, "auth_pam_service", &w.str_val(pam));
    } else {
        w.commented_kv(fi, "auth_pam_service", "\"pg_doorman\"");
    }
}

fn write_user_fields_yaml(w: &mut ConfigWriter, user: &User) {
    let indent = "  ".repeat(3);
    let item_indent = "  ".repeat(2);

    // Username with "- " prefix
    let _ = writeln!(
        w.output,
        "{item_indent}    # {}",
        w.t(
            "Username for client authentication.",
            "Имя пользователя для аутентификации клиента."
        )
    );
    let _ = writeln!(w.output, "{item_indent}  - username: \"{}\"", user.username);
    w.blank();

    write_field_desc(w, 3, "user", "password");
    let _ = writeln!(w.output, "{indent}  password: \"{}\"", user.password);
    w.blank();

    write_field_comment(w, 3, "user", "pool_size");
    let _ = writeln!(w.output, "{indent}  pool_size: {}", user.pool_size);
    w.blank();

    write_field_desc(w, 3, "user", "min_pool_size");
    if let Some(val) = user.min_pool_size {
        let _ = writeln!(w.output, "{indent}  min_pool_size: {val}");
    } else {
        let _ = writeln!(w.output, "{indent}  # min_pool_size: 5");
    }
    w.blank();

    write_field_desc(w, 3, "user", "pool_mode");
    if let Some(ref mode) = user.pool_mode {
        let _ = writeln!(w.output, "{indent}  pool_mode: \"{mode}\"");
    } else {
        let _ = writeln!(w.output, "{indent}  # pool_mode: \"session\"");
    }
    w.blank();

    write_field_desc(w, 3, "user", "server_lifetime");
    if let Some(val) = user.server_lifetime {
        let _ = writeln!(w.output, "{indent}  server_lifetime: {val}");
    } else {
        let _ = writeln!(w.output, "{indent}  # server_lifetime: 600000");
    }
    w.blank();

    // IMPORTANT: server_username/server_password
    write_server_credentials_comment(w, 3);
    if let Some(ref su) = user.server_username {
        let _ = writeln!(w.output, "{indent}  server_username: \"{su}\"");
    } else {
        let _ = writeln!(w.output, "{indent}  # server_username: \"actual_pg_user\"");
    }
    if let Some(ref sp) = user.server_password {
        let _ = writeln!(w.output, "{indent}  server_password: \"{sp}\"");
    } else {
        let _ = writeln!(
            w.output,
            "{indent}  # server_password: \"actual_pg_password\""
        );
    }
    w.blank();

    write_field_desc(w, 3, "user", "auth_pam_service");
    if let Some(ref pam) = user.auth_pam_service {
        let _ = writeln!(w.output, "{indent}  auth_pam_service: \"{pam}\"");
    } else {
        let _ = writeln!(w.output, "{indent}  # auth_pam_service: \"pg_doorman\"");
    }
}

/// Write prominent documentation about server_username/server_password.
/// This is the #1 pain point for new users — kept inline for precise formatting.
fn write_server_credentials_comment(w: &mut ConfigWriter, indent: usize) {
    if w.russian {
        w.comment(
            indent,
            "Учётные данные для подключения к серверу PostgreSQL.",
        );
        w.comment(indent, "");
        w.comment(
            indent,
            "ВАЖНО: По умолчанию pg_doorman использует те же username и password",
        );
        w.comment(
            indent,
            "для аутентификации на сервере PostgreSQL. Если пароль клиента — это",
        );
        w.comment(
            indent,
            "хеш MD5/SCRAM (что типично при автогенерации), PostgreSQL ОТКЛОНИТ",
        );
        w.comment(
            indent,
            "подключение, потому что сервер ожидает настоящий пароль, а не хеш.",
        );
        w.comment(indent, "");
        w.comment(
            indent,
            "Решение: укажите server_username и server_password с реальными",
        );
        w.comment(
            indent,
            "учётными данными PostgreSQL (пароль открытым текстом).",
        );
        w.comment(indent, "Оба параметра должны быть указаны вместе.");
        w.comment(indent, "");
        w.comment(
            indent,
            "Пример: клиент аутентифицируется MD5-хешем, сервер — реальным паролем:",
        );
        w.comment(indent, "  username = \"app_user\"");
        w.comment(
            indent,
            "  password = \"md5...\"                    # для аутентификации клиента",
        );
        w.comment(
            indent,
            "  server_username = \"app_user\"            # для аутентификации на PostgreSQL",
        );
        w.comment(
            indent,
            "  server_password = \"настоящий_пароль\"    # пароль открытым текстом",
        );
    } else {
        w.comment(
            indent,
            "Server-side credentials for connecting to PostgreSQL.",
        );
        w.comment(indent, "");
        w.comment(
            indent,
            "IMPORTANT: By default pg_doorman uses the same username and password",
        );
        w.comment(
            indent,
            "to authenticate on the PostgreSQL server. If the client password is",
        );
        w.comment(
            indent,
            "an MD5/SCRAM hash (which is typical), PostgreSQL will REJECT it because",
        );
        w.comment(
            indent,
            "the server expects the real plaintext password, not a hash.",
        );
        w.comment(indent, "");
        w.comment(
            indent,
            "To fix this, set server_username and server_password to the actual",
        );
        w.comment(
            indent,
            "PostgreSQL credentials (plaintext password). Both must be specified together.",
        );
        w.comment(indent, "");
        w.comment(
            indent,
            "Example: client authenticates with MD5 hash, server uses real password:",
        );
        w.comment(indent, "  username = \"app_user\"");
        w.comment(
            indent,
            "  password = \"md5...\"                    # for client auth",
        );
        w.comment(
            indent,
            "  server_username = \"app_user\"            # for PostgreSQL auth",
        );
        w.comment(
            indent,
            "  server_password = \"real_password_here\"  # plaintext password",
        );
    }
}

fn write_pool_examples(w: &mut ConfigWriter) {
    let f = &*FIELDS;
    let fi = w.pool_field_indent();
    w.blank();
    w.separator(fi, f.section_title("pool_examples").get(w.russian));
    w.blank();

    match w.format {
        ConfigFormat::Toml => {
            w.comment(0, f.text("pool_example_multi").get(w.russian));
            w.comment(0, "[pools.multi_user_db]");
            w.comment(0, "server_host = \"192.168.1.100\"");
            w.comment(0, "server_port = 5432");
            w.comment(0, "pool_mode = \"transaction\"");
            w.comment(0, "");
            w.comment(0, "[pools.multi_user_db.users.0]");
            w.comment(0, "username = \"readonly_user\"");
            w.comment(0, "password = \"md5...\"");
            w.comment(0, "pool_size = 20");
            w.comment(0, "");
            w.comment(0, "[pools.multi_user_db.users.1]");
            w.comment(0, "username = \"readwrite_user\"");
            w.comment(0, "password = \"SCRAM-SHA-256$...\"");
            w.comment(0, "pool_size = 10");
            w.blank();

            w.comment(0, f.text("pool_example_unix").get(w.russian));
            w.comment(0, "[pools.local_db]");
            w.comment(0, "server_host = \"/var/run/postgresql\"");
            w.comment(0, "server_port = 5432");
            w.comment(0, "pool_mode = \"session\"");
            w.comment(0, "");
            w.comment(0, "[pools.local_db.users.0]");
            w.comment(0, "username = \"local_user\"");
            w.comment(0, "password = \"md5...\"");
            w.comment(0, "pool_size = 50");
            w.blank();

            w.comment(0, f.text("pool_example_jwt").get(w.russian));
            w.comment(0, "[pools.jwt_auth_db]");
            w.comment(0, "server_host = \"127.0.0.1\"");
            w.comment(0, "server_port = 5432");
            w.comment(0, "pool_mode = \"transaction\"");
            w.comment(0, "");
            w.comment(0, "[pools.jwt_auth_db.users.0]");
            w.comment(0, "username = \"jwt_user\"");
            w.comment(
                0,
                "password = \"jwt-pkey-fpath:/etc/pg_doorman/jwt/public.pem\"",
            );
            w.comment(0, "pool_size = 30");
            w.comment(0, "server_username = \"actual_db_user\"");
            w.comment(0, "server_password = \"actual_password\"");
            w.blank();

            w.comment(0, f.text("pool_example_mapped").get(w.russian));
            w.comment(0, "[pools.mapped_db]");
            w.comment(0, "server_host = \"db.example.com\"");
            w.comment(0, "server_port = 5432");
            w.comment(0, "server_database = \"production\"");
            w.comment(0, "pool_mode = \"transaction\"");
            w.comment(0, "");
            w.comment(0, "[pools.mapped_db.users.0]");
            w.comment(0, "username = \"app_user\"");
            w.comment(0, "password = \"md5...\"");
            w.comment(0, "pool_size = 40");
            w.comment(0, "server_username = \"pg_app_user\"");
            w.comment(0, "server_password = \"secure_password\"");
        }
        ConfigFormat::Yaml => {
            w.comment(1, f.text("pool_example_multi").get(w.russian));
            w.comment(1, "multi_user_db:");
            w.comment(1, "  server_host: \"192.168.1.100\"");
            w.comment(1, "  server_port: 5432");
            w.comment(1, "  pool_mode: \"transaction\"");
            w.comment(1, "  users:");
            w.comment(1, "    - username: \"readonly_user\"");
            w.comment(1, "      password: \"md5...\"");
            w.comment(1, "      pool_size: 20");
            w.comment(1, "    - username: \"readwrite_user\"");
            w.comment(1, "      password: \"SCRAM-SHA-256$...\"");
            w.comment(1, "      pool_size: 10");
            w.blank();

            w.comment(1, f.text("pool_example_unix").get(w.russian));
            w.comment(1, "local_db:");
            w.comment(1, "  server_host: \"/var/run/postgresql\"");
            w.comment(1, "  server_port: 5432");
            w.comment(1, "  pool_mode: \"session\"");
            w.comment(1, "  users:");
            w.comment(1, "    - username: \"local_user\"");
            w.comment(1, "      password: \"md5...\"");
            w.comment(1, "      pool_size: 50");
            w.blank();

            w.comment(1, f.text("pool_example_jwt").get(w.russian));
            w.comment(1, "jwt_auth_db:");
            w.comment(1, "  server_host: \"127.0.0.1\"");
            w.comment(1, "  server_port: 5432");
            w.comment(1, "  pool_mode: \"transaction\"");
            w.comment(1, "  users:");
            w.comment(1, "    - username: \"jwt_user\"");
            w.comment(
                1,
                "      password: \"jwt-pkey-fpath:/etc/pg_doorman/jwt/public.pem\"",
            );
            w.comment(1, "      pool_size: 30");
            w.comment(1, "      server_username: \"actual_db_user\"");
            w.comment(1, "      server_password: \"actual_password\"");
            w.blank();

            w.comment(1, f.text("pool_example_mapped").get(w.russian));
            w.comment(1, "mapped_db:");
            w.comment(1, "  server_host: \"db.example.com\"");
            w.comment(1, "  server_port: 5432");
            w.comment(1, "  server_database: \"production\"");
            w.comment(1, "  pool_mode: \"transaction\"");
            w.comment(1, "  users:");
            w.comment(1, "    - username: \"app_user\"");
            w.comment(1, "      password: \"md5...\"");
            w.comment(1, "      pool_size: 40");
            w.comment(1, "      server_username: \"pg_app_user\"");
            w.comment(1, "      server_password: \"secure_password\"");
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers for duration/byte_size fields with format-specific rendering
// ---------------------------------------------------------------------------

fn write_duration_value(
    w: &mut ConfigWriter,
    indent: usize,
    key: &str,
    millis: u64,
    human_readable: &str,
    millis_str: &str,
) {
    match w.format {
        ConfigFormat::Toml => {
            if millis_str.is_empty() {
                w.comment(indent, &format!("Default: \"{human_readable}\""));
            } else {
                w.comment(indent, &format!("Default: {millis} ({millis_str})"));
            }
            w.kv(indent, key, &w.num_val(millis));
        }
        ConfigFormat::Yaml => {
            w.comment(
                indent,
                &format!(
                    "Supports human-readable format: \"{human_readable}\", \"{millis}ms\", or {millis} (milliseconds)"
                ),
            );
            if millis_str.is_empty() {
                w.comment(indent, &format!("Default: \"{human_readable}\""));
            } else {
                w.comment(
                    indent,
                    &format!("Default: \"{human_readable}\" ({millis_str})"),
                );
            }
            w.kv(indent, key, &w.str_val(human_readable));
        }
    }
    w.blank();
}

fn write_byte_size_value(
    w: &mut ConfigWriter,
    indent: usize,
    key: &str,
    bytes: u64,
    human_readable: &str,
    bytes_str: &str,
) {
    match w.format {
        ConfigFormat::Toml => {
            w.comment(indent, &format!("Default: {bytes} ({bytes_str})"));
            w.kv(indent, key, &w.num_val(bytes));
        }
        ConfigFormat::Yaml => {
            w.comment(
                indent,
                &format!(
                    "Supports human-readable format: \"{human_readable}\", \"{}\", or {bytes} (bytes)",
                    human_readable.replace("MB", "M").replace("GB", "G"),
                ),
            );
            w.comment(
                indent,
                &format!("Default: \"{human_readable}\" ({bytes_str})"),
            );
            w.kv(indent, key, &w.str_val(human_readable));
        }
    }
    w.blank();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fields_yaml_parses() {
        // Force parsing of fields.yaml and verify it doesn't panic
        let _ = &*FIELDS;
    }

    #[test]
    fn test_reference_config_toml_is_parseable() {
        let toml_str = generate_reference_config(ConfigFormat::Toml, false);
        let clean: String = toml_str
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && !l.is_empty())
            .collect::<Vec<&str>>()
            .join("\n");
        let result: Result<Config, _> = toml::from_str(&clean);
        assert!(
            result.is_ok(),
            "Generated TOML reference config is not parseable: {:?}\n---\n{clean}",
            result.err()
        );
    }

    #[test]
    fn test_reference_config_yaml_is_parseable() {
        let yaml_str = generate_reference_config(ConfigFormat::Yaml, false);
        let clean: String = yaml_str
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<&str>>()
            .join("\n");
        let result: Result<Config, _> = serde_yaml::from_str(&clean);
        assert!(
            result.is_ok(),
            "Generated YAML reference config is not parseable: {:?}\n---\n{clean}",
            result.err()
        );
    }

    #[test]
    fn test_reference_config_toml_matches_file() {
        let generated = generate_reference_config(ConfigFormat::Toml, false);
        let file_content = include_str!("../../../pg_doorman.toml");
        if generated != file_content {
            panic!(
                "Reference TOML config is outdated. Run: cargo run -- generate --reference -o pg_doorman.toml"
            );
        }
    }

    #[test]
    fn test_reference_config_yaml_matches_file() {
        let generated = generate_reference_config(ConfigFormat::Yaml, false);
        let file_content = include_str!("../../../pg_doorman.yaml");
        if generated != file_content {
            panic!(
                "Reference YAML config is outdated. Run: cargo run -- generate --reference -o pg_doorman.yaml"
            );
        }
    }

    #[test]
    fn test_russian_reference_config_toml_is_parseable() {
        let toml_str = generate_reference_config(ConfigFormat::Toml, true);
        let clean: String = toml_str
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && !l.is_empty())
            .collect::<Vec<&str>>()
            .join("\n");
        let result: Result<Config, _> = toml::from_str(&clean);
        assert!(
            result.is_ok(),
            "Generated Russian TOML reference config is not parseable: {:?}\n---\n{clean}",
            result.err()
        );
    }

    /// Extract public field names from a Rust source file.
    /// Matches lines like `pub field_name: Type` inside struct bodies.
    fn extract_pub_fields(source: &str) -> Vec<String> {
        source
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                // Must start with "pub " and contain ":", but not be a function/struct/etc
                if !trimmed.starts_with("pub ") || !trimmed.contains(':') {
                    return None;
                }
                // Skip pub fn, pub mod, pub struct, pub enum, pub use, pub type, pub const
                if trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub mod ")
                    || trimmed.starts_with("pub struct ")
                    || trimmed.starts_with("pub enum ")
                    || trimmed.starts_with("pub use ")
                    || trimmed.starts_with("pub type ")
                    || trimmed.starts_with("pub const ")
                {
                    return None;
                }
                // "pub field_name: Type" -> extract "field_name"
                let after_pub = trimmed.strip_prefix("pub ")?;
                // Handle "pub(crate) field: Type" style
                let after_vis = if after_pub.starts_with('(') {
                    let paren_end = after_pub.find(')')?;
                    after_pub[paren_end + 1..].trim()
                } else {
                    after_pub
                };
                let field_name = after_vis.split(':').next()?.trim();
                if field_name.contains('(') || field_name.is_empty() {
                    return None;
                }
                Some(field_name.to_string())
            })
            .collect()
    }

    /// Verify that all public fields from config source files appear in the
    /// generated annotated config. This catches missing fields when someone
    /// adds a new config parameter but forgets to add it to annotated.rs.
    #[test]
    fn test_annotated_config_covers_all_config_fields() {
        let annotated_toml = generate_reference_config(ConfigFormat::Toml, false);
        let annotated_yaml = generate_reference_config(ConfigFormat::Yaml, false);
        let combined = format!("{annotated_toml}\n{annotated_yaml}");

        // Fields that are internal/structural and not config parameters
        let skip_fields: &[&str] = &[
            "users", // structural: rendered as a sub-section, not a scalar field
            "pools", // structural: rendered as a section
            "path",  // internal: not a config parameter
        ];

        let sources: &[(&str, &str)] = &[
            ("General", include_str!("../../config/general.rs")),
            ("Pool", include_str!("../../config/pool.rs")),
            ("User", include_str!("../../config/user.rs")),
            ("Prometheus", include_str!("../../config/prometheus.rs")),
            ("Talos", include_str!("../../config/talos.rs")),
            ("Include", include_str!("../../config/include.rs")),
        ];

        let mut missing = Vec::new();

        for (struct_name, source) in sources {
            let fields = extract_pub_fields(source);
            for field in &fields {
                if skip_fields.contains(&field.as_str()) {
                    continue;
                }
                if !combined.contains(field.as_str()) {
                    missing.push(format!("{struct_name}::{field}"));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "The following config fields are NOT covered in annotated config generation.\n\
             Add them to src/app/generate/annotated.rs:\n  - {}\n\n\
             After adding, regenerate reference configs:\n  \
             cargo run --bin pg_doorman -- generate --reference -o pg_doorman.toml\n  \
             cargo run --bin pg_doorman -- generate --reference -o pg_doorman.yaml",
            missing.join("\n  - ")
        );
    }

    #[test]
    fn test_russian_reference_config_yaml_is_parseable() {
        let yaml_str = generate_reference_config(ConfigFormat::Yaml, true);
        let clean: String = yaml_str
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<&str>>()
            .join("\n");
        let result: Result<Config, _> = serde_yaml::from_str(&clean);
        assert!(
            result.is_ok(),
            "Generated Russian YAML reference config is not parseable: {:?}\n---\n{clean}",
            result.err()
        );
    }

    /// Verify that all config fields from source structs appear in fields.yaml.
    #[test]
    fn test_fields_yaml_covers_all_config_fields() {
        let fields = &*FIELDS;
        let yaml_content = include_str!("fields.yaml");

        // Fields that are internal/structural and not config parameters
        let skip_fields: &[&str] = &[
            "users",
            "pools",
            "path",
            "pooler_check_query_request_bytes", // internal, derived
        ];

        let sources: &[(&str, &str)] = &[
            ("General", include_str!("../../config/general.rs")),
            ("Pool", include_str!("../../config/pool.rs")),
            ("User", include_str!("../../config/user.rs")),
            ("Prometheus", include_str!("../../config/prometheus.rs")),
        ];

        let section_map: &[(&str, &str)] = &[
            ("General", "general"),
            ("Pool", "pool"),
            ("User", "user"),
            ("Prometheus", "prometheus"),
        ];

        let mut missing = Vec::new();

        for (struct_name, source) in sources {
            let pub_fields = extract_pub_fields(source);
            let section = section_map
                .iter()
                .find(|(s, _)| s == struct_name)
                .map(|(_, sec)| *sec);

            for field in &pub_fields {
                if skip_fields.contains(&field.as_str()) {
                    continue;
                }
                // Check if field exists in YAML (either as a field key or in raw content)
                let in_yaml = if let Some(sec) = section {
                    let map = match sec {
                        "general" => &fields.fields.general,
                        "pool" => &fields.fields.pool,
                        "user" => &fields.fields.user,
                        "prometheus" => &fields.fields.prometheus,
                        _ => unreachable!(),
                    };
                    map.contains_key(field.as_str())
                } else {
                    yaml_content.contains(field.as_str())
                };

                if !in_yaml {
                    missing.push(format!("{struct_name}::{field}"));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "The following config fields are NOT covered in fields.yaml.\n\
             Add them to src/app/generate/fields.yaml:\n  - {}",
            missing.join("\n  - ")
        );
    }
}
