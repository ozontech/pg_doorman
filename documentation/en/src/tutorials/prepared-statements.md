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

The client never sees `DOORMAN_<N>`: the rewrite lives only on the
leg between PgDoorman and the backend. When the backend already
holds the name, PgDoorman synthesises `ParseComplete` itself and
skips the round-trip.

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

  Client-level  Named:     AHashMap<String, CachedStatement>, unbounded.
                Anonymous: LruCache<u64, CachedStatement> bounded by
                           client_anonymous_prepared_cache_size (default 256),
                           or AHashMap if size = 0.
                Eviction of an Anonymous entry is local: the Arc<Parse>
                is dropped, the underlying DOORMAN_<N> on the backend
                stays.

  Server-level  LruCache<String, ()>, per backend connection.
                Tracks which DOORMAN_N this backend already holds.
                True LRU; on eviction issues Close to the backend.
```

When the Anonymous LRU evicts an entry, PgDoorman drops the local
reference and does not send `Close` to the backend. The underlying
`DOORMAN_<N>` is recycled by the server-level LRU or `server_lifetime`
(default 20 min), whichever comes first.

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

- **Ad-hoc / OLAP traffic.** Each query is unique, so every insert
  triggers an eviction with an O(N) scan. Disable with
  `prepared_statements_cache_size = 0`.
- **Single-statement scripts.** A connect → `Parse` → 1 `Bind` →
  disconnect pattern doesn't accumulate enough hits to repay the
  bookkeeping. The overhead per `Parse` is small (~700 ns) but
  measurable.
- **Async drivers in pipeline mode.** Each session gets a unique
  `DOORMAN_async_<N>` name to avoid name collisions between
  in-flight operations, so the server cache can't reuse entries
  across sessions. The pool-level cache still shares the query text
  across sessions; the backend planner still runs once per session.

Track effectiveness with the Prometheus counters
`pg_doorman_servers_prepared_hits` and
`pg_doorman_servers_prepared_misses`. A sustained miss rate above
30 % means the remap is spending CPU and memory without delivering
plan-cache reuse. Either disable it or raise
`prepared_statements_cache_size`.

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

| Setting                                  | Default | Effect                                                            |
| ---------------------------------------- | :-----: | ----------------------------------------------------------------- |
| `prepared_statements_cache_size`         | 8192    | Pool-level cache size in entries. 0 disables remap.               |
| `client_anonymous_prepared_cache_size`   | 256     | Per-client Anonymous LRU size. 0 = unlimited. Named is unbounded. |

The Named part of the per-client cache is always unlimited and is not
affected by `client_anonymous_prepared_cache_size`.

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

## Tuning

### Sizing the cache

PgDoorman's prepared-statement cache has three layers, governed by
two related config knobs:

- `prepared_statements_cache_size` (default `8192`) sizes the
  pool-level shared cache — one map per pool, keyed by query hash.
  This is the upper bound on distinct query shapes the pool will
  remember across all clients. Approximate LRU; eviction is O(N) over
  the whole map and never sends Close to a backend (other clients may
  still hold the Arc).
- `server_prepared_statements_cache_size` (default: inherits from
  `prepared_statements_cache_size`) sizes the per-backend cache —
  one LRU per backend connection, keyed by `DOORMAN_<N>` name. This
  is the upper bound on distinct prepared statements PgDoorman will
  let a single PostgreSQL backend hold. True LRU (O(1)); eviction
  queues a `Close` message for the backend, sent on the next Sync or
  Flush — your `pg_prepared_statements` view may temporarily show
  more rows than the cap until the next Sync arrives.

Both knobs accept a per-pool override:

```yaml
general:
  prepared_statements_cache_size: 8192
  server_prepared_statements_cache_size: 1024  # tighter per-backend

pools:
  oltp:
    # inherits both from general
    pool_mode: "transaction"
  reporting:
    # this pool has wider query diversity; let server cache hold more
    server_prepared_statements_cache_size: 4096
    pool_mode: "transaction"
```

Setting `prepared_statements_cache_size: 0` disables the entire
remap and forces server-level LRU to 0 too. Setting
`server_prepared_statements_cache_size: 0` while leaving the pool
size positive is allowed but rarely useful — the per-backend cache
becomes a pass-through that re-Parses on every cross-backend hit.

When to lower `server_prepared_statements_cache_size` below the pool
size:

- Backends carry too many `DOORMAN_<N>` rows (`pg_prepared_statements`
  near the cap, plan memory ballooning).
- You want faster `Close` recycling without shrinking pool-cache hit
  rate.

When to keep them equal (the default):

- You don't have a measured backend-memory problem. Leave the inheritance.

### Default `client_anonymous_prepared_cache_size = 256`

The default of 256 entries per client is chosen for the typical OLTP
workload: a small set of hot anonymous queries shared across thousands
of clients. Each entry holds a lightweight `(hash, async_name?, Arc<Parse>)`
record — the `Arc<Parse>` is shared with the pool-level cache, so the
per-client overhead is roughly `~80 bytes` of bookkeeping per entry.
At 10 000 connected clients × 256 entries × ~80 bytes that adds up to
about 200 MB of headroom on the pooler — predictable and bounded.

Raise the cap when:

- An ORM or generated SQL framework mints `stmt_<seq>` per query and
  the `Anonymous` LRU keeps recycling entries (visible as a sustained
  non-zero rate on `pg_doorman_clients_prepared_anonymous_evictions_total`).
- The application has a known wide working set per session (more than
  256 distinct anonymous queries) and the eviction rate matches that
  pressure.

Lower the cap or raise `max_memory_usage` for very large connection
counts (50 000+ clients): at that scale even 256 × clients × 80 bytes
crosses 1 GB of pooler bookkeeping, and trimming the cap halves it.

### Named is unbounded by design

The Named part of the per-client cache has no upper bound. PgDoorman
holds the `Arc<Parse>` for every named statement the client created
until the client disconnects or sends `DEALLOCATE` / `DEALLOCATE ALL`.
This matches PostgreSQL's own contract — named statements live for the
session — and avoids the failure mode where evicting a Named entry
under pressure causes the next `Bind` to fail with
`prepared statement does not exist`.

The flip side: drivers that mint per-query named statements (some
pgjdbc and Hibernate flows, some .NET Npgsql configurations) can grow
the per-client Named map without limit. PgDoorman cannot bound this
safely; the application is responsible for either reusing names or
sending `DEALLOCATE` on names it no longer uses.

The Anonymous LRU eviction counter
(`pg_doorman_clients_prepared_anonymous_evictions_total`) is the only
side that has a built-in pressure signal. The Named side has none —
watch the `client_named_count` column in `SHOW POOLS_MEMORY` and
`pg_doorman_clients_prepared_named_entries` for unexpected growth.

### Backend memory creep window

When the Anonymous LRU evicts an entry on the client side, PgDoorman
drops only the local `Arc<Parse>`. The corresponding `DOORMAN_<N>`
prepared statement stays alive on every PostgreSQL backend that ever
served it. Two mechanisms eventually clean it up:

- **Server-level LRU.** Each backend tracks its own
  `LruCache<String, ()>` of `DOORMAN_<N>` names, capped at
  `prepared_statements_cache_size` (default 8192). When the cap is
  reached, the backend issues `Close` on the least recently used
  name, releasing the plan.
- **Backend rotation.** A backend reaches `server_lifetime`
  (default 20 min) and pg_doorman closes it; the new backend starts
  with an empty plan cache.

The worst-case memory footprint per backend is therefore
`prepared_statements_cache_size × ~100 KB` of plan memory ≈ 800 MB
on the PostgreSQL side. To shrink the window:

- Lower `prepared_statements_cache_size` so the server-level LRU
  recycles plans sooner.
- Lower `server_lifetime` so backends rotate faster.

The PostgreSQL system view `pg_prepared_statements` reports the names
held by the current backend. Counting rows there per backend tells
you how close the backend is to the cap.

## Observability

Admin commands:

- `SHOW PREPARED_STATEMENTS` — pool, hash, name, query text,
  `count_used`, `kind`. Top rows by `count_used` show the hot queries
  that benefit most from the cache. The `kind` column is the last
  column and reports `named`, `anonymous`, or `mixed` depending on
  how clients have used the entry over its lifetime.

  Example output:

  ```text
   pool         | hash               | name        | query             | count_used | kind
  --------------+--------------------+-------------+-------------------+------------+-----------
   sharded.user | 1234567890123456   | DOORMAN_1   | SELECT * FROM t1  |     150234 | anonymous
   sharded.user | 2345678901234567   | DOORMAN_2   | INSERT INTO t2 .. |      87654 | named
   sharded.user | 3456789012345678   | DOORMAN_3   | SELECT * FROM t3  |      45678 | mixed
  ```

- `SHOW POOLS_MEMORY` — `pool_prepared_count`,
  `client_prepared_count`, `pool_prepared_bytes`,
  `client_prepared_bytes`, plus the breakdown by kind:
  `client_named_count`, `client_anonymous_count`,
  `client_anonymous_evictions_total`. The `_total` suffix marks the
  last column as a counter (cumulative since pool start), distinct
  from the gauge columns to its left.

Prometheus metrics (full list in [Prometheus](../reference/prometheus.md)):

- `pg_doorman_pool_prepared_cache_entries{user, database}`
- `pg_doorman_pool_prepared_cache_bytes`
- `pg_doorman_clients_prepared_cache_entries`
- `pg_doorman_clients_prepared_cache_bytes`
- `pg_doorman_clients_prepared_named_entries{user, database}`
- `pg_doorman_clients_prepared_anonymous_entries{user, database}`
- `pg_doorman_clients_prepared_anonymous_evictions_total{user, database}`
- `pg_doorman_servers_prepared_hits{user, database, backend_pid}`
- `pg_doorman_servers_prepared_misses`
- `pg_doorman_async_clients_count`

## Alerting

### Anonymous LRU eviction rate

A sustained non-zero rate on the Anonymous eviction counter means the
LRU is recycling entries faster than the application reuses them.
Alert template:

```text
rate(pg_doorman_clients_prepared_anonymous_evictions_total[5m]) > 10
  for 10m
```

The threshold of 10 evictions/second per pool is a starting point —
the right value depends on traffic shape and connection count. Treat
the alert as "the cap is too tight or the application's working set
is wider than expected", then either raise `client_anonymous_prepared_cache_size`
or investigate whether the application is generating unique queries
on the hot path.

### `kind = mixed` interpretation

Each pool-level cache entry remembers whether clients have used it
under a Named statement name, an Anonymous one, or both. `kind = mixed`
means the same `(query, param_types)` pair has been parsed by at
least one client as named and at least one client as anonymous in its
current lifetime. Most workloads do not see `mixed` rows; a pool
dominated by `mixed` entries indicates a heterogeneous client base
(different drivers or driver configurations against the same database)
worth verifying — sometimes intentional, sometimes a sign that one of
the clients is configured wrong.

### Backend prepared statement count

PostgreSQL exposes `pg_prepared_statements` per backend. If pooler
memory is fine but PostgreSQL backend RSS keeps growing, count rows
per backend:

```sql
SELECT count(*) FROM pg_prepared_statements;
```

Numbers near `prepared_statements_cache_size` (default 8192) per
backend mean the server-level LRU is at its cap and rotation is the
only way to release plan memory. If `server_lifetime` is long, plans
accumulate for a long time. Lowering either knob releases the
plan-memory pressure, at the cost of more frequent re-parses on the
backend.

## Reference

- [Pool Modes](../concepts/pool-modes.md) — transaction mode, where
  prepared-statement remapping is enabled.
- [General Settings](../reference/general.md) — `prepared_statements_cache_size`,
  `client_anonymous_prepared_cache_size`.
- [Admin Commands](../observability/admin-commands.md) — `SHOW PREPARED_STATEMENTS`,
  `SHOW POOLS_MEMORY`.
- [Prometheus](../reference/prometheus.md) — full metric list.
