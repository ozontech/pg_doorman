# Admin Commands

PgDoorman exposes a Postgres-compatible admin database. Connect to the same port as your data clients, but with `dbname=pgdoorman` and the admin credentials from your config:

```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman
```

Or via `psql` connection string:

```bash
psql "host=127.0.0.1 port=6432 user=admin dbname=pgdoorman"
```

Admin commands are read with `SHOW <subcommand>` or executed with bare verbs (`PAUSE`, `RESUME`, `RECONNECT`, `RELOAD`, `SHUTDOWN`, `SET <param> = <value>`).

## SHOW commands

| Command | Purpose |
| --- | --- |
| `SHOW HELP` | List available commands. |
| `SHOW CONFIG` | Current effective configuration. Read-only. |
| `SHOW DATABASES` | One row per pool: host, port, database, pool size, mode. |
| `SHOW POOLS` | Pool utilization snapshot per user×database: idle/active/waiting clients, idle/active servers. |
| `SHOW POOLS_EXTENDED` | `SHOW POOLS` plus bytes received/sent and average wait time. |
| `SHOW POOLS_MEMORY` | Per-pool memory accounting for prepared statement cache (client-side and server-side). |
| `SHOW POOL_COORDINATOR` | Pool Coordinator state per database: current connections, reserve usage, eviction count. See [Pool Coordinator](../concepts/pool-coordinator.md). |
| `SHOW POOL_SCALING` | Anticipation/burst metrics: in-flight creates, gate waits, anticipation notifies/timeouts. |
| `SHOW PREPARED_STATEMENTS` | Cached prepared statements per pool: hash, name, query text, hit count. |
| `SHOW CLIENTS` | Active clients: ID, database, user, app name, address, TLS state, transaction/query/error counts, age. |
| `SHOW SERVERS` | Active backend connections: server ID, backend PID, database, user, TLS, state, transaction/query counts, prepare cache hits/misses, bytes. |
| `SHOW CONNECTIONS` | Connection counts by type: total, errors, TLS, plain, cancel. |
| `SHOW STATS` | Aggregated stats per user×database: total transactions, queries, time, bytes, averages. |
| `SHOW LISTS` | Counts by category (databases, users, pools, clients, servers). |
| `SHOW USERS` | List of users and their pool modes. |
| `SHOW AUTH_QUERY` | `auth_query` cache hit/miss/refetch rates, auth success/failure, executor errors, dynamic pool counts. |
| `SHOW SOCKETS` | TCP and Unix socket counts by state (Linux only — reads `/proc/net/`). |
| `SHOW LOG_LEVEL` | Current log level. |
| `SHOW VERSION` | PgDoorman version. |

`SHOW POOL_COORDINATOR` and `SHOW POOL_SCALING` have no equivalent in PgBouncer or Odyssey — they expose PgDoorman-specific machinery.

## Control commands

| Command | Effect |
| --- | --- |
| `PAUSE` | Stop accepting new client requests. Existing clients finish their transactions. |
| `PAUSE <database>` | Pause a single pool. |
| `RESUME` / `RESUME <database>` | Resume after `PAUSE`. |
| `RECONNECT` / `RECONNECT <database>` | Force-recycle backend connections (close idle, drain active). New connections come from PostgreSQL. |
| `RELOAD` | Same as `SIGHUP` — reload config from disk. |
| `SHUTDOWN` | Same as `SIGTERM` — graceful shutdown. |
| `KILL <database>` | Drop all clients connected to a specific pool. |
| `SET log_level = '<level>'` | Change runtime log level (`error`, `warn`, `info`, `debug`, `trace`). |

`PAUSE`/`RESUME` are useful during failovers or maintenance windows. `RECONNECT` after rotating credentials in `pg_authid` ensures backends use the new password.

## Reading common output

### `SHOW POOLS`

```
database | user | cl_idle | cl_active | cl_waiting | sv_active | sv_idle | sv_used | maxwait
mydb     | app  | 12      | 4         | 0          | 4         | 36      | 0       | 0.0
```

- `cl_waiting > 0` means clients are stuck waiting for a backend. Either raise `pool_size` or check for slow queries.
- `sv_idle` matches free backends; `sv_active` is in-use; `sv_used` is reserved by the coordinator (see below).
- `maxwait` is the longest current wait in seconds. If it grows beyond `query_wait_timeout`, clients get errors.

### `SHOW POOL_COORDINATOR`

```
database | max_db_conn | current | reserve_size | reserve_used | evictions | reserve_acq | exhaustions
mydb     | 80          | 78      | 16           | 2            | 142       | 18          | 0
```

- `evictions` rising rapidly: a user is starved repeatedly. Set or raise `min_guaranteed_pool_size` for that user.
- `reserve_acq` high: bursts are normal but you might be undersized. Consider raising `max_db_connections` instead of relying on the reserve.
- `exhaustions` non-zero: even reserve was full. Clients hit `query_wait_timeout`. Raise the cap.

See [Pool Coordinator](../concepts/pool-coordinator.md) for tuning.

### `SHOW POOL_SCALING`

```
user | database | inflight | creates | gate_waits | burst_gate_budget_ex | antic_notify | antic_timeout | create_fallback | replenish_def
app  | mydb     | 1        | 12345   | 87         | 3                    | 142          | 8             | 22              | 0
```

- `inflight` is current backend creations in progress.
- `gate_waits` rising: `scaling_max_parallel_creates` is throttling you. Acceptable if PostgreSQL is under load; raise it if PG can handle more parallel `connect()` calls.
- `antic_notify` vs `antic_timeout` ratio: high timeout count means anticipation is not finding a returning connection in time. Raise `scaling_warm_pool_ratio` so the pool grows ahead of demand.
- `create_fallback` rising means pre-replacement is firing — connections expired before naturally being returned.

See [Pool Pressure → Tuning](../tutorials/pool-pressure.md#tuning-parameters).

## Authentication

The admin database uses the credentials from `general.admin_username` and `general.admin_password`:

```yaml
general:
  admin_username: "admin"
  admin_password: "change_me"
```

Admin connections do not pass through `pg_hba.conf` rules — they go directly to the admin handler. Restrict admin access at the network layer (`listen_addresses`, firewall) or use Unix sockets.

## Where to next

- [Prometheus reference](../reference/prometheus.md) — same data, machine-readable.
- [Pool Coordinator](../concepts/pool-coordinator.md) — what `SHOW POOL_COORDINATOR` is telling you.
- [Pool Pressure](../tutorials/pool-pressure.md) — what `SHOW POOL_SCALING` is telling you.
- [Troubleshooting](../tutorials/troubleshooting.md) — common failure modes and their `SHOW` output.
