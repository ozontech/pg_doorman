# Anonymous Prepared Statement Caching

PostgreSQL doesn't cache plans for anonymous prepared statements:
every `Bind` re-runs the planner from scratch. PgDoorman fills that
gap by transparently remapping every anonymous `Parse` to an internal
`DOORMAN_<N>` name on the backend, so the plan lands in the backend's
named prepared statement registry and gets reused. The reuse spans
`Bind`s of one client and `Bind`s of different clients sharing the
same pool.

The remap is transparent to the driver: clients send and receive
empty statement names just as they would against a vanilla
PostgreSQL.

This is a feature unique to PgDoorman. PgBouncer (1.21+) and Odyssey
support prepared statements in transaction mode, but only for
**named** statements; anonymous `Parse` is forwarded unchanged and
re-planned on every call.

## The PostgreSQL baseline

A `Parse` message carries a statement name. An empty name means
**anonymous**, anything else means **named**:

```text
                          Lifetime in PG          Plan caching
  ─────────────────────   ─────────────────       ─────────────────
  Anonymous (name="")     Until next anonymous    None: planner runs
                          Parse or session end    on every Bind
  Named (name="stmt_42")  Until Close /           Generic at first,
                          DEALLOCATE /            switches to custom
                          session end             after 5 observations
```

Most modern drivers default to **anonymous** for one-shot
parameterised queries: `lib/pq` (Go), `libpq` `PQexecParams` (C),
some flows in pgjdbc and psycopg. The application code looks
identical to a parameterised named-statement query, but the wire
protocol carries an empty name.

## Why this is a problem for transaction-mode pooling

Transaction pooling rotates a backend among many clients. If the
pooler forwards the empty `Parse` name as-is, every client's `Bind`
runs against a backend that has no plan cached for that query. Hot
OLTP paths pay the planner cost on every call.

Named prepared statements solve plan caching, but they push the
bookkeeping problem onto the pooler:

- The pooler must remember each client's named statements until the
  client disconnects, even if the pool-level shared cache evicts the
  entry.
- On every `Bind`, it must verify the current backend knows the
  name and re-`Parse` otherwise.
- On client disconnect, it must issue `Close` or `DEALLOCATE` to the
  right backend.
- Drivers that mint per-query names (`stmt_<seq>`) compound the
  per-client cache size: hundreds of entries per client times tens of
  thousands of clients.

So the choice is: give up plan caching for anonymous traffic, or
inherit the full cost of named-statement bookkeeping. PgDoorman
takes a third option.

## What PgDoorman does

On every anonymous `Parse` from the client, PgDoorman:

1. Hashes the query text plus parameter type OIDs.
2. Looks up the hash in the **pool-level** cache (shared across all
   clients of this pool). On miss, it allocates a fresh
   `DOORMAN_<counter>` name and registers an `Arc<Parse>` entry.
3. Stores a per-client cache entry keyed by `Anonymous(hash)` so the
   following `Bind` can locate the same `DOORMAN_<N>`.
4. Forwards `Parse` to the backend with the rewritten name.
5. On the matching `Bind` (with empty name), rewrites the statement
   name to `DOORMAN_<N>` and ensures the current backend already
   holds the named statement; sends a fresh `Parse` if not.

The client never sees `DOORMAN_<N>`. PgDoorman strips the rewrite
from all responses and synthesises `ParseComplete` when it skips a
backend round-trip.

### Wire-protocol example

A Go application running

```go
db.Query("SELECT * FROM t WHERE name = $1", "vasya")
```

through `lib/pq` produces this exchange:

```text
  Client                   PgDoorman                  Backend
  ──────                   ─────────                  ───────
  Parse("", q)        ────►│ hash, miss → DOORMAN_42
                            │ pool_cache[hash] = Arc<Parse>
                            │ client_cache[Anon(hash)] = ...
                            │             Parse("DOORMAN_42") ────►
                            │                    ◄── ParseComplete
                       ◄────│ ParseComplete
  Bind("", "vasya")   ────►│ rewrite "" → "DOORMAN_42"
                            │             Bind("DOORMAN_42") ─────►
                            │             Execute, Sync ──────────►
                            │                ◄── BindComplete, ...
                            │                ◄── ReadyForQuery
                       ◄────│ BindComplete, ...
```

A second client running the same query in the same pool hits the
pool cache and skips the backend `Parse` entirely:

```text
  Client B           PgDoorman                       Backend (same)
  ────────           ─────────                       ──────────────
  Parse("", q)  ───►│ hash hit → DOORMAN_42
                     │ server_cache contains "DOORMAN_42"
                ◄────│ synthetic ParseComplete       (no message sent)
  Bind("", v)   ───►│ rewrite "" → "DOORMAN_42"
                     │           Bind("DOORMAN_42") ────►
                     │           ...
```

## Cache layers

PgDoorman keeps prepared-statement state at three levels:

```text
  Pool-level    DashMap<hash, CacheEntry>
                One per pool. Holds Arc<Parse> with name "DOORMAN_N".
                Size:    prepared_statements_cache_size (default 8192).
                Eviction: approximate LRU.

  Client-level  AHashMap or LruCache, per client.
                Maps Named(client_name) | Anonymous(hash) → CachedStatement.
                Size:    client_prepared_statements_cache_size
                         (default 0 = unlimited).

  Server-level  LruCache<String, ()>, per backend connection.
                Tracks which DOORMAN_N this backend already holds.
                True LRU; on eviction issues Close to the backend.
```

The query text itself is interned via `Arc<str>`: ten clients sending
the same anonymous query share one allocation in memory.

## When the remap helps

- **API workloads with a small set of hot queries.** A handful of
  unique `SELECT` / `INSERT` shapes shared across thousands of
  clients. Pool-cache hit rate near 100 %, planner runs once per
  backend per query, scales linearly with concurrency.
- **Drivers that pin to anonymous prepared.** `lib/pq`, `libpq`
  `PQexecParams`, JDBC's `serverPreparedStatementType=NONE`. Without
  the remap they would re-plan on every call.
- **Mixed pools where named and anonymous coexist.** Anonymous
  statements get the same plan-cache benefit as named ones, without
  growing the per-client client cache.

## When the remap doesn't help

- **Ad-hoc / OLAP traffic.** Each query is unique, so the pool cache
  evicts continuously, scanning O(N) per insert. Disable with
  `prepared_statements_cache_size = 0`.
- **Single-statement scripts.** A connect → `Parse` → 1 `Bind` →
  disconnect pattern doesn't accumulate enough hits to repay the
  bookkeeping. The overhead per `Parse` is small (~700 ns) but
  measurable.
- **Async drivers in pipeline mode.** Each session gets a unique
  `DOORMAN_async_<N>` name to avoid in-flight collisions, so
  cross-session reuse on the server cache doesn't happen. Pool-level
  text sharing still works; the backend planner still runs once per
  session.

Track effectiveness with the Prometheus counters
`pg_doorman_servers_prepared_hits` and
`pg_doorman_servers_prepared_misses`. A sustained miss rate above
30 % means the remap is paying CPU and memory without earning the
plan-cache reuse. Either disable it or tune
`prepared_statements_cache_size` upward.

## How other poolers handle this

| Pooler          | Plan cache for anonymous Parse                    |
| --------------- | :------------------------------------------------ |
| **PgDoorman**   | Yes: transparent remap to `DOORMAN_<N>`           |
| PgBouncer 1.21+ | No: named only, anonymous forwarded unchanged     |
| Odyssey         | No: named only, `pool_reserve_prepared_statement` |
| PgCat           | No: named only                                    |

PgBouncer added prepared-statement support in 1.21, but limited it
to **named** statements: an anonymous `Parse` is forwarded as-is and
each `Bind` re-runs the planner. Odyssey's
`pool_reserve_prepared_statement` requires named statements; it does
nothing for anonymous traffic. PgCat behaves the same way.

This makes anonymous-prepared caching a feature only PgDoorman
provides.

## Configuration

| Setting                                  | Default | Effect                                                |
| ---------------------------------------- | :-----: | ----------------------------------------------------- |
| `prepared_statements_cache_size`         | 8192    | Pool-level cache size in entries. 0 disables remap.   |
| `client_prepared_statements_cache_size`  | 0       | Per-client cache size. 0 = unlimited (LRU disabled).  |

To disable anonymous remap entirely (rare, for OLAP-only deployments):

```yaml
general:
  prepared_statements_cache_size: 0
```

## Differences from PostgreSQL semantics

The remap changes a few protocol-level behaviours that strict
applications may rely on:

- The same anonymous `Parse` issued twice does **not** discard the
  previous one. Each `(query, param_types)` lives independently in
  the pool cache under a separate `DOORMAN_<N>`.
- `Close` with an empty name is a no-op for PgDoorman's caches. The
  underlying `DOORMAN_<N>` lives until pool-level LRU evicts it or
  the pool shuts down.
- The plan stays generic longer. PostgreSQL switches a named
  statement from generic to custom plans after five observations; if
  different clients share the same `DOORMAN_<N>` and each contributes
  one or two `Bind`s, the threshold is reached faster — but the
  resulting shared plan may be a poor fit for a client with skewed
  data.

Applications that depend on PostgreSQL's "anonymous Parse discards
the previous one" semantics should switch to named statements with
explicit `Close`.

## Observability

Admin commands:

- `SHOW PREPARED_STATEMENTS` — pool, hash, name, query text,
  `count_used`. Top rows by `count_used` show the hot queries that
  benefit most from the cache.
- `SHOW POOLS_MEMORY` — `pool_prepared_count`,
  `client_prepared_count`, `pool_prepared_bytes`,
  `client_prepared_bytes`.

Prometheus metrics (full list in [Prometheus](../reference/prometheus.md)):

- `pg_doorman_pool_prepared_cache_entries{user, database}`
- `pg_doorman_pool_prepared_cache_bytes`
- `pg_doorman_clients_prepared_cache_entries`
- `pg_doorman_clients_prepared_cache_bytes`
- `pg_doorman_servers_prepared_hits{user, database, backend_pid}`
- `pg_doorman_servers_prepared_misses`
- `pg_doorman_async_clients_count`

## Reference

- [Pool Modes](pool-modes.md) — transaction mode, where prepared-statement
  remapping is enabled.
- [General Settings](../reference/general.md) — `prepared_statements_cache_size`,
  `client_prepared_statements_cache_size`.
- [Admin Commands](../observability/admin-commands.md) — `SHOW PREPARED_STATEMENTS`,
  `SHOW POOLS_MEMORY`.
- [Prometheus](../reference/prometheus.md) — full metric list.
