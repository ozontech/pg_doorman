![pg_doorman](/static/logo_color_bg.png)

# PgDoorman: High-Performance PostgreSQL Connection Pooler

PgDoorman is a high-performance PostgreSQL connection pooler that acts as middleware between your applications and PostgreSQL servers, efficiently managing database connections to improve performance and resource utilization.

When an application connects to PgDoorman, it behaves exactly like a PostgreSQL server. Behind the scenes, PgDoorman either creates a new connection to the actual PostgreSQL server or reuses an existing connection from its pool, significantly reducing connection overhead.

## Key Benefits

- **Reduced Connection Overhead**: Minimizes the performance impact of establishing new database connections
- **Resource Optimization**: Limits the number of connections to your PostgreSQL server, control prepared statements
- **Improved Scalability**: Allows more client applications to connect to your database
- **Connection Management**: Provides tools to monitor and manage database connections
- **Seamless Binary Upgrades**: Update the software with minimal disruption to your database connections

## Comparison with Alternatives

PgDoorman is a good alternative to [PgBouncer](https://www.pgbouncer.org/), [Odyssey](https://github.com/yandex/odyssey), and [PgCat](https://github.com/postgresml/pgcat).

We aimed to create a more efficient, multithreaded version of PgBouncer with a focus on performing pooler tasks efficiently and fast, in line with the Unix philosophy. While we've removed load balancing and sharding, we believe it's more efficient to handle these at the application level.

Over two years of use, we've improved driver support for languages like Go (pgx), .NET (npgsql), and asynchronous drivers for Python and Node.js.

## Installation

### System Requirements

- Linux (recommended) or macOS or Windows
- PostgreSQL server (version 10 or higher)
- Sufficient memory for connection pooling (depends on expected load)

### Pre-built Binaries (Recommended)

The simplest way to install PgDoorman is to download a pre-built binary from the [GitHub releases page](https://github.com/ozontech/pg_doorman/releases).

### Docker Installation

You can build and run PgDoorman using Docker:

```bash
# Build the Docker image
docker build -t pg_doorman -f Dockerfile .

# Run PgDoorman with your configuration
docker run -p 6432:6432 \
  -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml \
  --rm -t -i pg_doorman
```

For a more complete setup including PostgreSQL, you can use Docker Compose:

```bash
cd example
docker compose up
```

## Basic Usage

### Configuration

PgDoorman uses a TOML format configuration file. Here's a minimal configuration example:

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
password = "your_password" # Password for PostgreSQL server
```

### Running PgDoorman

After creating your configuration file, you can run PgDoorman from the command line:

```bash
$ pg_doorman pg_doorman.toml
```

### Connecting to PostgreSQL via PgDoorman

Once PgDoorman is running, connect to it instead of connecting directly to your PostgreSQL database:

```bash
$ psql -h localhost -p 6432 -U doorman exampledb
```

Your application's connection string should be updated to point to PgDoorman instead of directly to PostgreSQL:

```
postgresql://doorman:password@localhost:6432/exampledb
```

## Admin Console

PgDoorman provides a powerful administrative interface that allows you to monitor and manage the connection pooler. You can access this interface by connecting to the special administration database named **pgdoorman**:

```bash
$ psql -h localhost -p 6432 -U admin pgdoorman
```

The admin console provides several commands to monitor the current state of PgDoorman:

- `SHOW STATS` - View performance statistics
- `SHOW CLIENTS` - List current client connections
- `SHOW SERVERS` - List current server connections
- `SHOW POOLS` - View connection pool status
- `SHOW DATABASES` - List configured databases
- `SHOW USERS` - List configured users

If you make changes to the configuration file, you can apply them without restarting the service:

```sql
pgdoorman=# RELOAD;
```

## Binary Upgrade Process

PgDoorman supports seamless binary upgrades that allow you to update the software with minimal disruption to your database connections.

When you send a `SIGINT` signal to the PgDoorman process, the binary upgrade process is initiated:

1. The current PgDoorman instance executes the exec command and starts a new, daemonized process
2. The new process uses the `SO_REUSE_PORT` socket option, allowing the operating system to distribute incoming traffic to the new instance
3. The old instance then closes its socket for incoming connections
4. Existing connections are handled gracefully during the transition

## Contributing

### Prerequisites

Before you begin, make sure you have the following installed:

- [Rust](https://www.rust-lang.org/tools/install) (latest stable version)
- [Git](https://git-scm.com/downloads)
- [Docker](https://docs.docker.com/get-docker/) (optional, for running tests)
- [Make](https://www.gnu.org/software/make/) (optional, for running test scripts)

### Local Development

1. **Fork and clone the repository**
2. **Build the project**: `cargo build`
3. **Configure PgDoorman**: Copy and modify the example configuration
4. **Run PgDoorman**: `cargo run --release`
5. **Run tests**: `cargo test`

For more detailed information on contributing, please see the [Contributing Guide](https://ozontech.github.io/pg_doorman/tutorials/contributing/).

## Documentation

For more detailed information, please visit the [PgDoorman Documentation](https://ozontech.github.io/pg_doorman/).