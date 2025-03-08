## PgDoorman: PostgreSQL Pooler

PgDoorman is a good alternative to [PgBouncer](https://www.pgbouncer.org/), [Odyssey](https://github.com/yandex/odyssey), and [PgCat](https://github.com/postgresml/pgcat).
We aimed to create a more efficient, multithreaded version of PgBouncer
and with focus on performing pooler tasks efficiently and fast, in line with the Unix philosophy.
While we’ve removed load balancing and sharding, we believe it’s more efficient to handle these at the application level.
Over two years of use, we've improved driver support for languages like Go (pgx), .NET (npgsql), and asynchronous drivers for Python and Node.js.

### Why not multi-PgBouncer?

Why do we think that using [multiple instances of PgBouncer](https://www.pgbouncer.org/config.html#so_reuseport) is not a suitable solution?
This approach has problems with reusing prepared statements and strange and inefficient control over query cancellation.
Additionally, the main issue we have encountered is that the operating system distributes new clients round-robin,
but each client can disconnect at any time, leading to an imbalance after prolonged use.

### Why not Odyssey?

We had difficulties using NPGSQL and SCRAM, as well as with `prepared_statements` support.
However, the main serious problem related to data consistency and, for a long time, we were unable to solve it.

### Status

PgDoorman has been a stable and reliable product for a while now, serving tens of thousands of servers and handling millions of queries per second.

### Differences from PgCat

While PgDoorman was initially based on the PgCat project, it has since evolved into a standalone solution with its own set of features.
Some of the key differences include:

- Performance improvements compared to PgCat/PgBouncer/Odyssey.
- Support for extended protocol with popular programming language drivers.
- Enhanced monitoring metrics to improve visibility into database activity..
- Careful resource management to avoid memory issues (`max_memory_usage`, `message_size_to_be_stream`).
- SCRAM client/server authentication support.
- [Gracefully binary upgrade](/BINARY_UPGRADE.md).
- Supporting JWT for service-to-database authentication.
- Many micro-optimizations (for example, the time spent with the client is longer than the server's busy time).

### Config

[See Configuration](/pg_doorman.toml).

### How to try

With Docker image:

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

[View benchmark results for detailed performance comparisons](/BENCHMARKS.md)
