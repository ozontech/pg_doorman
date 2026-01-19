---
title: General Settings
---

# Settings

## Configuration File Format

pg_doorman supports two configuration file formats:

* **YAML** (`.yaml`, `.yml`) - The primary and recommended format for new configurations.
* **TOML** (`.toml`) - Supported for backward compatibility with existing configurations.

The format is automatically detected based on the file extension. Both formats support the same configuration options and can be used interchangeably.

### Example YAML Configuration (Recommended)

```yaml
general:
  host: "0.0.0.0"
  port: 6432
  admin_username: "admin"
  admin_password: "admin"

pools:
  mydb:
    server_host: "localhost"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "myuser"
        password: "mypassword"
        pool_size: 40
```

### Example TOML Configuration (Legacy)

```toml
[general]
host = "0.0.0.0"
port = 6432
admin_username = "admin"
admin_password = "admin"

[pools.mydb]
server_host = "localhost"
server_port = 5432
pool_mode = "transaction"

[[pools.mydb.users]]
username = "myuser"
password = "mypassword"
pool_size = 40
```

### Generate Command

The `generate` command can output configuration in either format. The format is determined by the output file extension:

```bash
# Generate YAML configuration (recommended)
pg_doorman generate --output config.yaml

# Generate TOML configuration (for backward compatibility)
pg_doorman generate --output config.toml
```

### Include Files

Include files can be in either format, and you can mix formats. For example, a YAML main config can include TOML files and vice versa:

```yaml
include:
  files:
    - "pools.yaml"
    - "users.toml"
```

## Human-Readable Values

pg_doorman supports human-readable formats for duration and byte size values, while maintaining backward compatibility with numeric values.

### Duration Format

Duration values can be specified as:

* **Plain numbers**: interpreted as milliseconds (e.g., `5000` = 5 seconds)
* **String with suffix**:
    * `ms` - milliseconds (e.g., `"100ms"`)
    * `s` - seconds (e.g., `"5s"` = 5000 milliseconds)
    * `m` - minutes (e.g., `"5m"` = 300000 milliseconds)
    * `h` - hours (e.g., `"1h"` = 3600000 milliseconds)
    * `d` - days (e.g., `"1d"` = 86400000 milliseconds)

**Examples:**
```yaml
general:
  # All these are equivalent (3 seconds):
  # connect_timeout: 3000      # backward compatible (milliseconds)
  # connect_timeout: "3s"      # human-readable
  # connect_timeout: "3000ms"  # explicit milliseconds
  connect_timeout: "3s"
  idle_timeout: "5m"         # 5 minutes
  server_lifetime: "1h"      # 1 hour
```

### Byte Size Format

Byte size values can be specified as:

* **Plain numbers**: interpreted as bytes (e.g., `1048576` = 1 MB)
* **String with suffix** (case-insensitive):
    * `B` - bytes (e.g., `"1024B"`)
    * `K` or `KB` - kilobytes (e.g., `"1K"` or `"1KB"` = 1024 bytes)
    * `M` or `MB` - megabytes (e.g., `"1M"` or `"1MB"` = 1048576 bytes)
    * `G` or `GB` - gigabytes (e.g., `"1G"` or `"1GB"` = 1073741824 bytes)

Note: Uses binary prefixes (1 KB = 1024 bytes, not 1000 bytes).

**Examples:**
```yaml
general:
  # All these are equivalent (256 MB):
  # max_memory_usage: 268435456  # backward compatible (bytes)
  # max_memory_usage: "256MB"    # human-readable
  # max_memory_usage: "256M"     # short form
  max_memory_usage: "256MB"
  unix_socket_buffer_size: "1MB" # 1 MB
  worker_stack_size: "8MB"       # 8 MB
```

## General Settings

### host

Listen host (TCP v4 only).

Default: `"0.0.0.0"`.

### port

Listen port for incoming connections.

Default: `6432`.

### backlog

TCP backlog for incoming connections. A value of zero sets the `max_connections` as value for the TCP backlog.

Default: `0`.

### max_connections

The maximum number of clients that can connect to the pooler simultaneously. When this limit is reached:
* A client connecting without SSL will receive the expected error (code: `53300`, message: `sorry, too many clients already`).
* A client connecting via SSL will see a message indicating that the server does not support the SSL protocol.

Default: `8192`.

### max_concurrent_creates

Maximum number of server connections that can be created concurrently per pool. This setting uses a semaphore to limit parallel connection creation, which significantly improves performance during cold start and burst scenarios.

Higher values allow faster pool warm-up but may increase load on the PostgreSQL server during connection storms. Lower values provide more gradual connection creation.

Default: `4`.

### tls_mode

The TLS mode for incoming connections. It can be one of the following:

* `allow` - TLS connections are allowed but not required. The pg_doorman will attempt to establish a TLS connection if the client requests it.
* `disable` - TLS connections are not allowed. All connections will be established without TLS encryption.
* `require` - TLS connections are required. The pg_doorman will only accept connections that use TLS encryption.
* `verify-full` - TLS connections are required and the pg_doorman will verify the client certificate. This mode provides the highest level of security.

Default: `"allow"`.

### tls_ca_file

The file containing the CA certificate to verify the client certificate. This is required when `tls_mode` is set to `verify-full`.

Default: `None`.

### tls_private_key

The path to the private key file for TLS connections. This is required to enable TLS for incoming client connections. Must be used together with `tls_certificate`.

Default: `None`.

### tls_certificate

The path to the certificate file for TLS connections. This is required to enable TLS for incoming client connections. Must be used together with `tls_private_key`.

Default: `None`.

### tls_rate_limit_per_second

Limit the number of simultaneous attempts to create a TLS session.
Any value other than zero implies that there is a queue through which clients must pass in order to establish a TLS connection.
In some cases, this is necessary in order to launch an application that opens many connections at startup (the so-called "hot start").

Default: `0`.

### daemon_pid_file

Enabling this setting enables daemon mode. Comment this out if you want to run pg_doorman in the foreground with `-d`.

Default: `None`.

### syslog_prog_name

When specified, pg_doorman starts sending messages to syslog (using /dev/log or /var/run/syslog).
Comment this out if you want to log to stdout.

Default: `None`.

### log_client_connections 

Log client connections for monitoring.

Default: `true`.

### log_client_disconnections 

Log client disconnections for monitoring.

Default: `true`.

### worker_threads

The number of worker processes (posix threads) that async serve clients, which affects the performance of pg_doorman.
The more workers there are, the faster the system works, but only up to a certain limit (cpu count).

This parameter also controls the number of shards in internal concurrent hash maps (DashMap).
The shard count is calculated as `worker_threads * 4` rounded up to the nearest power of 2 (minimum 4 shards).
This is important for Kubernetes deployments where CPU count detection may be incorrect, causing unnecessary overhead.

Default: `4`.

### worker_cpu_affinity_pinning

Automatically assign workers to different CPUs (man 3 cpu_set).

Default: `false`.

### tokio_global_queue_interval

[Tokio runtime settings](https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html#method.global_queue_interval).
Controls how often the scheduler checks the global task queue.
Modern tokio versions handle this well by default, so this parameter is optional.

Default: not set (uses tokio's default).

### tokio_event_interval

[Tokio runtime settings](https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html#method.event_interval).
Controls how often the scheduler checks for external events (I/O, timers).
Modern tokio versions handle this well by default, so this parameter is optional.

Default: not set (uses tokio's default).

### worker_stack_size

[Tokio runtime settings](https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html#method.thread_stack_size).
Sets the stack size for worker threads.
Modern tokio versions handle this well by default, so this parameter is optional.

Default: not set (uses tokio's default).

### max_blocking_threads

[Tokio runtime settings](https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html#method.max_blocking_threads).
Sets the maximum number of threads for blocking operations.
Modern tokio versions handle this well by default, so this parameter is optional.

Default: not set (uses tokio's default).


### connect_timeout

Connection timeout to server in milliseconds.

Default: `3000` (3 sec).

### query_wait_timeout

Maximum time to wait for a query to complete, in milliseconds.

Default: `5000` (5 sec).

### idle_timeout

Server idle timeout in milliseconds.

Default: `300000000` (5000 min).

### server_lifetime

Server lifetime in milliseconds.

Default: `300000` (5 min).

### server_round_robin

In transactional pool mode, we can choose whether the last free server backend will be used or the next one will be selected.
By default, the LRU (Least Recently Used) method is used, which has a positive impact on performance.

Default: `false`.

### sync_server_parameters

If enabled, we strive to restore the parameters (via query `SET`) that were set by the client (and application_name)
in transaction mode in other server backends. By default, this is disabled (false) due to performance.
If you need to know `application_name`, but don't want to experience performance issues due to constant server queries `SET`,
you can consider creating a separate pool for each application and using the `application_name` parameter in the `pool` settings.

Default: `false`.

### tcp_so_linger

By default, pg_doorman send `RST` instead of keeping the connection open for a long time.

Default: `0`.

### tcp_no_delay

TCP_NODELAY to disable Nagle's algorithm for lower latency.

Default: `true`.

### tcp_keepalives_count

Keepalive enabled by default and overwrite OS defaults.

Default: `5`.

### tcp_keepalives_idle

Default: `5`.

### tcp_keepalives_interval

Default: `1`.

### unix_socket_buffer_size

Buffer size for read and write operations when connecting to PostgreSQL via a unix socket.

Default: `1048576`.

### admin_username

Access to the virtual admin database is carried out through the administrator's username and password.

Default: `"admin"`.

### admin_password

Access to the virtual admin database is carried out through the administrator's username and password.
It should be replaced with your secret.

Default: `"admin"`.

### prepared_statements

Switcher to enable/disable caching of prepared statements.

Default: `true`.

### prepared_statements_cache_size

Cache size of prepared requests on the server side.

Default: `8192`.

### message_size_to_be_stream

Data responses from the server (message type 'D') greater than this value will be
transmitted through the proxy in small chunks (1 MB).

Default: `1048576`.

### max_memory_usage

We calculate the total amount of memory used by the internal buffers for all current queries.
If the limit is reached, the client will receive an error (256 MB).

Default: `268435456`.

### shutdown_timeout

With a graceful shutdown, we wait for transactions to be completed within this time limit (10 seconds).

Default: `10000`.

### proxy_copy_data_timeout

Maximum time to wait for data copy operations during proxying, in milliseconds.

Default: `15000` (15 sec).


### server_tls

Enable TLS for connections to the PostgreSQL server. When enabled, pg_doorman will attempt to establish TLS connections to the backend PostgreSQL servers.

Default: `false`.

### verify_server_certificate

Verify the PostgreSQL server's TLS certificate when connecting with TLS. This setting is only relevant when `server_tls` is enabled.

Default: `false`.

### hba

The list of IP addresses from which it is permitted to connect to the pg-doorman.

### pg_hba

New-style client access control in native PostgreSQL `pg_hba.conf` format. This allows you to define fine-grained access rules similar to PostgreSQL, including per-database, per-user, address ranges, and TLS requirements.

You can specify `general.pg_hba` in three ways:

- As a multi-line string with the contents of a `pg_hba.conf` file
- As an object with `path` that points to a file on disk
- As an object with `content` containing the rules as a string

Examples:

```toml
[general]
# Inline content (triple-quoted TOML string)
pg_hba = """
# type   database  user   address         method
host     all       all    10.0.0.0/8      md5
hostssl  all       all    0.0.0.0/0       scram-sha-256
hostnossl all      all    192.168.1.0/24  trust
"""

# Or load from file
# pg_hba = { path = "./pg_hba.conf" }

# Or embed as a single-line string
# pg_hba = { content = "host all all 127.0.0.1/32 trust" }
```

Supported fields and methods:
- Connection types: `local`, `host`, `hostssl`, `hostnossl` (TLS-aware matching is honored)
- Database matcher: a name or `all`
- User matcher: a name or `all`
- Address: CIDR form like `1.2.3.4/32` or `::1/128` (required for non-`local` rules)
- Methods: `trust`, `md5`, `scram-sha-256` (unknown methods are parsed but treated as not-allowed by the checker)

Precedence and compatibility:
- `general.pg_hba` supersedes the legacy `general.hba` list. You cannot set both at the same time; configuration validation will reject this combination.
- Rules are evaluated in order; the first matching rule decides the outcome.

Behavior of method = trust:
- When a matching rule has `trust`, PgDoorman will accept the connection without requesting a password. This mirrors PostgreSQL behavior.
- Specifically, if `trust` matches, PgDoorman will skip password verification even if the user has an `md5` or `scram-sha-256` password stored. This affects both MD5 and SCRAM flows.
- TLS constraints from the rule are respected: `hostssl` requires TLS, `hostnossl` forbids TLS.

Admin console access:
- `general.pg_hba` rules apply to the special admin database `pgdoorman` as well.
- This means you can allow admin access with the `trust` method when a matching rule is present, for example:
  ```
  host  pgdoorman  admin  127.0.0.1/32  trust
  ```

Notes and limitations:
- Only a minimal subset of `pg_hba.conf` is supported that is sufficient for most proxy use-cases (type, database, user, address, method). Additional options (like `clientcert`) are currently ignored.
- For authentication methods other than `trust`, PgDoorman performs the corresponding challenge/response with the client.
- For Talos/JWT/PAM flows configured at the pool/user level, `trust` still bypasses the client password prompt; however, those modes may be used when `trust` does not match.

### pooler_check_query

This query will not be sent to the server if it is run as a SimpleQuery.
It can be used to check the connection at the application level.

Default: `;`.
