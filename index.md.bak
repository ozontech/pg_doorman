# PgDoorman

A multi-threaded PostgreSQL connection pooler written in Rust. Drop-in replacement for [PgBouncer](https://www.pgbouncer.org/) and [Odyssey](https://github.com/yandex/odyssey), and an alternative to [PgCat](https://github.com/postgresml/pgcat). Three years in production at Ozon under Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), and Node.js workloads.

[Get PgDoorman {{VERSION}}](tutorials/installation.md) · [Comparison](comparison.md) · [Benchmarks](benchmarks.md)

## Headline features

```admonish success title="Anonymous Parse Caching"
In the extended protocol, many drivers send short parameterised queries as an unnamed `Parse`. PgDoorman rewrites that `Parse` on the PostgreSQL side to an internal `DOORMAN_<N>` name and keeps the mapping in the pool. Later `Bind`s for the same query shape reuse prepared backend state.

That cuts repeated PostgreSQL planner work on hot OLTP paths without application changes. PgBouncer 1.21+ and Odyssey can track named prepared statements, but they forward anonymous `Parse` unchanged; PgDoorman covers the driver-default case.

The cache is bounded: anonymous entries expire after idle timeout, and named entries are freed after their last reference. `SHOW INTERNER` exposes query-text memory; Prometheus metrics expose hits, misses, and evictions.

[Read more →](tutorials/prepared-statements.md)
```

```admonish success title="Pool Coordinator"
When several user pools share one database, the global limit should protect PostgreSQL, not just queue clients. PgDoorman enforces `max_db_connections`: once the cap is reached, the coordinator closes an idle connection from a pool with spare capacity and gives the slot to a client waiting for a backend.

Donors are ranked by excess idle connections. On a tie, the pool with the higher p95 transaction time yields first: fast pools keep more reuse chances, while evicting an idle connection from a slower pool hurts less. The reserve pool absorbs short bursts, and `min_guaranteed_pool_size` protects critical workloads from eviction.

PgBouncer's `max_db_connections` sets a shared cap, but it does not redistribute already-open idle connections between pools. Odyssey has no direct equivalent.

[Read more →](concepts/pool-coordinator.md)
```

```admonish success title="Patroni-assisted Fallback"
When PgDoorman runs next to PostgreSQL and the local server disappears during a Patroni switchover, new backend connections temporarily go to another live cluster member. PgDoorman chooses the target through the Patroni REST API (`GET /cluster`): `sync_standby` first, then `replica`.

The local server enters cooldown, and fallback connections get a short lifetime. Once the local node is reachable again, the pool returns to it without a separate HAProxy or `consul-template` in front of the pooler. `patroni_api_urls` and `fallback_cooldown` are configured in `[general]`.

[Read more →](tutorials/patroni-assisted-fallback.md)
```

```admonish success title="Hot Process Handoff with Session Migration"
Before `SIGUSR2` or `UPGRADE`, operators can replace the binary and config on disk. PgDoorman validates that pair with `-t`, starts a child process with it, passes the listening socket to the child, and keeps the old process serving existing clients while eligible sessions migrate.

The new process receives `connection_id`, the cancel key, PostgreSQL session parameters, backend authentication state, and the client prepared-statement cache. From the application's point of view the connection stays open: no reconnect, no repeated `auth`/SCRAM, and no lost prepared statements. If the new backend connection has not prepared a statement yet, PgDoorman sends the needed `Parse` on the first `Bind`.

In foreground mode, non-TLS TCP sessions move through `SCM_RIGHTS`. TLS sessions migrate only on a Linux build with `tls-migration` and the same `tls_certificate`/`tls_private_key`; distro packages and the Docker image are built without it, so TLS clients drain. Clients inside a transaction stay on the old process and move after `COMMIT` or `ROLLBACK`. PgBouncer (`-R`, deprecated since 1.20, or rolling restart via `so_reuseport`) and Odyssey (`SIGUSR2` + `bindwith_reuseport`) leave old sessions in the old process until clients disconnect.

[Read more →](tutorials/binary-upgrade.md)
```

```admonish success title="Built-in Diagnostic Console"
PgDoorman serves the web console on the same address and port as `/metrics`. It is a local incident panel, not a replacement for long-term Prometheus/Grafana monitoring.

One screen shows pool saturation, p95/p99 query, transaction, and checkout latency, SQLSTATE errors, long-running queries, prepared-statement and query-interner state, the log tail, CPU by `tokio-worker`, and process memory (`jemalloc`, `/proc/self/status`, text/libs, stacks, swap).

The console can run Pause, Resume, Reconnect, and Reload for one pool or the whole instance. Other views are read-only. The UI is enabled only when `[web].ui = true` and `general.admin_password` is set to a non-placeholder value; otherwise PgDoorman keeps only `/metrics` and logs a `WARN`.

[Read more →](guides/web-ui.md)
```

## Why PgDoorman

- **Caches `Parse` on hot query paths.** Prepared backend state is reused between clients sharing a pool, including the anonymous `Parse` most drivers send for short parameterised queries. That cuts PostgreSQL planner CPU on repeated OLTP queries; `SHOW INTERNER` shows query-text memory, while Prometheus metrics show cache hits, misses, and evictions.
- **Multi-threaded, single shared pool.** All worker threads share one pool. PgBouncer is single-threaded; the recommended scale-out — several instances behind `so_reuseport` — gives each instance its own pool, and idle counts can drift between processes for the same database.
- **Thundering herd suppression.** When 200 clients race for 4 idle connections, PgDoorman caps concurrent backend creates (`scaling_max_parallel_creates`) and routes returning servers straight to the longest-waiting client through an in-process oneshot channel — no requeue through the idle pool.
- **Bounded tail latency.** Waiters are served strict FIFO so the worst-case wait can't be overtaken by latecomers. Pre-replacement of expiring backends — at 95% of `server_lifetime`, up to 3 in parallel — keeps the pool warm, so there is no checkout spike when a generation of connections rotates out.
- **Dead backend detection inside transactions.** If the backend dies mid-transaction (failover, OOM, network partition), PgDoorman returns SQLSTATE `08006` immediately by racing the client read against backend readability with a 100 ms tick. Without this, the client would block until TCP keepalive fires — on Linux defaults that is about two hours plus 9×75 s probes.
- **Built for operations.** YAML or TOML config with human-readable durations (`30s`, `5m`). `pg_doorman generate --host …` introspects an existing PostgreSQL and emits a starter config. `pg_doorman -t` validates the config without starting the server. A Prometheus `/metrics` endpoint is built-in.

## Comparison

| Feature                                                  |       PgDoorman       |              PgBouncer              |          Odyssey           |
| -------------------------------------------------------- | :-------------------: | :---------------------------------: | :------------------------: |
| Multi-threaded with shared pool                          |          Yes          |        No (single-threaded)         |  Workers, separate pools   |
| Prepared statements in transaction mode                  |          Yes          |           Yes (since 1.21)          | Yes (`pool_reserve_prepared_statement`) |
| Anonymous `Parse` cache for hot parameterised queries    | Yes, reused across clients in a pool | No, named statements only | No, named statements only |
| Pool Coordinator (per-database cap, priority eviction)   |          Yes          |                 No                  |             No             |
| Patroni-assisted fallback (built-in)                     |          Yes          |                 No                  |             No             |
| Pre-replacement on `server_lifetime` expiry              |          Yes          |                 No                  |             No             |
| Stale backend detection inside a transaction             | Yes (immediate `08006`) |     No (waits for TCP keepalive)    | No (waits for TCP keepalive) |
| Hot process handoff with idle-session migration          | Yes, via `SCM_RIGHTS`; TLS state with `tls-migration` and same cert/key | No (sessions stay on old process) | No (sessions stay on old process) |
| Backend TLS to PostgreSQL                                |   Yes (5 modes, hot reload via `SIGHUP`) | Yes (`server_tls_*`, hot reload via `RELOAD`) |             No             |
| Auth: SCRAM passthrough (no plaintext password)          | Yes (`ClientKey` extracted from proof) | Yes (encrypted SCRAM secret via `auth_query`/`userlist.txt`, since 1.14) |            Yes             |
| Auth: JWT (RSA-SHA256)                                   |          Yes          |                 No                  |             No             |
| Auth: PAM / `pg_hba.conf` / `auth_query`                 |          Yes          |                 Yes                 |            Yes             |
| Auth: LDAP                                               |          No           |           Yes (since 1.25)          |            Yes             |
| Config format                                            |     YAML / TOML       |                 INI                 |        Own format          |
| JSON structured logging                                  |          Yes          |                 No                  |   Yes (`log_format "json"`)   |
| Latency percentiles (p50/p90/p95/p99)                    | Yes (built-in `/metrics`) |        No (averages only)         |  Yes (via separate Go exporter) |
| Config test mode (`-t`)                                  |          Yes          |                 No                  |             No             |
| Auto-config from PostgreSQL (`generate --host`)          |          Yes          |                 No                  |             No             |
| Prometheus endpoint                                      | Built-in `/metrics`   |          External exporter          |  External exporter (Go sidecar) |

[Full feature matrix →](comparison.md)

## Benchmarks

AWS Fargate (16 vCPU), pool size 40, `pgbench` 30 s per test:

| Scenario                                | vs PgBouncer | vs Odyssey |
| --------------------------------------- | :----------: | :--------: |
| Extended protocol, 500 clients + SSL    |     ×3.5     |    +61%    |
| Prepared statements, 500 clients + SSL  |     ×4.0     |    +5%     |
| Simple protocol, 10 000 clients         |     ×2.8     |    +20%    |
| Extended + SSL + reconnect, 500 clients |     +96%     |    ~0%     |

[Full results →](benchmarks.md)

## Quick start

Install via your distro package manager:

```bash
# Ubuntu / Debian
sudo add-apt-repository ppa:vadv/pg-doorman
sudo apt update
sudo apt install pg-doorman

# Fedora / RHEL family
sudo dnf copr enable @pg-doorman/pg-doorman
sudo dnf install pg_doorman
```

Distro packages and the Docker image are built without the `tls-migration` and `pam` features. See [Installation](tutorials/installation.md) for the TLS feature matrix and how to build with them.

Or run via Docker:

```bash
docker run -p 6432:6432 \
  -v $(pwd)/pg_doorman.yaml:/etc/pg_doorman/pg_doorman.yaml \
  ghcr.io/ozontech/pg_doorman \
  pg_doorman /etc/pg_doorman/pg_doorman.yaml
```

Minimal config (`pg_doorman.yaml`):

```yaml
general:
  host: "0.0.0.0"
  port: 6432
  admin_username: "admin"
  admin_password: "change_me"

pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "app"
        password: "md5..."   # hash from pg_shadow / pg_authid
        pool_size: 40
```

`server_username` and `server_password` are omitted on purpose: PgDoorman re-uses the client's MD5 hash or SCRAM `ClientKey` to authenticate against PostgreSQL. No plaintext passwords in the config.

[Installation guide →](tutorials/installation.md) · [Configuration reference →](reference/general.md)

## Where to next

- New to PgDoorman? Start with [Overview](tutorials/overview.md), then [Installation](tutorials/installation.md) and [Basic usage](tutorials/basic-usage.md).
- Migrating from PgBouncer or Odyssey? Read [Comparison](comparison.md) and [Authentication](authentication/overview.md).
- Running Patroni? See [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md) and [`patroni_proxy`](tutorials/patroni-proxy.md).
- Production sizing? Read [Pool pressure](tutorials/pool-pressure.md) and [Pool Coordinator](concepts/pool-coordinator.md).
- Operating PgDoorman? See [Binary upgrade](tutorials/binary-upgrade.md), [Signals](operations/signals.md), [Troubleshooting](tutorials/troubleshooting.md).
