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
- **Transparent Pooling**: Connect applications to the database without adapting them to work with transaction pool mode

## Comparison with Alternatives

PgDoorman is a good alternative to [PgBouncer](https://www.pgbouncer.org/), [Odyssey](https://github.com/yandex/odyssey), and [PgCat](https://github.com/postgresml/pgcat).

We aimed to create a more efficient, multithreaded version of PgBouncer with a focus on performing pooler tasks efficiently and fast, in line with the Unix philosophy. While we've removed load balancing and sharding, we believe it's more efficient to handle these at the application level.

A key advantage of PgDoorman is its ability to work transparently with applications in transaction pool mode. Unlike some other poolers, applications can connect to PgDoorman without requiring modifications to handle the complexities of transaction pooling, such as connection state management between transactions.

Over two years of use, we've improved driver support for languages like Go (pgx), .NET (npgsql), and asynchronous drivers for Python and Node.js.

## Additional Binary: patroni_proxy

This repository also includes `patroni_proxy` — a specialized high-performance TCP proxy for Patroni-managed PostgreSQL clusters. Following the Unix philosophy of "do one thing and do it well", `patroni_proxy` focuses exclusively on TCP load balancing and failover for Patroni clusters.

**Key advantages over HAProxy:**
- **Zero-downtime connection management** — existing connections are preserved during cluster topology changes
- **Hot upstream updates** — automatic discovery of cluster members via Patroni REST API without connection drops
- **Role-based routing** — route connections to leader, sync replicas, or async replicas based on configuration

### Recommended Deployment Architecture

For optimal performance, we recommend a two-tier architecture:

- **pg_doorman** should be deployed **close to PostgreSQL servers** — it handles connection pooling, prepared statement caching, and protocol-level optimizations that benefit from low latency to the database
- **patroni_proxy** should be deployed **close to application clients** — it handles TCP routing and failover, distributing connections across the cluster without the overhead of connection pooling

This separation allows each component to excel at its specific task while providing both high availability and optimal performance.

For more details, see the [patroni_proxy documentation](src/bin/patroni_proxy/README.md).

## Installation

### System Requirements

- Linux (recommended) or macOS or Windows
- PostgreSQL server (version 10 or higher)
- Sufficient memory for connection pooling (depends on expected load)

### Pre-built Binaries (Recommended)

The simplest way to install PgDoorman is to download a pre-built binary from the [GitHub releases page](https://github.com/ozontech/pg_doorman/releases).

### Ubuntu/Debian (Launchpad PPA)

For Ubuntu users, PgDoorman is available via Launchpad PPA:

```bash
# Add the PPA repository
sudo add-apt-repository ppa:vadv/pg-doorman
sudo apt-get update

# Install pg-doorman
sudo apt-get install pg-doorman
```

Supported Ubuntu versions: 22.04 (Jammy), 24.04 (Noble), 25.04 (Plucky).

For more information, visit the [Launchpad PPA page](https://launchpad.net/~pg-doorman/+archive/ubuntu/pg-doorman).

### Fedora/RHEL/Rocky/Alma (COPR)

For Fedora and RHEL-based distributions, PgDoorman is available via Fedora COPR:

```bash
# Enable the COPR repository
sudo dnf copr enable vadvya/pg-doorman

# Install pg-doorman
sudo dnf install pg-doorman
```

Supported distributions: Fedora 39+, RHEL/Rocky/Alma 8+.

For more information, visit the [COPR project page](https://copr.fedorainfracloud.org/coprs/vadvya/pg-doorman/).

### Docker Installation

PgDoorman provides an official Docker image that you can use directly:

```bash
# Pull the official Docker image
docker pull ghcr.io/ozontech/pg_doorman

# Run PgDoorman with your configuration
docker run -p 6432:6432 \
  -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml \
  --rm -t -i ghcr.io/ozontech/pg_doorman
```

Alternatively, you can build and run PgDoorman using Docker:

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

To connect to pg_doorman running in Docker Compose, use the following command:

```bash
PGPASSWORD=password psql -h 127.0.0.1 -p 6432 -d exampledb -U doorman
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
server_host = "127.0.0.1"  # PostgreSQL server address (or unix-domain socket like /var/run/postgresql)
server_port = 5432         # PostgreSQL server port
pool_mode = "transaction"  # Connection pooling mode

# User configuration for this pool
[pools.exampledb.users.0]
pool_size = 40             # Maximum number of connections in the pool
username = "doorman"       # Username for PostgreSQL server
password = "md5xxxxxx"     # Password hash (md5/scram) for authentication (you can use `select * from pg_shadow` or `pg_doorman generate --help` to fetch this value).
# server_username = "doorman" # Username for PostgreSQL server (required if PostgreSQL server requires authentication in pg_hba.conf)
# server_password = "your_password" # Plain password for PostgreSQL server (required if PostgreSQL server requires authentication in pg_hba.conf)
# server_database = "exampledb" # Database for PostgreSQL server (optional)
```

### Automatic Configuration Generation

PgDoorman provides a powerful `generate` command that can automatically create a configuration file by connecting to your PostgreSQL server and detecting databases and users:

```bash
# View all available options
pg_doorman generate --help

# Generate a configuration file with default settings
pg_doorman generate --output pg_doorman.toml

# Connect to a specific PostgreSQL server
pg_doorman generate --host db.example.com --port 5432 --output pg_doorman.toml

# Connect with specific credentials (requires superuser privileges)
pg_doorman generate --user postgres --password secret --output pg_doorman.toml

# Generate configuration with SSL/TLS enabled
pg_doorman generate --ssl --output pg_doorman.toml

# Set custom pool size and pool mode
pg_doorman generate --pool-size 20 --session-pool-mode --output pg_doorman.toml
```

The `generate` command connects to your PostgreSQL server, automatically detects all databases and users, and creates a complete configuration file with appropriate settings. This is especially useful for quickly setting up PgDoorman in new environments or when you have many databases and users to configure.

**Warning:** If your PostgreSQL server requires authentication in pg_hba.conf, you will need to manually set the `server_password` parameter in the configuration file after using the `generate` command.

Key features of the `generate` command:
- Automatically detects all non-template databases
- Retrieves user authentication information from PostgreSQL
- Configures appropriate pool settings for each database
- Supports both regular and SSL/TLS connections
- Can use standard PostgreSQL environment variables (PGHOST, PGPORT, etc.)
- Allows customization of pool size and pool mode

### Client access control (pg_hba)

PgDoorman supports PostgreSQL-style `pg_hba.conf` rules via the `general.pg_hba` parameter in `pg_doorman.toml`.
You can provide rules inline, or via `{ path = "..." }` to a file. See the full reference with examples in
`documentation/docs/reference/general.md` (section "pg_hba").

Trust behavior: when a matching rule uses `trust`, PgDoorman accepts the connection without asking the client
for a password, even if the user has an MD5 or SCRAM password configured. TLS constraints from the rule are honored
(`hostssl` requires TLS, `hostnossl` forbids TLS).

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

## Prometheus Metrics

PgDoorman includes a built-in Prometheus exporter that runs on port 9127.
This allows you to monitor the application using Prometheus and visualize the metrics with tools like Grafana.

Read more about the available prometheus metrics in the [Prometheus Documentation](https://ozontech.github.io/pg_doorman/latest/reference/prometheus/).

## Binary Upgrade Process

PgDoorman supports seamless binary upgrades that allow you to update the software with minimal disruption to your database connections.

When you send a `SIGINT` signal to the PgDoorman process, the binary upgrade process is initiated:

1. The current PgDoorman instance executes the exec command and starts a new, daemonized process
2. The new process uses the `SO_REUSE_PORT` socket option, allowing the operating system to distribute incoming traffic to the new instance
3. The old instance then closes its socket for incoming connections
4. Existing connections are handled gracefully during the transition

## Contributing

### Prerequisites

For running integration tests, you only need:

- [Docker](https://docs.docker.com/get-docker/) (required)
- [Make](https://www.gnu.org/software/make/) (required)

**Nix installation is NOT required** — test environment reproducibility is ensured by Docker containers.

For local development (optional): [Rust](https://www.rust-lang.org/tools/install), [Git](https://git-scm.com/downloads)

### Integration Testing

PgDoorman uses BDD tests with a Docker-based test environment. All tests are **reproducible** — they run inside Docker containers with identical environments.

```bash
# From project root directory:

# Pull the test image
make pull

# Run all BDD tests
make test-bdd

# Run tests with specific tag
make test-bdd TAGS=@copy-protocol
make test-bdd TAGS=@cancel
make test-bdd TAGS=@go

# Enable debug output
DEBUG=1 make test-bdd TAGS=@my-tag

# Open interactive shell in test container
make shell
```

For detailed information on writing tests (shell tests, Rust protocol-level tests) and contributing, see the [Contributing Guide](https://ozontech.github.io/pg_doorman/latest/tutorials/contributing/).

## Documentation

For more detailed information, please visit the [PgDoorman Documentation](https://ozontech.github.io/pg_doorman/).