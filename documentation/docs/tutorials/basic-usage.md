---
title: Basic Usage
---

# PgDoorman Basic Usage Guide

PgDoorman is a high-performance PostgreSQL connection pooler based on PgCat. This comprehensive guide will help you get started with configuring, running, and managing PgDoorman for your PostgreSQL environment.

## Command Line Options

PgDoorman offers several command-line options to customize its behavior when starting the service:

```bash
$ pg_doorman --help

PgDoorman: Nextgen PostgreSQL Pooler (based on PgCat)

Usage: pg_doorman [OPTIONS] [CONFIG_FILE] [COMMAND]

Commands:
  generate  Generate configuration for pg_doorman by connecting to PostgreSQL and auto-detecting databases and users
  help      Print this message or the help of the given subcommand(s)

Arguments:
  [CONFIG_FILE]  [env: CONFIG_FILE=] [default: pg_doorman.toml]

Options:
  -l, --log-level <LOG_LEVEL>    [env: LOG_LEVEL=] [default: INFO]
  -F, --log-format <LOG_FORMAT>  [env: LOG_FORMAT=] [default: text] [possible values: text, structured, debug]
  -n, --no-color                 disable colors in the log output [env: NO_COLOR=]
  -d, --daemon                   run as daemon [env: DAEMON=]
  -h, --help                     Print help
  -V, --version                  Print version
```

### Available Options

| Option | Description |
|--------|-------------|
| `-d`, `--daemon` | Run in the background. Without this option, the process will run in the foreground.<br><br>In daemon mode, setting `daemon_pid_file` and `syslog_prog_name` is required. No log messages will be written to stderr after going into the background. |
| `-l`, `--log-level` | Set log level: `INFO`, `DEBUG`, or `WARN`. |
| `-F`, `--log-format` | Set log format. Possible values: `text`, `structured`, `debug`. |
| `-n`, `--no-color` | Disable colors in the log output. |
| `-V`, `--version` | Show version information. |
| `-h`, `--help` | Show help information. |

## Setup and Configuration

### Configuration File Structure

PgDoorman uses a [TOML format](https://toml.io/) configuration file to define its behavior. The configuration file is organized into several sections:

- `[general]` - Global settings for the PgDoorman service
- `[pools]` - Database pool definitions
- `[pools.<name>]` - Settings for a specific database pool
- `[pools.<name>.users.<n>]` - User settings for a specific database pool

!!! important
    Some parameters **must** be specified in the configuration file for PgDoorman to start, even if they have default values. For example, you must specify an admin username and password to access the administrative console.

### Minimal Configuration Example

Here's a minimal configuration example to get you started:

```toml
# Global settings
[general]
host = "0.0.0.0"    # Listen on all interfaces
port = 6432         # Port for client connections

# Admin credentials for the management console
admin_username = "admin"
admin_password = "admin"  # Change this in production!

# Database pools section
[pools]

# Example database pool
[pools.exampledb]
server_host = "127.0.0.1"  # PostgreSQL server address
server_port = 5432         # PostgreSQL server port
pool_mode = "transaction"  # Connection pooling mode

# User configuration for this pool
[pools.exampledb.users.0]
pool_size = 40             # Maximum number of connections in the pool
username = "doorman"       # Username for PostgreSQL server
password = "SCRAM-SHA-256$4096:6nD+Ppi9rgaNyP7...MBiTld7xJipwG/X4="  # Hashed password
```

For a complete list of configuration options and their descriptions, see the [Settings Reference Guide](../reference/settings.md).

### Automatic Configuration Generation

PgDoorman provides a powerful `generate` command that can automatically create a configuration file by connecting to your PostgreSQL server and detecting databases and users:

```bash
# View all available options
pg_doorman generate --help

# Generate a configuration file with default settings
pg_doorman generate --output pg_doorman.toml
```

The `generate` command supports several options:

| Option | Description |
|--------|-------------|
| `--host`, `-h` | PostgreSQL host to connect to (default: localhost) |
| `--port`, `-p` | PostgreSQL port to connect to (default: 5432) |
| `--user`, `-u` | PostgreSQL user to connect as (requires superuser privileges) |
| `--password` | PostgreSQL password to connect with |
| `--database`, `-d` | PostgreSQL database to connect to |
| `--ssl` | Use SSL/TLS for PostgreSQL connection |
| `--pool-size` | Pool size for the generated configuration (default: 40) |
| `--session-pool-mode`, `-s` | Use session pool mode instead of transaction mode |
| `--output`, `-o` | Output file for the generated configuration |

The command connects to your PostgreSQL server, automatically detects all databases and users, and creates a complete configuration file with appropriate settings. This is especially useful for quickly setting up PgDoorman in new environments or when you have many databases and users to configure.

!!! note "PostgreSQL Environment Variables"
    The `generate` command also respects standard PostgreSQL environment variables like `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, and `PGDATABASE`.

!!! warning "Authentication Required"
    If your PostgreSQL server requires authentication in pg_hba.conf, you will need to manually set the `server_password` parameter in the configuration file after using the `generate` command.

!!! warning "Superuser Privileges"
    Reading user information from PostgreSQL requires superuser privileges to access the `pg_shadow` table.

### Running PgDoorman

After creating your configuration file, you can run PgDoorman from the command line:

```bash
$ pg_doorman pg_doorman.toml
```

If you don't specify a configuration file, PgDoorman will look for `pg_doorman.toml` in the current directory.

### Connecting to PostgreSQL via PgDoorman

Once PgDoorman is running, connect to it instead of connecting directly to your PostgreSQL database:

```bash
$ psql -h localhost -p 6432 -U doorman exampledb
```

Your application's connection string should be updated to point to PgDoorman instead of directly to PostgreSQL:

```
postgresql://doorman:password@localhost:6432/exampledb
```

PgDoorman will handle the connection pooling transparently, so your application doesn't need to be aware that it's connecting through a pooler.

## Administration

### Admin Console

PgDoorman provides a powerful administrative interface that allows you to monitor and manage the connection pooler. You can access this interface by connecting to the special administration database named **pgdoorman**:

```bash
$ psql -h localhost -p 6432 -U admin pgdoorman
```

Once connected, you can view available commands:

```sql
pgdoorman=> SHOW HELP;
NOTICE:  Console usage
DETAIL:
	SHOW HELP|CONFIG|DATABASES|POOLS|POOLS_EXTENDED|CLIENTS|SERVERS|USERS|VERSION
	SHOW LISTS
	SHOW CONNECTIONS
	SHOW STATS
	RELOAD
    SHUTDOWN
	SHOW
```

!!! note "Protocol Compatibility"
    The admin console currently supports only the simple query protocol.
    Some database drivers use the extended query protocol for all commands, making them unsuitable for admin console access. In such cases, use the `psql` command-line client for administration.

!!! warning "Security"
    Only the user specified by `admin_username` in the configuration file is allowed to log in to the admin console. 
    Make sure to use a strong password for this account in production environments.

### Monitoring PgDoorman

The admin console provides several commands to monitor the current state of PgDoorman:

- `SHOW STATS` - View performance statistics
- `SHOW CLIENTS` - List current client connections
- `SHOW SERVERS` - List current server connections
- `SHOW POOLS` - View connection pool status
- `SHOW DATABASES` - List configured databases
- `SHOW USERS` - List configured users

These commands are described in detail in the [Admin Console Commands](#admin-console-commands) section below.

### Reloading Configuration

If you make changes to the `pg_doorman.toml` file, you can apply them without restarting the service:

```sql
pgdoorman=# RELOAD;
```

When you reload the configuration:

1. PgDoorman reads the updated configuration file
2. Changes to database connection parameters are detected
3. Existing server connections are closed when they're next released (according to the pooling mode)
4. New server connections immediately use the updated parameters

This allows you to make configuration changes with minimal disruption to your applications.

## Admin Console Commands

The admin console provides a set of commands to monitor and manage PgDoorman. These commands follow a SQL-like syntax and can be executed from any PostgreSQL client connected to the admin console.

### Show Commands

The `SHOW` commands display information about PgDoorman's operation. Each command provides different insights into the pooler's performance and current state.

#### SHOW STATS

The `SHOW STATS` command displays comprehensive statistics about PgDoorman's operation:

```sql
pgdoorman=> SHOW STATS;
```

Statistics are presented per database with the following metrics:

| Metric | Description |
|--------|-------------|
| `database` | The database name these statistics apply to |
| `total_xact_count` | Total number of SQL transactions processed since startup |
| `total_query_count` | Total number of SQL commands processed since startup |
| `total_received` | Total bytes of network traffic received from clients |
| `total_sent` | Total bytes of network traffic sent to clients |
| `total_xact_time` | Total microseconds spent in transactions (including idle in transaction) |
| `total_query_time` | Total microseconds spent actively executing queries |
| `total_wait_time` | Total microseconds clients spent waiting for a server connection |
| `avg_xact_count` | Average transactions per second in the last 15-second period |
| `avg_query_count` | Average queries per second in the last 15-second period |
| `avg_server_assignment_count` | Average server assignments per second in the last 15-second period |
| `avg_recv` | Average bytes received per second from clients |
| `avg_sent` | Average bytes sent per second to clients |
| `avg_xact_time` | Average transaction duration in microseconds |
| `avg_query_time` | Average query duration in microseconds |
| `avg_wait_time` | Average time clients spent waiting for a server in microseconds |

!!! tip "Performance Monitoring"
    Pay special attention to the `avg_wait_time` metric. If this value is consistently high, it may indicate that your pool size is too small for your workload.

#### SHOW SERVERS

The `SHOW SERVERS` command displays detailed information about all server connections:

```sql
pgdoorman=> SHOW SERVERS;
```

| Column | Description |
|--------|-------------|
| `server_id` | Unique identifier for the server connection |
| `server_process_id` | PID of the backend PostgreSQL server process (if available) |
| `database_name` | Name of the database this connection is using |
| `user` | Username PgDoorman uses to connect to the PostgreSQL server |
| `application_name` | Value of the `application_name` parameter set on the server connection |
| `state` | Current state of the connection: **active**, **idle**, or **used** |
| `wait` | Wait state of the connection: **idle**, **read**, or **write** |
| `transaction_count` | Total number of transactions processed by this connection |
| `query_count` | Total number of queries processed by this connection |
| `bytes_sent` | Total bytes sent to the PostgreSQL server |
| `bytes_received` | Total bytes received from the PostgreSQL server |
| `age_seconds` | Lifetime of the current server connection in seconds |
| `prepare_cache_hit` | Number of prepared statement cache hits |
| `prepare_cache_miss` | Number of prepared statement cache misses |
| `prepare_cache_size` | Number of unique prepared statements in the cache |

!!! info "Connection States"
    - **active**: The connection is currently executing a query
    - **idle**: The connection is available for use
    - **used**: The connection is allocated to a client but not currently executing a query

#### SHOW CLIENTS

The `SHOW CLIENTS` command displays information about all client connections to PgDoorman:

```sql
pgdoorman=> SHOW CLIENTS;
```

| Column | Description |
|--------|-------------|
| `client_id` | Unique identifier for the client connection |
| `database` | Name of the database (pool) the client is connected to |
| `user` | Username the client used to connect |
| `addr` | Client's IP address and port (IP:port) |
| `tls` | Whether the connection uses TLS encryption (**true** or **false**) |
| `state` | Current state of the client connection: **active**, **idle**, or **waiting** |
| `wait` | Wait state of the client connection: **idle**, **read**, or **write** |
| `transaction_count` | Total number of transactions processed for this client |
| `query_count` | Total number of queries processed for this client |
| `age_seconds` | Lifetime of the client connection in seconds |

!!! tip "Monitoring Long-Running Connections"
    The `age_seconds` column can help identify long-running connections that might be holding resources unnecessarily. Consider implementing connection timeouts in your application for idle connections.

#### SHOW POOLS

The `SHOW POOLS` command displays information about connection pools. A new pool entry is created for each (database, user) pair:

```sql
pgdoorman=> SHOW POOLS;
```

| Column | Description |
|--------|-------------|
| `database` | Name of the database |
| `user` | Username associated with this pool |
| `pool_mode` | Pooling mode in use: **session** or **transaction** |
| `cl_active` | Number of active client connections (linked to servers or idle) |
| `cl_waiting` | Number of client connections waiting for a server connection |
| `sv_active` | Number of server connections linked to clients |
| `sv_idle` | Number of idle server connections available for immediate use |
| `sv_login` | Number of server connections currently in the login process |
| `maxwait` | Maximum wait time in seconds for the oldest client in the queue |
| `maxwait_us` | Microsecond part of the maximum waiting time |

!!! warning "Performance Alert"
    If the `maxwait` value starts increasing, your server pool may not be handling requests quickly enough. This could be due to an overloaded PostgreSQL server or insufficient `pool_size` setting.

#### SHOW USERS

The `SHOW USERS` command displays information about all configured users:

```sql
pgdoorman=> SHOW USERS;
```

| Column | Description |
|--------|-------------|
| `name` | Username as configured in PgDoorman |
| `pool_mode` | Pooling mode assigned to this user: **session** or **transaction** |

#### SHOW DATABASES

The `SHOW DATABASES` command displays information about all configured database pools:

```sql
pgdoorman=> SHOW DATABASES;
```

| Column | Description |
|--------|-------------|
| `database` | Name of the configured database pool |
| `host` | Hostname of the PostgreSQL server PgDoorman connects to |
| `port` | Port number of the PostgreSQL server |
| `pool_size` | Maximum number of server connections for this database |
| `min_pool_size` | Minimum number of server connections to maintain |
| `reserve_pool_size` | Maximum number of additional connections allowed |
| `pool_mode` | Default pooling mode for this database |
| `max_connections` | Maximum allowed server connections (from `max_db_connections`) |
| `current_connections` | Current number of server connections for this database |

!!! tip "Connection Management"
    Monitor the ratio between `current_connections` and `pool_size` to ensure your pool is properly sized. If `current_connections` frequently reaches `pool_size`, consider increasing the pool size.

#### SHOW SOCKETS

The `SHOW SOCKETS` command displays low-level information about network sockets:

```sql
pgdoorman=> SHOW SOCKETS;
```

This command includes all information shown in `SHOW CLIENTS` and `SHOW SERVERS` plus additional low-level details about the socket connections.

#### SHOW VERSION

The `SHOW VERSION` command displays the PgDoorman version information:

```sql
pgdoorman=> SHOW VERSION;
```

This is useful for verifying which version you're running, especially after upgrades.

### Control Commands

PgDoorman provides control commands that allow you to manage the service operation directly from the admin console.

#### SHUTDOWN

The `SHUTDOWN` command gracefully terminates the PgDoorman process:

```sql
pgdoorman=> SHUTDOWN;
```

When executed:

1. PgDoorman stops accepting new client connections
2. Existing transactions are allowed to complete (within the configured timeout)
3. All connections are closed
4. The process exits

!!! warning "Service Interruption"
    Using the `SHUTDOWN` command will terminate the PgDoorman service, disconnecting all clients. Use this command with caution in production environments.

#### RELOAD

The `RELOAD` command refreshes PgDoorman's configuration without restarting the service:

```sql
pgdoorman=> RELOAD;
```

This command:

1. Rereads the configuration file
2. Updates all changeable settings
3. Applies changes to connection parameters for new connections
4. Maintains existing connections until they're released back to the pool

!!! tip "Zero-Downtime Configuration Changes"
    The `RELOAD` command allows you to modify most configuration parameters without disrupting existing connections. This is ideal for production environments where downtime must be minimized.

## Signal Handling

PgDoorman responds to standard Unix signals for control and management. These signals can be sent using the `kill` command (e.g., `kill -HUP <pid>`).

| Signal | Description | Effect |
|--------|-------------|--------|
| **SIGHUP** | Configuration reload | Equivalent to the `RELOAD` command in the admin console. Rereads the configuration file and applies changes to settings. |
| **SIGTERM** | Immediate shutdown | Forces PgDoorman to exit immediately. Active connections may be terminated abruptly. |
| **SIGINT** | Graceful shutdown | Initiates a binary upgrade process. The current process starts a new instance and gracefully transfers connections. See [Binary Upgrade Process](binary-upgrade.md) for details. |

!!! note "Process Management"
    In systemd-based environments, you can use `systemctl reload pg_doorman` to send SIGHUP and `systemctl restart pg_doorman` for a complete restart.