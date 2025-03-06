## PgDoorman: PostgreSQL Pooler

PgDoorman: PostgreSQL pooler that aims to provide high-performance database access for your applications. 
PgDoorman offers features that make it a great alternative to PgBouncer/Odyssey, with additional benefits for database operations.

### Status

PgDoorman has been stable and in production for a while, serving tens of thousands of servers and processing millions of queries per second with ease.

### Differences from PgCat

While PgDoorman was initially based on the PgCat project, it has since evolved into a standalone solution with its own set of features.
Some of the key differences include:

- Performance improvements compared to PgCat/PgBouncer/Odyssey.
- Support for extended protocol and popular programming language drivers.
- Enhanced monitoring metrics for better visibility into database activity.
- Careful resource management to avoid memory issues.
- SCRAM client authentication support.
- Gracefully binary upgrade.
- Custom JWT inter-service authentication support.
- Micro-optimizations aimed at improving communication with database and management.

### How to try

With docker image:

1. docker build -t pg_doorman -f Dockerfile .
2. docker run -p 6432:6432 -v /path/to/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml --rm -t -i pg_doorman

With docker compose:

1. cd example && docker compose up
2. connection string: `postgresql://doorman:password@127.0.0.1:6432/doorman`

### Local development

1. **Install Rust** (the latest stable version will work great)
2. Run `cargo build --release` to get better benchmarks.
3. Adjust the configuration in `pg_doorman.toml` to match your setup (this step is optional, given next).
4. Execute `cargo run --release`. You're now ready to go!

### Benchmarks

[benchmarks here](/BENCHMARKS.md)