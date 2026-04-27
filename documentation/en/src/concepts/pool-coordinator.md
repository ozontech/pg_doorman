# Pool Coordinator

The Pool Coordinator caps total backend connections per database across all users in that pool, with priority eviction when the cap is reached. It is what PgBouncer's `max_db_connections` should have been: enforced fairly, with a reserve for short bursts, and per-user minimums to protect critical workloads.

This page explains the concept and when to use it. For tuning recipes and read-out from `SHOW POOL_COORDINATOR`, see [Pool Pressure](../tutorials/pool-pressure.md#coordinator-mode).

## What problem it solves

Without a coordinator, every user-pool is independent. A `pool_size` of 40 across 5 users means up to 200 backend connections — and PostgreSQL fights to maintain its own limits.

`max_db_connections` in PgBouncer caps the total, but once the cap is reached new clients simply queue. Connections only free up when their current owner closes them naturally on `server_idle_timeout`. Whoever grabbed connections first keeps them regardless of how heavily they use them, and slow workloads never yield to fast ones.

PgDoorman's Pool Coordinator caps the total **and**:

- **Evicts** idle connections from over-allocated users when another user needs to grow.
- **Ranks** users by p95 transaction time so the slowest pools yield first. Pools running fast transactions keep their reuse advantage; pools running long transactions sit idle a larger fraction of the time, so taking from them costs less.
- **Reserves** a small overflow for short bursts. Configured separately from the main cap.
- **Guarantees** a per-user minimum that is never evicted. Critical workloads keep their footing during contention.

## When to use it

Turn on the coordinator when:

- Multiple distinct workloads share the same database and you need an upper bound on backend connection count (PostgreSQL `max_connections`, RAM, file descriptors).
- One workload has bursty demand and you want it to absorb idle slots from others without crowding them out permanently.
- You operate near the PostgreSQL connection ceiling and need fair degradation rather than first-come-first-served.

You do **not** need it when:

- Each user's `pool_size` is small enough that the sum is comfortably below PostgreSQL's `max_connections`.
- Workloads are predictable and pre-sized.
- You want PgBouncer-level simplicity. `max_db_connections` without eviction is supported but discouraged for shared databases.

## Configuration

```yaml
pools:
  shared_db:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"

    # Total cap across all users in this pool.
    max_db_connections: 80

    # Reserve overflow above max_db_connections for short bursts.
    # Acquired only when no idle connection is available within reserve_pool_timeout.
    reserve_pool_size: 16
    reserve_pool_timeout: "3s"

    # Per-user safety net: connections never evicted from a user, even under pressure.
    # Sum across users should be ≤ max_db_connections.
    min_guaranteed_pool_size: 5

    # Eviction grace period: connections younger than this are not evicted.
    # Prevents thrashing when a workload briefly idles.
    min_connection_lifetime: "30s"

    users:
      - username: "fast_app"
        password: "md5..."
        pool_size: 40

      - username: "batch_job"
        password: "md5..."
        pool_size: 60
```

Effective ceiling: `max_db_connections + reserve_pool_size = 96`. The reserve absorbs sub-second spikes; if the spike persists, eviction kicks in.

## How it picks who donates

When a user requests a new backend and the cap is reached:

1. **Find candidates with idle connections.** A user holding only active connections cannot donate — its work is in flight.
2. **Skip protected users.** A user below `min_guaranteed_pool_size` is excluded.
3. **Skip recently-created connections.** Connections younger than `min_connection_lifetime` are not evicted (avoids churn during minor idle gaps).
4. **Rank by surplus.** Users with the most idle connections above their `min_guaranteed_pool_size` rank highest.
5. **Tiebreak by p95 transaction time.** Among equally-idle users, the pool with the higher p95 yields first. Higher p95 means each transaction holds the connection longer; the same user therefore reuses each connection less often, so a single eviction translates into fewer reused checkouts lost.

The chosen idle connection is closed; the requesting user receives a fresh connection from PostgreSQL.

## Observability

`SHOW POOL_COORDINATOR` shows current state per database:

```
database    | max_db_conn | current | reserve_size | reserve_used | evictions | reserve_acq | exhaustions
shared_db   | 80          | 78      | 16           | 2            | 142       | 18          | 0
```

- `evictions` rising fast — one user is starved repeatedly. Either raise `max_db_connections` or set `min_guaranteed_pool_size` for that user.
- `reserve_acq` high — bursts are normal but you might be undersized; consider raising `max_db_connections` instead of relying on reserve.
- `exhaustions` non-zero — even reserve was full. Clients hit `query_wait_timeout` waiting for a backend. Raise the cap.

Prometheus: `pg_doorman_pool_coordinator{type="..."}` (gauges) and `pg_doorman_pool_coordinator_total{type="evictions|reserve_acquisitions|exhaustions"}` (counters). See [Admin commands](../observability/admin-commands.md) and [Prometheus reference](../reference/prometheus.md).

## Caveats

- The coordinator only operates within one pool (one database). Cross-pool / cross-database limits are not supported.
- Eviction picks idle connections; a user holding all connections in long transactions cannot donate, so other users may starve. If this is your shape, raise `max_db_connections` or split the workload.
- `min_guaranteed_pool_size` is a floor for eviction, not a `min_pool_size` for warm-up. The pool still has to create those connections on demand.
- Setting `max_db_connections` without `min_guaranteed_pool_size` is the PgBouncer mode — works, but starves smaller users under pressure. Always set both for shared databases.

## Where to next

- Sizing recipe with worked examples: [Pool Pressure → Sizing the cap](../tutorials/pool-pressure.md#sizing-the-cap-against-postgresql).
- Tuning under load: [Pool Pressure → Tuning parameters](../tutorials/pool-pressure.md#tuning-parameters).
- Reading admin output: [Admin Commands → SHOW POOL_COORDINATOR](../observability/admin-commands.md).
- Pool modes (transaction vs session): [Pool Modes](pool-modes.md).
