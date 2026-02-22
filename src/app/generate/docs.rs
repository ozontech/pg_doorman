//! Reference documentation generator (Markdown).
//!
//! Generates markdown reference docs from `fields.yaml`, the same source
//! used by `annotated.rs` for config generation. Supports EN and RU.

use std::fmt::Write;

use super::annotated::{FieldsData, FIELDS};

/// Generate the general settings reference doc.
pub fn generate_general_doc(russian: bool) -> String {
    let f = &*FIELDS;
    let mut out = String::with_capacity(16 * 1024);

    write_general_frontmatter(&mut out, russian);
    write_config_format_section(&mut out, russian);
    write_human_readable_section(&mut out, russian);
    write_general_fields(&mut out, f, russian);

    out
}

/// Generate the pool settings reference doc.
pub fn generate_pool_doc(russian: bool) -> String {
    let f = &*FIELDS;
    let mut out = String::with_capacity(8 * 1024);

    write_pool_frontmatter(&mut out, russian);
    write_pool_fields(&mut out, f, russian);
    write_user_fields(&mut out, f, russian);

    out
}

/// Generate the prometheus settings reference doc.
pub fn generate_prometheus_doc(russian: bool) -> String {
    let f = &*FIELDS;
    let mut out = String::with_capacity(8 * 1024);

    write_prometheus_frontmatter(&mut out, russian);
    write_prometheus_fields(&mut out, f, russian);
    write_prometheus_metrics_section(&mut out, russian);

    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn field_doc(f: &FieldsData, section: &str, name: &str, russian: bool) -> String {
    let desc = f.field(section, name);
    let text = desc
        .doc
        .as_ref()
        .map(|d| d.get(russian))
        .unwrap_or_else(|| desc.config.as_ref().map(|c| c.get(russian)).unwrap_or(""));
    text.trim_end().to_string()
}

fn write_param(out: &mut String, f: &FieldsData, section: &str, name: &str, russian: bool) {
    let _ = writeln!(out, "### {name}\n");
    let desc = field_doc(f, section, name, russian);
    if !desc.is_empty() {
        let _ = writeln!(out, "{desc}\n");
    }
    let field = f.field(section, name);
    if let Some(ref d) = field.default {
        let _ = writeln!(out, "Default: `{d}`.\n");
    }
}

// ---------------------------------------------------------------------------
// General doc
// ---------------------------------------------------------------------------

fn write_general_frontmatter(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "# Настройки\n");
    } else {
        let _ = writeln!(out, "# Settings\n");
    }
}

fn write_config_format_section(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "## Формат конфигурационного файла\n");
        let _ = writeln!(
            out,
            "pg_doorman поддерживает два формата конфигурационных файлов:\n"
        );
        let _ = writeln!(out, "* **YAML** (`.yaml`, `.yml`) - Основной и рекомендуемый формат для новых конфигураций.");
        let _ = writeln!(out, "* **TOML** (`.toml`) - Поддерживается для обратной совместимости с существующими конфигурациями.\n");
        let _ = writeln!(out, "Формат определяется автоматически по расширению файла. Оба формата поддерживают одинаковые параметры конфигурации и могут использоваться взаимозаменяемо.\n");
    } else {
        let _ = writeln!(out, "## Configuration File Format\n");
        let _ = writeln!(out, "pg_doorman supports two configuration file formats:\n");
        let _ = writeln!(out, "* **YAML** (`.yaml`, `.yml`) - The primary and recommended format for new configurations.");
        let _ = writeln!(out, "* **TOML** (`.toml`) - Supported for backward compatibility with existing configurations.\n");
        let _ = writeln!(out, "The format is automatically detected based on the file extension. Both formats support the same configuration options and can be used interchangeably.\n");
    }

    write_config_examples(out, russian);
    write_generate_command(out, russian);
    write_include_files(out, russian);
}

fn write_config_examples(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "### Пример конфигурации YAML (рекомендуется)\n");
    } else {
        let _ = writeln!(out, "### Example YAML Configuration (Recommended)\n");
    }
    let _ = writeln!(
        out,
        "```yaml\ngeneral:\n  host: \"0.0.0.0\"\n  port: 6432\n  admin_username: \"admin\"\n  admin_password: \"admin\"\n\npools:\n  mydb:\n    server_host: \"localhost\"\n    server_port: 5432\n    pool_mode: \"transaction\"\n    users:\n      - username: \"myuser\"\n        password: \"mypassword\"\n        pool_size: 40\n```\n"
    );

    if russian {
        let _ = writeln!(out, "### Пример конфигурации TOML (устаревший)\n");
    } else {
        let _ = writeln!(out, "### Example TOML Configuration (Legacy)\n");
    }
    let _ = writeln!(
        out,
        "```toml\n[general]\nhost = \"0.0.0.0\"\nport = 6432\nadmin_username = \"admin\"\nadmin_password = \"admin\"\n\n[pools.mydb]\nserver_host = \"localhost\"\nserver_port = 5432\npool_mode = \"transaction\"\n\n[[pools.mydb.users]]\nusername = \"myuser\"\npassword = \"mypassword\"\npool_size = 40\n```\n"
    );
}

fn write_generate_command(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "### Команда generate\n");
        let _ = writeln!(out, "Команда `generate` может выводить конфигурацию в любом формате. Формат определяется по расширению выходного файла. По умолчанию сгенерированный конфиг включает подробные встроенные комментарии, объясняющие каждый параметр.\n");
    } else {
        let _ = writeln!(out, "### Generate Command\n");
        let _ = writeln!(out, "The `generate` command can output configuration in either format. The format is determined by the output file extension. By default, the generated config includes detailed inline comments explaining every parameter.\n");
    }
    let _ = writeln!(
        out,
        "```bash\n# Generate YAML configuration (recommended)\npg_doorman generate --output config.yaml\n\n# Generate TOML configuration (for backward compatibility)\npg_doorman generate --output config.toml\n\n# Generate a complete reference config without PG connection\npg_doorman generate --reference --output config.yaml\n\n# Generate reference config with Russian comments\npg_doorman generate --reference --ru --output config.yaml\n\n# Generate config without comments (plain serialization)\npg_doorman generate --no-comments --output config.yaml\n```\n"
    );

    let _ = writeln!(out, "| Flag | Description |");
    let _ = writeln!(out, "|------|-------------|");
    let _ = writeln!(out, "| `--no-comments` | Disable inline comments in generated config (by default, comments are included) |");
    let _ = writeln!(out, "| `--reference` | Generate a complete reference config with example values, no PostgreSQL connection needed |");
    let _ = writeln!(
        out,
        "| `--russian-comments`, `--ru` | Generate comments in Russian for quick start guide |"
    );
    let _ = writeln!(out, "| `--format`, `-f` | Output format: `yaml` (default) or `toml`. If `--output` is specified, format is auto-detected from file extension. This flag overrides auto-detection |\n");
}

fn write_include_files(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "### Включение файлов\n");
        let _ = writeln!(out, "Включаемые файлы могут быть в любом формате, и форматы можно смешивать. Например, основной конфиг YAML может включать файлы TOML и наоборот:\n");
    } else {
        let _ = writeln!(out, "### Include Files\n");
        let _ = writeln!(out, "Include files can be in either format, and you can mix formats. For example, a YAML main config can include TOML files and vice versa:\n");
    }
    let _ = writeln!(
        out,
        "```yaml\ninclude:\n  files:\n    - \"pools.yaml\"\n    - \"users.toml\"\n```\n"
    );
}

fn write_human_readable_section(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "## Человекочитаемые значения\n");
        let _ = writeln!(out, "pg_doorman поддерживает человекочитаемые форматы для значений длительности и размера в байтах, сохраняя обратную совместимость с числовыми значениями.\n");
        let _ = writeln!(out, "### Формат длительности\n");
        let _ = writeln!(out, "Значения длительности могут быть указаны как:\n");
    } else {
        let _ = writeln!(out, "## Human-Readable Values\n");
        let _ = writeln!(out, "pg_doorman supports human-readable formats for duration and byte size values, while maintaining backward compatibility with numeric values.\n");
        let _ = writeln!(out, "### Duration Format\n");
        let _ = writeln!(out, "Duration values can be specified as:\n");
    }
    if russian {
        let _ = writeln!(
            out,
            "* **Числа**: интерпретируются как миллисекунды (напр., `5000` = 5 секунд)"
        );
        let _ = writeln!(out, "* **Строка с суффиксом**:");
    } else {
        let _ = writeln!(
            out,
            "* **Plain numbers**: interpreted as milliseconds (e.g., `5000` = 5 seconds)"
        );
        let _ = writeln!(out, "* **String with suffix**:");
    }
    let _ = writeln!(out, "    * `ms` - milliseconds (e.g., `\"100ms\"`)");
    let _ = writeln!(
        out,
        "    * `s` - seconds (e.g., `\"5s\"` = 5000 milliseconds)"
    );
    let _ = writeln!(
        out,
        "    * `m` - minutes (e.g., `\"5m\"` = 300000 milliseconds)"
    );
    let _ = writeln!(
        out,
        "    * `h` - hours (e.g., `\"1h\"` = 3600000 milliseconds)"
    );
    let _ = writeln!(
        out,
        "    * `d` - days (e.g., `\"1d\"` = 86400000 milliseconds)\n"
    );

    if russian {
        let _ = writeln!(out, "**Примеры:**");
    } else {
        let _ = writeln!(out, "**Examples:**");
    }
    let _ = writeln!(out, "```yaml\ngeneral:\n  # All these are equivalent (3 seconds):\n  # connect_timeout: 3000      # backward compatible (milliseconds)\n  # connect_timeout: \"3s\"      # human-readable\n  # connect_timeout: \"3000ms\"  # explicit milliseconds\n  connect_timeout: \"3s\"\n  idle_timeout: \"5m\"         # 5 minutes\n  server_lifetime: \"1h\"      # 1 hour\n```\n");

    if russian {
        let _ = writeln!(out, "### Формат размера в байтах\n");
        let _ = writeln!(out, "Значения размера в байтах могут быть указаны как:\n");
        let _ = writeln!(
            out,
            "* **Числа**: интерпретируются как байты (напр., `1048576` = 1 МБ)"
        );
        let _ = writeln!(out, "* **Строка с суффиксом** (регистронезависимо):");
    } else {
        let _ = writeln!(out, "### Byte Size Format\n");
        let _ = writeln!(out, "Byte size values can be specified as:\n");
        let _ = writeln!(
            out,
            "* **Plain numbers**: interpreted as bytes (e.g., `1048576` = 1 MB)"
        );
        let _ = writeln!(out, "* **String with suffix** (case-insensitive):");
    }
    let _ = writeln!(out, "    * `B` - bytes (e.g., `\"1024B\"`)");
    let _ = writeln!(
        out,
        "    * `K` or `KB` - kilobytes (e.g., `\"1K\"` or `\"1KB\"` = 1024 bytes)"
    );
    let _ = writeln!(
        out,
        "    * `M` or `MB` - megabytes (e.g., `\"1M\"` or `\"1MB\"` = 1048576 bytes)"
    );
    let _ = writeln!(
        out,
        "    * `G` or `GB` - gigabytes (e.g., `\"1G\"` or `\"1GB\"` = 1073741824 bytes)\n"
    );

    if russian {
        let _ = writeln!(
            out,
            "Примечание: Используются двоичные префиксы (1 КБ = 1024 байт, не 1000 байт).\n"
        );
        let _ = writeln!(out, "**Примеры:**");
    } else {
        let _ = writeln!(
            out,
            "Note: Uses binary prefixes (1 KB = 1024 bytes, not 1000 bytes).\n"
        );
        let _ = writeln!(out, "**Examples:**");
    }
    let _ = writeln!(out, "```yaml\ngeneral:\n  # All these are equivalent (256 MB):\n  # max_memory_usage: 268435456  # backward compatible (bytes)\n  # max_memory_usage: \"256MB\"    # human-readable\n  # max_memory_usage: \"256M\"     # short form\n  max_memory_usage: \"256MB\"\n  unix_socket_buffer_size: \"1MB\" # 1 MB\n  worker_stack_size: \"8MB\"       # 8 MB\n```\n");
}

fn write_general_fields(out: &mut String, f: &FieldsData, russian: bool) {
    if russian {
        let _ = writeln!(out, "## Основные настройки\n");
    } else {
        let _ = writeln!(out, "## General Settings\n");
    }

    // Order matches the existing reference docs
    let fields = [
        "host",
        "port",
        "backlog",
        "max_connections",
        "max_concurrent_creates",
        "tls_mode",
        "tls_ca_cert",
        "tls_private_key",
        "tls_certificate",
        "tls_rate_limit_per_second",
        "daemon_pid_file",
        "syslog_prog_name",
        "log_client_connections",
        "log_client_disconnections",
        "worker_threads",
        "worker_cpu_affinity_pinning",
        "tokio_global_queue_interval",
        "tokio_event_interval",
        "worker_stack_size",
        "max_blocking_threads",
        "connect_timeout",
        "query_wait_timeout",
        "idle_timeout",
        "server_lifetime",
        "retain_connections_time",
        "retain_connections_max",
        "server_idle_check_timeout",
        "server_round_robin",
        "sync_server_parameters",
        "tcp_so_linger",
        "tcp_no_delay",
        "tcp_keepalives_count",
        "tcp_keepalives_idle",
        "tcp_keepalives_interval",
        "tcp_user_timeout",
        "unix_socket_buffer_size",
        "admin_username",
        "admin_password",
        "prepared_statements",
        "prepared_statements_cache_size",
        "client_prepared_statements_cache_size",
        "message_size_to_be_stream",
        "max_memory_usage",
        "shutdown_timeout",
        "proxy_copy_data_timeout",
        "server_tls",
        "verify_server_certificate",
        "hba",
        "pg_hba",
        "pooler_check_query",
    ];

    for name in &fields {
        write_param(out, f, "general", name, russian);
    }
}

// ---------------------------------------------------------------------------
// Pool doc
// ---------------------------------------------------------------------------

fn write_pool_frontmatter(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "## Настройки пула\n");
        let _ = writeln!(out, "Каждая запись в пуле — это имя виртуальной базы данных, к которой клиент pg-doorman может подключиться.\n");
    } else {
        let _ = writeln!(out, "## Pool Settings\n");
        let _ = writeln!(out, "Each record in the pool is the name of the virtual database that the pg-doorman client can connect to.\n");
    }
    let _ = writeln!(
        out,
        "```toml\n[pools.exampledb] # Declaring the 'exampledb' database\n```\n"
    );
}

fn write_pool_fields(out: &mut String, f: &FieldsData, russian: bool) {
    let fields = [
        "server_host",
        "server_port",
        "server_database",
        "application_name",
        "connect_timeout",
        "idle_timeout",
        "server_lifetime",
        "pool_mode",
        "log_client_parameter_status_changes",
        "cleanup_server_connections",
    ];

    for name in &fields {
        write_param(out, f, "pool", name, russian);
    }
}

fn write_user_fields(out: &mut String, f: &FieldsData, russian: bool) {
    if russian {
        let _ = writeln!(out, "## Настройки пользователей пула\n");
    } else {
        let _ = writeln!(out, "## Pool Users Settings\n");
    }
    let _ = writeln!(
        out,
        "```toml\n[pools.exampledb.users.0]\nusername = \"exampledb-user-0\" # A virtual user who can connect to this virtual database.\n```"
    );

    let fields = [
        "username",
        "password",
        "auth_pam_service",
        "server_username",
        "server_password",
        "pool_size",
        "min_pool_size",
        "server_lifetime",
    ];

    for name in &fields {
        write_param(out, f, "user", name, russian);
    }

    // Add the server_credentials warning admonition (mdbook-admonish format)
    if russian {
        let _ = writeln!(
            out,
            "`````admonish warning title=\"Типичная проблема настройки\""
        );
        let _ = writeln!(out, "Если вы видите ошибки аутентификации при подключении PgDoorman к PostgreSQL, наиболее вероятная причина — `server_username` и `server_password` не установлены. Без них PgDoorman пытается аутентифицироваться в PostgreSQL, используя хеш MD5/SCRAM из поля `password`, который PostgreSQL отклоняет.\n");
        let _ = writeln!(out, "**Решение:** Установите оба поля `server_username` и `server_password` с реальными учётными данными PostgreSQL:\n");
    } else {
        let _ = writeln!(out, "`````admonish warning title=\"Common Setup Issue\"");
        let _ = writeln!(out, "If you see authentication errors when PgDoorman tries to connect to PostgreSQL, the most likely cause is that `server_username` and `server_password` are not set. Without these, PgDoorman tries to authenticate to PostgreSQL using the MD5/SCRAM hash from the `password` field, which PostgreSQL rejects.\n");
        let _ = writeln!(out, "**Solution:** Set both `server_username` and `server_password` to the actual PostgreSQL credentials:\n");
    }
    let _ = writeln!(out, "```yaml\nusers:\n  - username: \"app_user\"\n    password: \"md5...\"                # hash for client authentication\n    server_username: \"app_user\"       # real PostgreSQL username\n    server_password: \"plaintext_pwd\"  # real PostgreSQL password\n```\n`````\n");
}

// ---------------------------------------------------------------------------
// Prometheus doc
// ---------------------------------------------------------------------------

fn write_prometheus_frontmatter(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "# Настройки Prometheus\n");
        let _ = writeln!(out, "pg_doorman включает экспортёр метрик Prometheus, предоставляющий подробную информацию о производительности и поведении пулов подключений. В этом документе описано, как включить и использовать экспортёр метрик Prometheus, а также доступные метрики.\n");
    } else {
        let _ = writeln!(out, "# Prometheus Settings\n");
        let _ = writeln!(out, "pg_doorman includes a Prometheus metrics exporter that provides detailed insights into the performance and behavior of your connection pools. This document describes how to enable and use the Prometheus metrics exporter, as well as the available metrics.\n");
    }
}

fn write_prometheus_fields(out: &mut String, f: &FieldsData, russian: bool) {
    if russian {
        let _ = writeln!(out, "## Включение метрик Prometheus\n");
    } else {
        let _ = writeln!(out, "## Enabling Prometheus Metrics\n");
    }
    if russian {
        let _ = writeln!(out, "Для включения экспортёра метрик Prometheus добавьте следующую конфигурацию в ваш файл `pg_doorman.toml`:\n");
    } else {
        let _ = writeln!(out, "To enable the Prometheus metrics exporter, add the following configuration to your `pg_doorman.toml` file:\n");
    }
    let _ = writeln!(out, "```toml\n[prometheus]\nenabled = true\nhost = \"0.0.0.0\"  # The host on which the metrics server will listen\nport = 9127       # The port on which the metrics server will listen\n```\n");

    if russian {
        let _ = writeln!(out, "### Параметры конфигурации\n");
    } else {
        let _ = writeln!(out, "### Configuration Options\n");
    }

    let _ = writeln!(out, "| Option | Description | Default |");
    let _ = writeln!(out, "|--------|-------------|---------|");
    let _ = writeln!(
        out,
        "| `enabled` | {} | `false` |",
        field_doc(f, "prometheus", "enabled", russian)
    );
    let _ = writeln!(
        out,
        "| `host` | {} | `\"0.0.0.0\"` |",
        field_doc(f, "prometheus", "host", russian)
    );
    let _ = writeln!(
        out,
        "| `port` | {} | `9127` |\n",
        field_doc(f, "prometheus", "port", russian)
    );
}

fn write_prometheus_metrics_section(out: &mut String, russian: bool) {
    if russian {
        let _ = writeln!(out, "## Настройка Prometheus\n");
        let _ = writeln!(
            out,
            "Добавьте следующую задачу в конфигурацию Prometheus для сбора метрик pg_doorman:\n"
        );
    } else {
        let _ = writeln!(out, "## Configuring Prometheus\n");
        let _ = writeln!(out, "Add the following job to your Prometheus configuration to scrape metrics from pg_doorman:\n");
    }
    let _ = writeln!(out, "```yaml\nscrape_configs:\n  - job_name: 'pg_doorman'\n    static_configs:\n      - targets: ['<pg_doorman_host>:9127']\n```\n");
    if russian {
        let _ = writeln!(out, "Замените `<pg_doorman_host>` на имя хоста или IP-адрес вашего экземпляра pg_doorman.\n");
    } else {
        let _ = writeln!(out, "Replace `<pg_doorman_host>` with the hostname or IP address of your pg_doorman instance.\n");
    }

    if russian {
        let _ = writeln!(out, "## Доступные метрики\n");
        let _ = writeln!(out, "pg_doorman предоставляет следующие метрики:\n");
    } else {
        let _ = writeln!(out, "## Available Metrics\n");
        let _ = writeln!(out, "pg_doorman exposes the following metrics:\n");
    }

    // System Metrics
    if russian {
        let _ = writeln!(out, "### Системные метрики\n");
    } else {
        let _ = writeln!(out, "### System Metrics\n");
    }
    let _ = writeln!(out, "| Metric | Description |");
    let _ = writeln!(out, "|--------|-------------|");
    let _ = writeln!(out, "| `pg_doorman_total_memory` | Total memory allocated to the pg_doorman process in bytes. Monitors the memory footprint of the application. |\n");

    // Connection Metrics
    if russian {
        let _ = writeln!(out, "### Метрики подключений\n");
    } else {
        let _ = writeln!(out, "### Connection Metrics\n");
    }
    let _ = writeln!(out, "| Metric | Description |");
    let _ = writeln!(out, "|--------|-------------|");
    let _ = writeln!(out, "| `pg_doorman_connection_count` | Counter of new connections by type handled by pg_doorman. Types include: 'plain' (unencrypted connections), 'tls' (encrypted connections), 'cancel' (connection cancellation requests), and 'total' (sum of all connections). |\n");

    // Socket Metrics
    if russian {
        let _ = writeln!(out, "### Метрики сокетов (только Linux)\n");
    } else {
        let _ = writeln!(out, "### Socket Metrics (Linux only)\n");
    }
    let _ = writeln!(out, "| Metric | Description |");
    let _ = writeln!(out, "|--------|-------------|");
    let _ = writeln!(out, "| `pg_doorman_sockets` | Counter of sockets used by pg_doorman by socket type. Types include: 'tcp' (IPv4 TCP sockets), 'tcp6' (IPv6 TCP sockets), 'unix' (Unix domain sockets), and 'unknown' (sockets of unrecognized type). Only available on Linux systems. |\n");

    // Pool Metrics
    if russian {
        let _ = writeln!(out, "### Метрики пулов\n");
    } else {
        let _ = writeln!(out, "### Pool Metrics\n");
    }
    let _ = writeln!(out, "| Metric | Description |");
    let _ = writeln!(out, "|--------|-------------|");
    let _ = writeln!(out, "| `pg_doorman_pools_clients` | Number of clients in connection pools by status, user, and database. Status values include: 'idle' (connected but not executing queries), 'waiting' (waiting for a server connection), and 'active' (currently executing queries). Helps monitor connection pool utilization and client distribution. |");
    let _ = writeln!(out, "| `pg_doorman_pools_servers` | Number of servers in connection pools by status, user, and database. Status values include: 'active' (actively serving clients) and 'idle' (available for new connections). Helps monitor server availability and load distribution. |");
    let _ = writeln!(out, "| `pg_doorman_pools_bytes` | Total bytes transferred through connection pools by direction, user, and database. Direction values include: 'received' (bytes received from clients) and 'sent' (bytes sent to clients). Useful for monitoring network traffic and identifying high-volume connections. |\n");

    // Query and Transaction Metrics
    if russian {
        let _ = writeln!(out, "### Метрики запросов и транзакций\n");
    } else {
        let _ = writeln!(out, "### Query and Transaction Metrics\n");
    }
    let _ = writeln!(out, "| Metric | Description |");
    let _ = writeln!(out, "|--------|-------------|");
    let _ = writeln!(out, "| `pg_doorman_pools_queries_percentile` | Query execution time percentiles by user and database. Percentile values include: '99', '95', '90', and '50' (median). Values are in milliseconds. Helps identify slow queries and performance trends across different users and databases. |");
    let _ = writeln!(out, "| `pg_doorman_pools_transactions_percentile` | Transaction execution time percentiles by user and database. Percentile values include: '99', '95', '90', and '50' (median). Values are in milliseconds. Helps monitor transaction performance and identify long-running transactions that might impact database performance. |");
    let _ = writeln!(out, "| `pg_doorman_pools_transactions_count` | Counter of transactions executed in connection pools by user and database. Helps track transaction volume and identify users or databases with high transaction rates. |");
    let _ = writeln!(out, "| `pg_doorman_pools_transactions_total_time` | Total time spent executing transactions in connection pools by user and database. Values are in milliseconds. Helps monitor overall transaction performance and identify users or databases with high transaction execution times. |");
    let _ = writeln!(out, "| `pg_doorman_pools_queries_count` | Counter of queries executed in connection pools by user and database. Helps track query volume and identify users or databases with high query rates. |");
    let _ = writeln!(out, "| `pg_doorman_pools_queries_total_time` | Total time spent executing queries in connection pools by user and database. Values are in milliseconds. Helps monitor overall query performance and identify users or databases with high query execution times. |");
    let _ = writeln!(out, "| `pg_doorman_pools_avg_wait_time` | Average wait time for clients in connection pools by user and database. Values are in milliseconds. Helps monitor client wait times and identify potential bottlenecks. |\n");

    // Server Metrics
    if russian {
        let _ = writeln!(out, "### Метрики серверов\n");
    } else {
        let _ = writeln!(out, "### Server Metrics\n");
    }
    let _ = writeln!(out, "| Metric | Description |");
    let _ = writeln!(out, "|--------|-------------|");
    let _ = writeln!(out, "| `pg_doorman_servers_prepared_hits` | Counter of prepared statement hits in databases backends by user and database. Helps track the effectiveness of prepared statements in reducing query parsing overhead. |");
    let _ = writeln!(out, "| `pg_doorman_servers_prepared_misses` | Counter of prepared statement misses in databases backends by user and database. Helps identify queries that could benefit from being prepared to improve performance. |\n");

    // Grafana Dashboard
    if russian {
        let _ = writeln!(out, "## Дашборд Grafana\n");
        let _ = writeln!(out, "Вы можете создать дашборд Grafana для визуализации этих метрик. Вот простой пример панелей, которые вы можете включить:\n");
    } else {
        let _ = writeln!(out, "## Grafana Dashboard\n");
        let _ = writeln!(out, "You can create a Grafana dashboard to visualize these metrics. Here's a simple example of panels you might want to include:\n");
    }
    let _ = writeln!(out, "1. Connection counts by type\n2. Memory usage over time\n3. Client and server counts by pool\n4. Query and transaction performance percentiles\n5. Network traffic by pool\n");

    // Example Queries
    if russian {
        let _ = writeln!(out, "## Примеры запросов\n");
        let _ = writeln!(
            out,
            "Вот некоторые примеры запросов Prometheus, которые могут быть полезны:\n"
        );
    } else {
        let _ = writeln!(out, "## Example Queries\n");
        let _ = writeln!(
            out,
            "Here are some example Prometheus queries that you might find useful:\n"
        );
    }

    if russian {
        let _ = writeln!(out, "### Частота подключений\n");
    } else {
        let _ = writeln!(out, "### Connection Rate\n");
    }
    let _ = writeln!(
        out,
        "```\nrate(pg_doorman_connection_count{{type=\"total\"}}[5m])\n```\n"
    );

    if russian {
        let _ = writeln!(out, "### Использование пула\n");
    } else {
        let _ = writeln!(out, "### Pool Utilization\n");
    }
    let _ = writeln!(out, "```\nsum by (database) (pg_doorman_pools_clients{{status=\"active\"}}) / sum by (database) (pg_doorman_pools_servers{{status=\"active\"}} + pg_doorman_pools_servers{{status=\"idle\"}})\n```\n");

    if russian {
        let _ = writeln!(out, "### Медленные запросы\n");
    } else {
        let _ = writeln!(out, "### Slow Queries\n");
    }
    let _ = writeln!(
        out,
        "```\npg_doorman_pools_queries_percentile{{percentile=\"99\"}}\n```\n"
    );

    if russian {
        let _ = writeln!(out, "### Время ожидания клиента\n");
    } else {
        let _ = writeln!(out, "### Client Wait Time\n");
    }
    let _ = writeln!(out, "```\npg_doorman_pools_avg_wait_time\n```\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Read a reference doc file at runtime. Returns None if file doesn't exist
    /// (generated files may not be in the repo).
    fn read_reference_doc(rel_path: &str) -> Option<String> {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = Path::new(manifest_dir).join(rel_path);
        std::fs::read_to_string(&path).ok()
    }

    #[test]
    fn test_general_doc_en_matches_file() {
        let generated = generate_general_doc(false);
        if let Some(file_content) = read_reference_doc("documentation/en/src/reference/general.md")
        {
            assert_eq!(
                generated, file_content,
                "EN general.md is outdated. Run: cargo run -- generate-docs --all-languages"
            );
        }
    }

    #[test]
    fn test_pool_doc_en_matches_file() {
        let generated = generate_pool_doc(false);
        if let Some(file_content) = read_reference_doc("documentation/en/src/reference/pool.md") {
            assert_eq!(
                generated, file_content,
                "EN pool.md is outdated. Run: cargo run -- generate-docs --all-languages"
            );
        }
    }

    #[test]
    fn test_prometheus_doc_en_matches_file() {
        let generated = generate_prometheus_doc(false);
        if let Some(file_content) =
            read_reference_doc("documentation/en/src/reference/prometheus.md")
        {
            assert_eq!(
                generated, file_content,
                "EN prometheus.md is outdated. Run: cargo run -- generate-docs --all-languages"
            );
        }
    }

    #[test]
    fn test_general_doc_ru_matches_file() {
        let generated = generate_general_doc(true);
        if let Some(file_content) = read_reference_doc("documentation/ru/src/reference/general.md")
        {
            assert_eq!(
                generated, file_content,
                "RU general.md is outdated. Run: cargo run -- generate-docs --all-languages"
            );
        }
    }

    #[test]
    fn test_pool_doc_ru_matches_file() {
        let generated = generate_pool_doc(true);
        if let Some(file_content) = read_reference_doc("documentation/ru/src/reference/pool.md") {
            assert_eq!(
                generated, file_content,
                "RU pool.md is outdated. Run: cargo run -- generate-docs --all-languages"
            );
        }
    }

    #[test]
    fn test_prometheus_doc_ru_matches_file() {
        let generated = generate_prometheus_doc(true);
        if let Some(file_content) =
            read_reference_doc("documentation/ru/src/reference/prometheus.md")
        {
            assert_eq!(
                generated, file_content,
                "RU prometheus.md is outdated. Run: cargo run -- generate-docs --all-languages"
            );
        }
    }

    /// Check that EN and RU docs have the same field-level ### headings.
    /// Introductory headings (with spaces, i.e. human-readable titles) are
    /// intentionally localized and excluded from comparison.
    #[test]
    fn test_en_ru_docs_same_field_headings() {
        fn field_headings(doc: &str) -> Vec<&str> {
            doc.lines()
                .filter(|l| {
                    l.starts_with("### ")
                        && l[4..]
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
                .collect()
        }

        let pairs = vec![
            (generate_general_doc(false), generate_general_doc(true)),
            (generate_pool_doc(false), generate_pool_doc(true)),
            (
                generate_prometheus_doc(false),
                generate_prometheus_doc(true),
            ),
        ];

        for (en, ru) in pairs {
            let en_h = field_headings(&en);
            let ru_h = field_headings(&ru);
            assert_eq!(
                en_h, ru_h,
                "EN and RU docs have different field ### headings"
            );
        }
    }
}
