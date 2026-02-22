## PgDoorman: PostgreSQL Pooler

PgDoorman is a stable and high-performance alternative to [PgBouncer](https://www.pgbouncer.org/), [Odyssey](https://github.com/yandex/odyssey), or [PgCat](https://github.com/postgresml/pgcat).
It was created with the Unix philosophy in mind. Development focused on performance, efficiency, and reliability.
Additionally, PgDoorman provides improved driver support for languages like Go (pgx), .NET (npgsql), and asynchronous drivers for Python and Node.js.

[Get PgDoorman 3.2.4](tutorials/installation.md)

### Quick Start

Run PgDoorman instantly using Docker:

```bash
docker run -p 6432:6432 \
  -v $(pwd)/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml \
  ghcr.io/ozontech/pg_doorman
```

*For more details, see the [Installation Guide](tutorials/installation.md).*


### Why not multi-PgBouncer?

Why do we think that using [multiple instances of PgBouncer](https://www.pgbouncer.org/config.html#so_reuseport) is not a suitable solution?
This approach has problems with reusing prepared statements and strange and inefficient control over query cancellation.
Additionally, the main issue we have encountered is that the operating system distributes new clients round-robin,
but each client can disconnect at any time, leading to an imbalance after prolonged use.

### Why not Odyssey?

We had difficulties using NPGSQL and SCRAM, as well as with `prepared_statements` support.
However, the main serious problem related to data consistency and, for a long time, we were unable to solve it.

### Differences from PgCat

While PgDoorman was initially based on the PgCat project, it has since evolved into a standalone solution with its own set of features.
Some of the key differences include:

- Performance improvements compared to PgCat/PgBouncer/Odyssey.
- Support for extended protocol with popular programming language drivers.
- Enhanced monitoring metrics to improve visibility into database activity.
- Careful resource management to avoid memory issues (`max_memory_usage`, `message_size_to_be_stream`).
- SCRAM client/server authentication support.
- [Gracefully binary upgrade](tutorials/binary-upgrade.md).
- Supporting JWT for service-to-database authentication.
- Many micro-optimizations (for example, the time spent with the client is longer than the server's busy time).

### Additional Binary: patroni_proxy

This repository also includes [patroni_proxy](tutorials/patroni-proxy.md) — a specialized high-performance TCP proxy for Patroni-managed PostgreSQL clusters. Unlike HAProxy + confd, it preserves existing connections during cluster topology changes and provides native role-based routing with replication lag awareness.
