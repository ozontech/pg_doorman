//! Annotated config generation with comments and field documentation.
//!
//! This module generates fully documented configuration files (TOML and YAML)
//! with inline comments for every field. It serves as the single source of truth
//! for config documentation.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::config::{Config, ConfigFormat, Pool, PoolMode, Prometheus, User};

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
    w.comment(
        0,
        w.t(
            "IMPORTANT: Use ONLY ONE configuration file format (YAML or TOML), not both.",
            "ВАЖНО: Используйте ТОЛЬКО ОДИН формат конфигурации (YAML или TOML), не оба.",
        ),
    );
    w.comment(
        0,
        w.t(
            "YAML format (.yaml, .yml) is recommended for new configurations.",
            "Формат YAML (.yaml, .yml) рекомендуется для новых конфигураций.",
        ),
    );
    w.comment(
        0,
        w.t(
            "TOML format (.toml) is supported for backward compatibility.",
            "Формат TOML (.toml) поддерживается для обратной совместимости.",
        ),
    );
    w.comment(
        0,
        w.t(
            "The format is automatically detected based on the file extension.",
            "Формат определяется автоматически по расширению файла.",
        ),
    );
    w.comment(
        0,
        "============================================================================",
    );

    if w.format == ConfigFormat::Yaml {
        w.comment(0, "");
        w.comment(0, w.t("HUMAN-READABLE VALUES", "ЧЕЛОВЕКОЧИТАЕМЫЕ ЗНАЧЕНИЯ"));
        w.comment(
            0,
            "============================================================================",
        );
        w.comment(
            0,
            w.t(
                "pg_doorman supports human-readable formats for duration and byte size values.",
                "pg_doorman поддерживает человекочитаемые форматы для значений времени и размера.",
            ),
        );
        w.comment(
            0,
            w.t(
                "Both numeric values (for backward compatibility) and string formats are supported.",
                "Поддерживаются как числовые значения (для совместимости), так и строковые форматы.",
            ),
        );
        w.comment(0, "");
        w.comment(0, w.t("Duration formats:", "Форматы времени:"));
        w.comment(
            0,
            w.t(
                "  - Plain numbers: interpreted as milliseconds (e.g., 5000 = 5 seconds)",
                "  - Числа: интерпретируются как миллисекунды (напр., 5000 = 5 секунд)",
            ),
        );
        w.comment(0, "  - \"Nms\" : milliseconds (e.g., \"100ms\")");
        w.comment(
            0,
            w.t(
                "  - \"Ns\"  : seconds (e.g., \"5s\" = 5000 milliseconds)",
                "  - \"Ns\"  : секунды (напр., \"5s\" = 5000 миллисекунд)",
            ),
        );
        w.comment(
            0,
            w.t(
                "  - \"Nm\"  : minutes (e.g., \"5m\" = 300000 milliseconds)",
                "  - \"Nm\"  : минуты (напр., \"5m\" = 300000 миллисекунд)",
            ),
        );
        w.comment(
            0,
            w.t(
                "  - \"Nh\"  : hours (e.g., \"1h\" = 3600000 milliseconds)",
                "  - \"Nh\"  : часы (напр., \"1h\" = 3600000 миллисекунд)",
            ),
        );
        w.comment(
            0,
            w.t(
                "  - \"Nd\"  : days (e.g., \"1d\" = 86400000 milliseconds)",
                "  - \"Nd\"  : дни (напр., \"1d\" = 86400000 миллисекунд)",
            ),
        );
        w.comment(0, "");
        w.comment(0, w.t("Byte size formats:", "Форматы размера:"));
        w.comment(
            0,
            w.t(
                "  - Plain numbers: interpreted as bytes (e.g., 1048576 = 1 MB)",
                "  - Числа: интерпретируются как байты (напр., 1048576 = 1 МБ)",
            ),
        );
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
        w.comment(0, w.t("Examples:", "Примеры:"));
        w.comment(
            0,
            w.t(
                "  connect_timeout: \"3s\"        # instead of 3000",
                "  connect_timeout: \"3s\"        # вместо 3000",
            ),
        );
        w.comment(
            0,
            w.t(
                "  idle_timeout: \"5m\"           # instead of 300000",
                "  idle_timeout: \"5m\"           # вместо 300000",
            ),
        );
        w.comment(
            0,
            w.t(
                "  max_memory_usage: \"256MB\"    # instead of 268435456",
                "  max_memory_usage: \"256MB\"    # вместо 268435456",
            ),
        );
        w.comment(
            0,
            "============================================================================",
        );
    }
    w.blank();
}

fn write_include_section(w: &mut ConfigWriter) {
    w.comment(
        0,
        w.t(
            "Include additional configuration files.",
            "Подключение дополнительных файлов конфигурации.",
        ),
    );
    w.comment(
        0,
        w.t(
            "Files are merged in order, allowing modular configuration.",
            "Файлы объединяются по порядку, что позволяет собирать конфиг из частей.",
        ),
    );
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
    let g = &config.general;
    w.major_separator(w.t("GENERAL SETTINGS", "ОСНОВНЫЕ НАСТРОЙКИ"));
    w.section(0, "general");

    let fi = w.field_indent();

    // --- Network Settings ---
    w.separator(fi, w.t("Network Settings", "Сетевые настройки"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Listen host for incoming connections (IPv4 only).",
            "Адрес для приёма входящих подключений (только IPv4).",
        ),
    );
    w.comment(fi, "Default: \"0.0.0.0\"");
    w.kv(fi, "host", &w.str_val(&g.host));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Listen port for incoming connections.",
            "Порт для приёма входящих подключений.",
        ),
    );
    w.comment(fi, "Default: 5432");
    w.kv(fi, "port", &w.num_val(g.port));
    w.blank();

    w.comment(
        fi,
        w.t(
            "TCP backlog for incoming connections.",
            "TCP backlog для входящих подключений.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "A value of zero sets max_connections as the TCP backlog value.",
            "Значение 0 использует max_connections как значение TCP backlog.",
        ),
    );
    w.comment(fi, "Default: 0");
    w.kv(fi, "backlog", &w.num_val(g.backlog));
    w.blank();

    // --- Connection Timeouts ---
    w.separator(fi, w.t("Connection Timeouts", "Таймауты подключений"));
    w.blank();

    write_duration_field(
        w,
        fi,
        "connect_timeout",
        g.connect_timeout.as_millis(),
        w.t(
            "Connection timeout to server.",
            "Таймаут подключения к серверу.",
        ),
        "3s",
        "3000 ms",
    );

    write_duration_field(
        w,
        fi,
        "query_wait_timeout",
        g.query_wait_timeout.as_millis(),
        w.t(
            "Maximum time to wait for a query to complete.\n# Analog of query_wait_timeout in PgBouncer.",
            "Максимальное время ожидания выполнения запроса.\n# Аналог query_wait_timeout в PgBouncer.",
        ),
        "5s",
        "5000 ms",
    );

    write_duration_field(
        w,
        fi,
        "idle_timeout",
        g.idle_timeout.as_millis(),
        w.t(
            "Server idle timeout.",
            "Таймаут простоя серверного соединения.",
        ),
        "5m",
        "300000 ms",
    );

    write_duration_field(
        w,
        fi,
        "server_lifetime",
        g.server_lifetime.as_millis(),
        w.t(
            "Server lifetime. Only applied to idle connections.",
            "Время жизни серверного соединения. Применяется только к простаивающим соединениям.",
        ),
        "5m",
        "300000 ms",
    );

    write_duration_field(
        w,
        fi,
        "retain_connections_time",
        g.retain_connections_time.as_millis(),
        w.t(
            "Interval for checking and closing idle connections.",
            "Интервал проверки и закрытия простаивающих соединений.",
        ),
        "30s",
        "30000 ms",
    );

    w.comment(
        fi,
        w.t(
            "Maximum number of idle connections to close per retain cycle.",
            "Максимальное количество простаивающих соединений, закрываемых за один цикл.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "0 means unlimited (close all idle connections that exceed timeout).",
            "0 — без ограничений (закрывать все соединения, превысившие таймаут).",
        ),
    );
    w.comment(fi, "Default: 3");
    w.kv(
        fi,
        "retain_connections_max",
        &w.num_val(g.retain_connections_max),
    );
    w.blank();

    write_duration_field(
        w,
        fi,
        "server_idle_check_timeout",
        g.server_idle_check_timeout.as_millis(),
        w.t(
            "Time after which an idle server connection should be checked before being\n# given to a client. This helps detect dead connections caused by PostgreSQL\n# restart, network issues, or server-side idle timeouts.\n# 0 means disabled (no check).",
            "Время простоя серверного соединения, после которого оно проверяется перед\n# передачей клиенту. Помогает обнаружить мёртвые соединения из-за перезапуска\n# PostgreSQL, сетевых проблем или серверных таймаутов.\n# 0 — проверка отключена.",
        ),
        "60s",
        "",
    );

    write_duration_field(
        w,
        fi,
        "shutdown_timeout",
        g.shutdown_timeout.as_millis(),
        w.t(
            "Graceful shutdown timeout.",
            "Таймаут корректного завершения работы.",
        ),
        "10s",
        "10000 ms",
    );

    write_duration_field(
        w,
        fi,
        "proxy_copy_data_timeout",
        g.proxy_copy_data_timeout.as_millis(),
        w.t(
            "Timeout for COPY data operations.",
            "Таймаут операций COPY.",
        ),
        "15s",
        "15000 ms",
    );

    // --- TCP Settings ---
    w.separator(fi, w.t("TCP Settings", "Настройки TCP"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "TCP keepalive settings (in seconds).",
            "Настройки TCP keepalive (в секундах).",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Keepalive is enabled by default and overwrites OS defaults.",
            "Keepalive включён по умолчанию и перезаписывает настройки ОС.",
        ),
    );
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

    w.comment(fi, "TCP SO_LINGER setting.");
    w.comment(
        fi,
        w.t(
            "By default, pg_doorman sends RST instead of keeping the connection open.",
            "По умолчанию pg_doorman отправляет RST вместо удержания соединения.",
        ),
    );
    w.comment(fi, "Default: 0");
    w.kv(fi, "tcp_so_linger", &w.num_val(g.tcp_so_linger));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Enable TCP_NODELAY to disable Nagle's algorithm for lower latency.",
            "Включить TCP_NODELAY для отключения алгоритма Нагла (меньше задержка).",
        ),
    );
    w.comment(fi, "Default: true");
    w.kv(fi, "tcp_no_delay", &w.bool_val(g.tcp_no_delay));
    w.blank();

    w.comment(
        fi,
        w.t(
            "TCP_USER_TIMEOUT for client connections (in seconds).",
            "TCP_USER_TIMEOUT для клиентских соединений (в секундах).",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Helps detect dead connections faster when data remains unacknowledged.",
            "Помогает быстрее обнаружить мёртвые соединения при неподтверждённых данных.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Only supported on Linux. Set to 0 to disable.",
            "Поддерживается только на Linux. 0 — отключено.",
        ),
    );
    w.comment(fi, "Default: 60");
    w.kv(fi, "tcp_user_timeout", &w.num_val(g.tcp_user_timeout));
    w.blank();

    write_byte_size_field(
        w,
        fi,
        "unix_socket_buffer_size",
        g.unix_socket_buffer_size.as_bytes(),
        w.t(
            "Buffer size for read/write operations when connecting via unix socket.",
            "Размер буфера для чтения/записи при подключении через unix socket.",
        ),
        "1MB",
        "1048576 bytes",
    );

    // --- Connection Limits ---
    w.separator(fi, w.t("Connection Limits", "Лимиты подключений"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Maximum number of clients that can connect simultaneously.",
            "Максимальное количество одновременных клиентских подключений.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "When reached, clients receive error code 53300: \"sorry, too many clients already\"",
            "При превышении клиент получит ошибку 53300: \"sorry, too many clients already\"",
        ),
    );
    w.comment(fi, "Default: 8192");
    w.kv(fi, "max_connections", &w.num_val(g.max_connections));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Maximum number of server connections that can be created concurrently.",
            "Максимальное количество серверных соединений, создаваемых одновременно.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Uses a semaphore to limit parallel connection creation.",
            "Использует семафор для ограничения параллельного создания соединений.",
        ),
    );
    w.comment(fi, "Default: 4");
    w.kv(
        fi,
        "max_concurrent_creates",
        &w.num_val(g.max_concurrent_creates),
    );
    w.blank();

    write_byte_size_field(
        w,
        fi,
        "max_memory_usage",
        g.max_memory_usage.as_bytes(),
        w.t(
            "Maximum memory usage for internal buffers.\n# If exceeded, clients receive an error.",
            "Максимальное использование памяти для внутренних буферов.\n# При превышении клиенты получат ошибку.",
        ),
        "256MB",
        "268435456 bytes",
    );

    // --- Logging ---
    w.separator(fi, w.t("Logging", "Логирование"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Log client connections for monitoring.",
            "Логировать подключения клиентов.",
        ),
    );
    w.comment(fi, "Default: true");
    w.kv(
        fi,
        "log_client_connections",
        &w.bool_val(g.log_client_connections),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Log client disconnections for monitoring.",
            "Логировать отключения клиентов.",
        ),
    );
    w.comment(fi, "Default: true");
    w.kv(
        fi,
        "log_client_disconnections",
        &w.bool_val(g.log_client_disconnections),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Syslog program name. When specified, pg_doorman sends messages to syslog.",
            "Имя программы для syslog. Если указано, pg_doorman отправляет логи в syslog.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Comment out to log to stdout.",
            "Закомментируйте для вывода логов в stdout.",
        ),
    );
    w.comment(fi, "Default: None");
    if let Some(ref name) = g.syslog_prog_name {
        w.kv(fi, "syslog_prog_name", &w.str_val(name));
    } else {
        w.commented_kv(fi, "syslog_prog_name", &w.str_val("pg_doorman"));
    }
    w.blank();

    // --- Worker Settings ---
    w.separator(fi, w.t("Worker Settings", "Настройки воркеров"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Number of worker threads for async client handling.",
            "Количество рабочих потоков для асинхронной обработки клиентов.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "More workers = better performance, up to CPU count.",
            "Больше воркеров = выше производительность, но не больше числа CPU.",
        ),
    );
    w.comment(fi, "Default: 4");
    w.kv(fi, "worker_threads", &w.num_val(g.worker_threads));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Automatically pin workers to different CPUs.",
            "Автоматически привязывать воркеры к разным CPU.",
        ),
    );
    w.comment(fi, "Default: false");
    w.kv(
        fi,
        "worker_cpu_affinity_pinning",
        &w.bool_val(g.worker_cpu_affinity_pinning),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Tokio runtime settings (advanced, change only if you understand the implications).",
            "Настройки Tokio runtime (продвинутые, меняйте только если понимаете последствия).",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Modern tokio versions handle these well by default, so these parameters are optional.",
            "Современные версии tokio хорошо справляются по умолчанию, поэтому параметры опциональны.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Uncomment only if you need to override tokio's defaults.",
            "Раскомментируйте, только если нужно переопределить значения по умолчанию.",
        ),
    );
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
            w.comment(
                fi,
                w.t(
                    "Stack size for each worker thread.",
                    "Размер стека каждого рабочего потока.",
                ),
            );
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

    w.comment(
        fi,
        w.t(
            "Maximum number of threads for blocking operations.",
            "Максимальное количество потоков для блокирующих операций.",
        ),
    );
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

    w.comment(
        fi,
        w.t(
            "Controls how often the scheduler checks the global task queue.",
            "Как часто планировщик проверяет глобальную очередь задач.",
        ),
    );
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

    w.comment(
        fi,
        w.t(
            "Controls how often the scheduler checks for external events (I/O, timers).",
            "Как часто планировщик проверяет внешние события (I/O, таймеры).",
        ),
    );
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
    w.separator(fi, w.t("Pool Behavior", "Поведение пула"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Server selection strategy in transaction pool mode.",
            "Стратегия выбора сервера в режиме пула транзакций.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "false = LRU (Least Recently Used) - better performance",
            "false = LRU (наименее недавно использованный) — лучшая производительность",
        ),
    );
    w.comment(fi, "true = Round Robin");
    w.comment(fi, "Default: false");
    w.kv(fi, "server_round_robin", &w.bool_val(g.server_round_robin));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Sync server parameters (SET commands, application_name) across backends.",
            "Синхронизировать серверные параметры (SET, application_name) между бэкендами.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Disabled by default due to performance impact.",
            "Отключено по умолчанию из-за влияния на производительность.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Consider using pool-level application_name instead.",
            "Рекомендуется использовать application_name на уровне пула.",
        ),
    );
    w.comment(fi, "Default: false");
    w.kv(
        fi,
        "sync_server_parameters",
        &w.bool_val(g.sync_server_parameters),
    );
    w.blank();

    write_byte_size_field(
        w,
        fi,
        "message_size_to_be_stream",
        g.message_size_to_be_stream.as_bytes(),
        w.t(
            "Data responses larger than this value are transmitted in chunks.",
            "Ответы данных больше этого значения передаются частями (потоково).",
        ),
        "1MB",
        "1048576 bytes",
    );

    w.comment(
        fi,
        w.t(
            "Query that won't be sent to server (used for connection health checks).",
            "Запрос, который не отправляется на сервер (для проверки живости соединения).",
        ),
    );
    w.comment(fi, "Default: \";\"");
    w.kv(fi, "pooler_check_query", &w.str_val(&g.pooler_check_query));
    w.blank();

    // --- Prepared Statements ---
    w.separator(
        fi,
        w.t(
            "Prepared Statements",
            "Подготовленные запросы (Prepared Statements)",
        ),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Enable caching of prepared statements.",
            "Включить кеширование подготовленных запросов.",
        ),
    );
    w.comment(fi, "Default: true");
    w.kv(
        fi,
        "prepared_statements",
        &w.bool_val(g.prepared_statements),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Cache size for prepared statements at the pool level (shared across clients).",
            "Размер кеша подготовленных запросов на уровне пула (общий для всех клиентов).",
        ),
    );
    w.comment(fi, "Default: 8192");
    w.kv(
        fi,
        "prepared_statements_cache_size",
        &w.num_val(g.prepared_statements_cache_size),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Maximum prepared statements cached per client connection.",
            "Максимум подготовленных запросов в кеше для одного клиентского соединения.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Protection against malicious clients that don't call DEALLOCATE.",
            "Защита от вредоносных клиентов, которые не вызывают DEALLOCATE.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Set to 0 for unlimited (relies on client calling DEALLOCATE).",
            "0 — без ограничений (полагается на вызов DEALLOCATE клиентом).",
        ),
    );
    w.comment(fi, "Default: 0 (unlimited)");
    w.kv(
        fi,
        "client_prepared_statements_cache_size",
        &w.num_val(g.client_prepared_statements_cache_size),
    );
    w.blank();

    // --- Admin Console ---
    w.separator(fi, w.t("Admin Console", "Консоль администратора"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Admin username for the virtual admin database (pgdoorman).",
            "Имя администратора для виртуальной базы данных pgdoorman.",
        ),
    );
    w.comment(fi, "Default: \"admin\"");
    w.kv(fi, "admin_username", &w.str_val(&g.admin_username));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Admin password for the virtual admin database.",
            "Пароль администратора для виртуальной базы данных.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "IMPORTANT: Change this in production!",
            "ВАЖНО: Обязательно смените пароль в продакшене!",
        ),
    );
    w.comment(fi, "Default: \"admin\"");
    w.kv(fi, "admin_password", &w.str_val(&g.admin_password));
    w.blank();

    // --- TLS Settings (Client-facing) ---
    w.separator(
        fi,
        w.t(
            "TLS Settings (Client-facing)",
            "Настройки TLS (для клиентов)",
        ),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Path to the TLS certificate file for incoming client connections.",
            "Путь к файлу TLS-сертификата для входящих клиентских подключений.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Must be used together with tls_private_key.",
            "Должен использоваться вместе с tls_private_key.",
        ),
    );
    if let Some(ref cert) = g.tls_certificate {
        w.kv(fi, "tls_certificate", &w.str_val(cert));
    } else {
        w.commented_kv(fi, "tls_certificate", "\"/etc/pg_doorman/server.crt\"");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Path to the TLS private key file for incoming client connections.",
            "Путь к файлу приватного ключа TLS для входящих клиентских подключений.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Must be used together with tls_certificate.",
            "Должен использоваться вместе с tls_certificate.",
        ),
    );
    if let Some(ref key) = g.tls_private_key {
        w.kv(fi, "tls_private_key", &w.str_val(key));
    } else {
        w.commented_kv(fi, "tls_private_key", "\"/etc/pg_doorman/server.key\"");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Path to the CA certificate for client certificate verification.",
            "Путь к CA-сертификату для верификации клиентских сертификатов.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Used with tls_mode = \"verify-full\"",
            "Используется с tls_mode = \"verify-full\"",
        ),
    );
    if let Some(ref ca) = g.tls_ca_cert {
        w.kv(fi, "tls_ca_cert", &w.str_val(ca));
    } else {
        w.commented_kv(fi, "tls_ca_cert", "\"/etc/pg_doorman/ca.crt\"");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "TLS mode for incoming connections:",
            "Режим TLS для входящих подключений:",
        ),
    );
    w.comment(
        fi,
        w.t(
            "- \"allow\"       : TLS allowed but not required (default)",
            "- \"allow\"       : TLS разрешён, но не обязателен (по умолчанию)",
        ),
    );
    w.comment(
        fi,
        w.t(
            "- \"disable\"     : TLS not allowed",
            "- \"disable\"     : TLS запрещён",
        ),
    );
    w.comment(
        fi,
        w.t(
            "- \"require\"     : TLS required",
            "- \"require\"     : TLS обязателен",
        ),
    );
    w.comment(
        fi,
        w.t(
            "- \"verify-full\" : TLS required with client certificate verification",
            "- \"verify-full\" : TLS обязателен с проверкой клиентского сертификата",
        ),
    );
    w.comment(fi, "Default: \"allow\"");
    if let Some(ref mode) = g.tls_mode {
        w.kv(fi, "tls_mode", &w.str_val(mode));
    } else {
        w.kv(fi, "tls_mode", &w.str_val("allow"));
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Limit simultaneous TLS session creation attempts.",
            "Ограничение на одновременное создание TLS-сессий.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Useful for applications with many connections at startup (\"hot start\").",
            "Полезно для приложений с множеством подключений при старте (\"горячий старт\").",
        ),
    );
    w.comment(fi, w.t("0 = no limit", "0 — без ограничений"));
    w.comment(fi, "Default: 0");
    w.kv(
        fi,
        "tls_rate_limit_per_second",
        &w.num_val(g.tls_rate_limit_per_second),
    );
    w.blank();

    // --- TLS Settings (Server-facing) ---
    w.separator(
        fi,
        w.t(
            "TLS Settings (Server-facing)",
            "Настройки TLS (для серверов PostgreSQL)",
        ),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Enable TLS for connections to PostgreSQL servers.",
            "Включить TLS для подключений к серверам PostgreSQL.",
        ),
    );
    w.comment(fi, "Default: false");
    w.kv(fi, "server_tls", &w.bool_val(g.server_tls));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Verify server certificate when connecting to PostgreSQL.",
            "Проверять сертификат сервера при подключении к PostgreSQL.",
        ),
    );
    w.comment(fi, "Default: false");
    w.kv(
        fi,
        "verify_server_certificate",
        &w.bool_val(g.verify_server_certificate),
    );
    w.blank();

    // --- Daemon Mode ---
    w.separator(fi, w.t("Daemon Mode", "Режим демона"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "PID file path for daemon mode.",
            "Путь к PID-файлу для режима демона.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Setting this enables daemon mode. Comment out for foreground mode with `-d`.",
            "Задание этого параметра активирует режим демона. Закомментируйте для запуска на переднем плане с `-d`.",
        ),
    );
    w.comment(fi, "Default: \"/tmp/pg_doorman.pid\"");
    w.commented_kv(fi, "daemon_pid_file", &w.str_val(&g.daemon_pid_file));
    w.blank();

    // --- Access Control (Legacy) ---
    w.separator(
        fi,
        w.t(
            "Access Control (Legacy)",
            "Контроль доступа (устаревший способ)",
        ),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Simple IP-based access control list.",
            "Простой список контроля доступа по IP.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "List of IP networks allowed to connect (e.g., \"10.0.0.0/8\").",
            "Список IP-сетей, которым разрешено подключаться (напр., \"10.0.0.0/8\").",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Empty list allows all addresses.",
            "Пустой список разрешает все адреса.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "For more advanced access control, use pg_hba instead.",
            "Для более гибкого контроля доступа используйте pg_hba.",
        ),
    );
    w.comment(fi, "Default: []");
    w.kv(fi, "hba", &w.empty_array());
    w.blank();

    // --- Access Control (pg_hba) ---
    w.separator(
        fi,
        w.t(
            "Access Control (pg_hba - Recommended)",
            "Контроль доступа (pg_hba — рекомендуется)",
        ),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "PostgreSQL-style pg_hba.conf rules for client authentication.",
            "Правила аутентификации клиентов в стиле pg_hba.conf PostgreSQL.",
        ),
    );
    w.comment(
        fi,
        w.t("Supports three formats:", "Поддерживает три формата:"),
    );
    w.comment(fi, "");
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
    w.comment(fi, "");
    w.comment(
        fi,
        w.t(
            "Rule format: TYPE DATABASE USER ADDRESS METHOD",
            "Формат правил: ТИП БАЗА ПОЛЬЗОВАТЕЛЬ АДРЕС МЕТОД",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Types: local, host, hostssl, hostnossl",
            "Типы: local, host, hostssl, hostnossl",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Methods: trust, md5, scram-sha-256, reject",
            "Методы: trust, md5, scram-sha-256, reject",
        ),
    );
    w.comment(fi, "");
    w.comment(
        fi,
        w.t(
            "Trust behavior: when a matching rule uses 'trust', pg_doorman accepts",
            "Поведение trust: если подходящее правило использует 'trust', pg_doorman принимает",
        ),
    );
    w.comment(
        fi,
        w.t(
            "the connection without asking for a password, even if the user has",
            "подключение без запроса пароля, даже если у пользователя настроен",
        ),
    );
    w.comment(
        fi,
        w.t(
            "an MD5 or SCRAM password configured.",
            "пароль MD5 или SCRAM.",
        ),
    );
    w.comment(fi, "");
    match w.format {
        ConfigFormat::Toml => {
            w.comment(fi, w.t("Example pg_hba rules:", "Пример правил pg_hba:"));
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
            w.comment(fi, w.t("Example pg_hba rules:", "Пример правил pg_hba:"));
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
    w.blank();
}

fn write_prometheus_section(w: &mut ConfigWriter, prom: &Prometheus) {
    w.major_separator(w.t("PROMETHEUS METRICS", "МЕТРИКИ PROMETHEUS"));
    w.section(0, "prometheus");
    let fi = w.field_indent();

    w.comment(
        fi,
        w.t(
            "Enable Prometheus metrics exporter.",
            "Включить экспорт метрик Prometheus.",
        ),
    );
    w.comment(fi, "Default: false");
    w.kv(fi, "enabled", &w.bool_val(prom.enabled));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Host for the metrics HTTP endpoint.",
            "Адрес HTTP-эндпоинта для метрик.",
        ),
    );
    w.comment(fi, "Default: \"0.0.0.0\"");
    w.kv(fi, "host", &w.str_val(&prom.host));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Port for the metrics HTTP endpoint.",
            "Порт HTTP-эндпоинта для метрик.",
        ),
    );
    w.comment(fi, "Default: 9127");
    w.kv(fi, "port", &w.num_val(prom.port));
    w.blank();
}

fn write_talos_section(w: &mut ConfigWriter) {
    w.major_separator(w.t(
        "TALOS AUTHENTICATION (Optional)",
        "АУТЕНТИФИКАЦИЯ TALOS (Опционально)",
    ));
    w.comment(
        0,
        w.t(
            "Talos is an optional authentication mechanism using public key cryptography.",
            "Talos — опциональный механизм аутентификации на основе криптографии с открытым ключом.",
        ),
    );
    match w.format {
        ConfigFormat::Toml => {
            w.comment(0, "[talos]");
            w.comment(
                0,
                &format!(
                    "# {}",
                    w.t(
                        "List of public key files for Talos authentication.",
                        "Список файлов публичных ключей для аутентификации Talos.",
                    )
                ),
            );
            w.comment(
                0,
                "keys = [\"/etc/pg_doorman/talos/public-key-1.pem\", \"/etc/pg_doorman/talos/public-key-2.pem\"]",
            );
            w.comment(
                0,
                &format!(
                    "# {}",
                    w.t(
                        "List of databases that use Talos authentication.",
                        "Список баз данных, использующих аутентификацию Talos.",
                    )
                ),
            );
            w.comment(0, "databases = [\"talos_db1\", \"talos_db2\"]");
        }
        ConfigFormat::Yaml => {
            w.comment(0, "talos:");
            w.comment(
                0,
                &format!(
                    "  # {}",
                    w.t(
                        "List of public key files for Talos authentication.",
                        "Список файлов публичных ключей для аутентификации Talos.",
                    )
                ),
            );
            w.comment(0, "  keys:");
            w.comment(0, "    - \"/etc/pg_doorman/talos/public-key-1.pem\"");
            w.comment(0, "    - \"/etc/pg_doorman/talos/public-key-2.pem\"");
            w.comment(
                0,
                &format!(
                    "  # {}",
                    w.t(
                        "List of databases that use Talos authentication.",
                        "Список баз данных, использующих аутентификацию Talos.",
                    )
                ),
            );
            w.comment(0, "  databases:");
            w.comment(0, "    - \"talos_db1\"");
            w.comment(0, "    - \"talos_db2\"");
        }
    }
    w.blank();
}

fn write_pools_section(w: &mut ConfigWriter, config: &Config) {
    w.major_separator(w.t("CONNECTION POOLS", "ПУЛЫ ПОДКЛЮЧЕНИЙ"));
    w.comment(
        0,
        w.t(
            "Each pool represents a virtual database that clients can connect to.",
            "Каждый пул представляет виртуальную базу данных, к которой подключаются клиенты.",
        ),
    );
    w.comment(
        0,
        w.t(
            "Pool names are visible to clients as database names.",
            "Имена пулов видны клиентам как имена баз данных.",
        ),
    );
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
    let fi = w.pool_field_indent();

    w.comment(
        fi,
        w.t("Example pool configuration", "Пример конфигурации пула"),
    );
    match w.format {
        ConfigFormat::Toml => {
            w.subsection(&format!("pools.{pool_name}"));
        }
        ConfigFormat::Yaml => {
            let _ = writeln!(w.output, "  {pool_name}:");
        }
    }

    // --- Server Connection Settings ---
    w.separator(
        fi,
        w.t(
            "Server Connection Settings",
            "Настройки подключения к серверу",
        ),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "PostgreSQL server host (IP address or unix socket directory).",
            "Адрес сервера PostgreSQL (IP или директория unix socket).",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Examples: \"127.0.0.1\", \"/var/run/postgresql\"",
            "Примеры: \"127.0.0.1\", \"/var/run/postgresql\"",
        ),
    );
    w.comment(fi, "Default: \"127.0.0.1\"");
    w.kv(fi, "server_host", &w.str_val(&pool.server_host));
    w.blank();

    w.comment(
        fi,
        w.t("PostgreSQL server port.", "Порт сервера PostgreSQL."),
    );
    w.comment(fi, "Default: 5432");
    w.kv(fi, "server_port", &w.num_val(pool.server_port));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Actual database name on the PostgreSQL server.",
            "Имя реальной базы данных на сервере PostgreSQL.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "If not specified, the pool name is used.",
            "Если не указано, используется имя пула.",
        ),
    );
    if let Some(ref db) = pool.server_database {
        w.kv(fi, "server_database", &w.str_val(db));
    } else {
        w.commented_kv(fi, "server_database", "\"actual_db_name\"");
    }
    w.blank();

    // --- Pool Settings ---
    w.separator(fi, w.t("Pool Settings", "Настройки пула"));
    w.blank();

    w.comment(fi, w.t("Pooling mode:", "Режим пулинга:"));
    w.comment(
        fi,
        w.t(
            "- \"transaction\" : Server released after each transaction (recommended)",
            "- \"transaction\" : Сервер освобождается после каждой транзакции (рекомендуется)",
        ),
    );
    w.comment(
        fi,
        w.t(
            "- \"session\"     : Server released when client disconnects",
            "- \"session\"     : Сервер освобождается при отключении клиента",
        ),
    );
    w.comment(fi, "Default: \"transaction\"");
    w.kv(fi, "pool_mode", &w.str_val(&pool.pool_mode.to_string()));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Override global connect_timeout for this pool (in milliseconds).",
            "Переопределить глобальный connect_timeout для этого пула (в миллисекундах).",
        ),
    );
    if let Some(val) = pool.connect_timeout {
        w.kv(fi, "connect_timeout", &w.num_val(val));
    } else {
        w.commented_kv(fi, "connect_timeout", "5000");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Override global idle_timeout for this pool (in milliseconds).",
            "Переопределить глобальный idle_timeout для этого пула (в миллисекундах).",
        ),
    );
    if let Some(val) = pool.idle_timeout {
        w.kv(fi, "idle_timeout", &w.num_val(val));
    } else {
        w.commented_kv(fi, "idle_timeout", "300000");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Override global server_lifetime for this pool (in milliseconds).",
            "Переопределить глобальный server_lifetime для этого пула (в миллисекундах).",
        ),
    );
    if let Some(val) = pool.server_lifetime {
        w.kv(fi, "server_lifetime", &w.num_val(val));
    } else {
        w.commented_kv(fi, "server_lifetime", "300000");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Clean up server connections (reset state) when returning to pool.",
            "Очищать серверные соединения (сброс состояния) при возврате в пул.",
        ),
    );
    w.comment(fi, "Default: true");
    w.kv(
        fi,
        "cleanup_server_connections",
        &w.bool_val(pool.cleanup_server_connections),
    );
    w.blank();

    w.comment(
        fi,
        w.t(
            "Override global prepared_statements_cache_size for this pool.",
            "Переопределить глобальный prepared_statements_cache_size для этого пула.",
        ),
    );
    if let Some(val) = pool.prepared_statements_cache_size {
        w.kv(fi, "prepared_statements_cache_size", &w.num_val(val));
    } else {
        w.commented_kv(fi, "prepared_statements_cache_size", "8192");
    }
    w.blank();

    // --- Application Settings ---
    w.separator(fi, w.t("Application Settings", "Настройки приложения"));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Application name sent to PostgreSQL when opening connections.",
            "Имя приложения, отправляемое PostgreSQL при открытии соединений.",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Useful when sync_server_parameters is disabled.",
            "Полезно, когда sync_server_parameters отключён.",
        ),
    );
    if let Some(ref name) = pool.application_name {
        w.kv(fi, "application_name", &w.str_val(name));
    } else {
        w.commented_kv(fi, "application_name", "\"my_application\"");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Log SET commands from clients.",
            "Логировать SET-команды от клиентов.",
        ),
    );
    w.comment(fi, "Default: false");
    w.kv(
        fi,
        "log_client_parameter_status_changes",
        &w.bool_val(pool.log_client_parameter_status_changes),
    );
    w.blank();

    write_pool_users(w, pool_name, &pool.users);
}

fn write_pool_users(w: &mut ConfigWriter, pool_name: &str, users: &[User]) {
    let fi = w.pool_field_indent();
    let ui = w.user_field_indent();

    match w.format {
        ConfigFormat::Toml => {
            w.separator(
                fi,
                w.t(
                    "Users Configuration (TOML uses indexed format)",
                    "Настройки пользователей (в TOML используется индексный формат)",
                ),
            );
            let en_msg = format!("Users are defined with numeric indices: [pools.{pool_name}.users.0], [pools.{pool_name}.users.1], etc.");
            let ru_msg = format!("Пользователи задаются с числовыми индексами: [pools.{pool_name}.users.0], [pools.{pool_name}.users.1] и т.д.");
            w.comment(fi, w.t(&en_msg, &ru_msg));
            w.comment(
                fi,
                w.t(
                    "Each user must have a unique username within the pool.",
                    "Каждый пользователь должен иметь уникальный username в рамках пула.",
                ),
            );
        }
        ConfigFormat::Yaml => {
            w.separator(fi, w.t("Users Configuration", "Настройки пользователей"));
            w.comment(
                fi,
                w.t(
                    "Array of users allowed to connect to this pool.",
                    "Массив пользователей, которым разрешено подключаться к этому пулу.",
                ),
            );
            w.comment(
                fi,
                w.t(
                    "Each user must have a unique username within the pool.",
                    "Каждый пользователь должен иметь уникальный username в рамках пула.",
                ),
            );
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
    w.comment(
        fi,
        w.t(
            "Username for client authentication.",
            "Имя пользователя для аутентификации клиента.",
        ),
    );
    w.kv(fi, "username", &w.str_val(&user.username));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Password for client authentication.",
            "Пароль для аутентификации клиента.",
        ),
    );
    w.comment(fi, w.t("Supported formats:", "Поддерживаемые форматы:"));
    w.comment(fi, "- MD5: \"md5\" + md5(password + username)");
    w.comment(
        fi,
        "- SCRAM-SHA-256: \"SCRAM-SHA-256$iterations:salt$StoredKey:ServerKey\"",
    );
    w.comment(
        fi,
        "- JWT public key: \"jwt-pkey-fpath:/path/to/public.pem\"",
    );
    w.comment(fi, "");
    w.comment(
        fi,
        w.t(
            "Generate MD5: echo -n \"passwordusername\" | md5sum",
            "Сгенерировать MD5: echo -n \"парольимяпользователя\" | md5sum",
        ),
    );
    w.comment(
        fi,
        w.t(
            "Get from PostgreSQL: SELECT usename, passwd FROM pg_shadow;",
            "Получить из PostgreSQL: SELECT usename, passwd FROM pg_shadow;",
        ),
    );
    w.kv(fi, "password", &w.str_val(&user.password));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Maximum connections to PostgreSQL for this user.",
            "Максимальное количество соединений с PostgreSQL для этого пользователя.",
        ),
    );
    w.comment(fi, "Default: 40");
    w.kv(fi, "pool_size", &w.num_val(user.pool_size));
    w.blank();

    w.comment(
        fi,
        w.t(
            "Minimum connections to maintain in the pool.",
            "Минимальное количество соединений для поддержания в пуле.",
        ),
    );
    w.comment(
        fi,
        w.t("Must be <= pool_size.", "Должно быть <= pool_size."),
    );
    if let Some(val) = user.min_pool_size {
        w.kv(fi, "min_pool_size", &w.num_val(val));
    } else {
        w.commented_kv(fi, "min_pool_size", "5");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Override pool-level pool_mode for this user.",
            "Переопределить pool_mode пула для этого пользователя.",
        ),
    );
    if let Some(ref mode) = user.pool_mode {
        w.kv(fi, "pool_mode", &w.str_val(&mode.to_string()));
    } else {
        w.commented_kv(fi, "pool_mode", "\"session\"");
    }
    w.blank();

    w.comment(
        fi,
        w.t(
            "Override pool-level server_lifetime for this user (in milliseconds).",
            "Переопределить server_lifetime пула для этого пользователя (в миллисекундах).",
        ),
    );
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

    w.comment(
        fi,
        w.t(
            "PAM service name for PAM authentication (requires 'pam' feature).",
            "Имя PAM-сервиса для PAM-аутентификации (требуется фича 'pam').",
        ),
    );
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

    w.comment(
        3,
        w.t(
            "Password for client authentication.",
            "Пароль для аутентификации клиента.",
        ),
    );
    w.comment(3, w.t("Supported formats:", "Поддерживаемые форматы:"));
    w.comment(3, "- MD5: \"md5\" + md5(password + username)");
    w.comment(
        3,
        "- SCRAM-SHA-256: \"SCRAM-SHA-256$iterations:salt$StoredKey:ServerKey\"",
    );
    w.comment(
        3,
        "- JWT public key: \"jwt-pkey-fpath:/path/to/public.pem\"",
    );
    w.comment(3, "");
    w.comment(
        3,
        w.t(
            "Generate MD5: echo -n \"passwordusername\" | md5sum",
            "Сгенерировать MD5: echo -n \"парольимяпользователя\" | md5sum",
        ),
    );
    w.comment(
        3,
        w.t(
            "Get from PostgreSQL: SELECT usename, passwd FROM pg_shadow;",
            "Получить из PostgreSQL: SELECT usename, passwd FROM pg_shadow;",
        ),
    );
    let _ = writeln!(w.output, "{indent}  password: \"{}\"", user.password);
    w.blank();

    w.comment(
        3,
        w.t(
            "Maximum connections to PostgreSQL for this user.",
            "Максимальное количество соединений с PostgreSQL для этого пользователя.",
        ),
    );
    w.comment(3, "Default: 40");
    let _ = writeln!(w.output, "{indent}  pool_size: {}", user.pool_size);
    w.blank();

    w.comment(
        3,
        w.t(
            "Minimum connections to maintain in the pool.",
            "Минимальное количество соединений для поддержания в пуле.",
        ),
    );
    w.comment(3, w.t("Must be <= pool_size.", "Должно быть <= pool_size."));
    if let Some(val) = user.min_pool_size {
        let _ = writeln!(w.output, "{indent}  min_pool_size: {val}");
    } else {
        let _ = writeln!(w.output, "{indent}  # min_pool_size: 5");
    }
    w.blank();

    w.comment(
        3,
        w.t(
            "Override pool-level pool_mode for this user.",
            "Переопределить pool_mode пула для этого пользователя.",
        ),
    );
    if let Some(ref mode) = user.pool_mode {
        let _ = writeln!(w.output, "{indent}  pool_mode: \"{mode}\"");
    } else {
        let _ = writeln!(w.output, "{indent}  # pool_mode: \"session\"");
    }
    w.blank();

    w.comment(
        3,
        w.t(
            "Override pool-level server_lifetime for this user (in milliseconds).",
            "Переопределить server_lifetime пула для этого пользователя (в миллисекундах).",
        ),
    );
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

    w.comment(
        3,
        w.t(
            "PAM service name for PAM authentication (requires 'pam' feature).",
            "Имя PAM-сервиса для PAM-аутентификации (требуется фича 'pam').",
        ),
    );
    if let Some(ref pam) = user.auth_pam_service {
        let _ = writeln!(w.output, "{indent}  auth_pam_service: \"{pam}\"");
    } else {
        let _ = writeln!(w.output, "{indent}  # auth_pam_service: \"pg_doorman\"");
    }
}

/// Write prominent documentation about server_username/server_password.
/// This is the #1 pain point for new users.
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
    let fi = w.pool_field_indent();
    w.blank();
    w.separator(
        fi,
        w.t("Additional Pool Examples", "Дополнительные примеры пулов"),
    );
    w.blank();

    match w.format {
        ConfigFormat::Toml => {
            w.comment(
                0,
                w.t(
                    "Example: Pool with multiple users",
                    "Пример: Пул с несколькими пользователями",
                ),
            );
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

            w.comment(
                0,
                w.t(
                    "Example: Pool with unix socket connection",
                    "Пример: Пул с подключением через unix socket",
                ),
            );
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

            w.comment(
                0,
                w.t(
                    "Example: Pool with JWT authentication",
                    "Пример: Пул с JWT-аутентификацией",
                ),
            );
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

            w.comment(
                0,
                w.t(
                    "Example: Pool with server-side credentials mapping",
                    "Пример: Пул с маппингом серверных учётных данных",
                ),
            );
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
            w.comment(
                1,
                w.t(
                    "Example: Pool with multiple users",
                    "Пример: Пул с несколькими пользователями",
                ),
            );
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

            w.comment(
                1,
                w.t(
                    "Example: Pool with unix socket connection",
                    "Пример: Пул с подключением через unix socket",
                ),
            );
            w.comment(1, "local_db:");
            w.comment(1, "  server_host: \"/var/run/postgresql\"");
            w.comment(1, "  server_port: 5432");
            w.comment(1, "  pool_mode: \"session\"");
            w.comment(1, "  users:");
            w.comment(1, "    - username: \"local_user\"");
            w.comment(1, "      password: \"md5...\"");
            w.comment(1, "      pool_size: 50");
            w.blank();

            w.comment(
                1,
                w.t(
                    "Example: Pool with JWT authentication",
                    "Пример: Пул с JWT-аутентификацией",
                ),
            );
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

            w.comment(
                1,
                w.t(
                    "Example: Pool with server-side credentials mapping",
                    "Пример: Пул с маппингом серверных учётных данных",
                ),
            );
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

fn write_duration_field(
    w: &mut ConfigWriter,
    indent: usize,
    key: &str,
    millis: u64,
    description: &str,
    human_readable: &str,
    millis_str: &str,
) {
    // Description may contain \n# for multiline
    for line in description.split('\n') {
        let line = line.strip_prefix("# ").unwrap_or(line);
        w.comment(indent, line);
    }

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

fn write_byte_size_field(
    w: &mut ConfigWriter,
    indent: usize,
    key: &str,
    bytes: u64,
    description: &str,
    human_readable: &str,
    bytes_str: &str,
) {
    for line in description.split('\n') {
        let line = line.strip_prefix("# ").unwrap_or(line);
        w.comment(indent, line);
    }

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

    /// Verify that all public fields from config source files are documented
    /// in the reference documentation (documentation/docs/reference/*.md).
    /// This catches missing fields when someone adds a new config parameter
    /// but forgets to document it.
    #[test]
    fn test_reference_docs_cover_all_config_fields() {
        let general_md = include_str!("../../../documentation/docs/reference/general.md");
        let pool_md = include_str!("../../../documentation/docs/reference/pool.md");

        // Fields that are internal/structural and not documented as standalone params
        let skip_fields: &[&str] = &[
            "users",   // structural: documented as a section in pool.md
            "pools",   // structural: section header
            "path",    // internal: not a config parameter
            "include", // structural: section in general.md, not a field heading
            "files",   // part of include section
        ];

        let checks: &[(&str, &str, &str)] = &[
            (
                "General",
                include_str!("../../config/general.rs"),
                general_md,
            ),
            ("Pool", include_str!("../../config/pool.rs"), pool_md),
            ("User", include_str!("../../config/user.rs"), pool_md),
        ];

        let mut missing = Vec::new();

        for (struct_name, source, doc) in checks {
            let fields = extract_pub_fields(source);
            for field in &fields {
                if skip_fields.contains(&field.as_str()) {
                    continue;
                }
                if !doc.contains(field.as_str()) {
                    missing.push(format!("{struct_name}::{field}"));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "The following config fields are NOT documented in reference docs.\n\
             Add them to documentation/docs/reference/general.md or pool.md:\n  - {}",
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
}
