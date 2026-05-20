# Anonymous Parse caching

PgDoorman caches anonymous `Parse` messages for transaction pooling.
Many drivers send short parameterised queries as `Parse` with an empty
statement name. Without a remap, PostgreSQL plans the query again on
every `Bind`, so hot OLTP paths pay planner CPU on every call.

PgDoorman transparently remaps every anonymous `Parse` to an internal
`DOORMAN_<N>` name on the backend. The plan lands in the backend's
named prepared statement registry and gets reused across `Bind`s of
one client and across clients sharing the same pool. The main effect
is less planner CPU and fewer repeated backend Parses for the same
query shape.

The remap is transparent to the driver: clients send and receive
empty statement names just as they would against a vanilla
PostgreSQL.

PgBouncer (1.21+) and Odyssey support prepared statements in
transaction mode, but only for **named** statements. They forward
anonymous `Parse` unchanged. PgDoorman's extra behaviour is the
anonymous remap. Cache bounds, LRU, TTL, and observability keep that
remap controlled under dynamic SQL.

## What gets faster

Anonymous `Parse` caching removes repeated work from hot
parameterised query paths:

- the backend does not receive the same `Parse` on every reuse of an
  already known query shape;
- PostgreSQL can use a prepared statement already created on that
  backend connection;
- different clients in the same pool share one pool-level entry
  instead of warming the same query independently;
- on a server-cache hit, PgDoorman synthesizes `ParseComplete`
  without a PostgreSQL round-trip.

This is primarily a performance optimization for OLTP workloads with
repeated query shapes. Cache caps, TTL, and the interner exist so the
speedup does not become unbounded memory growth in the pooler or on
PostgreSQL backends.

## The PostgreSQL baseline

A `Parse` message carries a statement name. An empty name means
**anonymous**, anything else means **named**:

```text
                          Lifetime in PG          Plan caching
  ─────────────────────   ─────────────────       ─────────────────────
  Anonymous (name="")     Until next anonymous    No named registry
                          Parse or session end    entry; each new
                                                   unnamed Parse plans
  Named (name="stmt_42")  Until Close /           Starts with custom
                          DEALLOCATE /            plans; may switch to
                          session end             a generic plan after
                                                   5 custom executions
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

Named prepared statements solve planner performance, but they push
the bookkeeping problem onto the pooler:

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

1. Hashes the query text plus parameter type OIDs plus a digest of the
   client's startup-pinned planner GUCs (`search_path`,
   `default_transaction_isolation`, `default_transaction_read_only`,
   `default_text_search_config`, `role`). Two clients that send the
   same `query` + parameter OIDs but pinned different `search_path`
   values therefore get separate cache entries and separate plans.
   Other planner-relevant GUCs (`TimeZone`, `DateStyle`,
   `plan_cache_mode`, `enable_*`, JIT cost knobs) are **not** part of
   the digest — see the `sync_server_parameters` reference for the
   scope and how to handle workloads that need broader coverage.
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
                           client_anonymous_prepared_cache_size (defaults to
                           prepared_statements_cache_size when unset),
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
  backend per query shape, and later calls go through `Bind` against
  already prepared backend state.
- **Drivers that pin to anonymous prepared.** `lib/pq`, `libpq`
  `PQexecParams`, pgjdbc before the `prepareThreshold` is reached.
  Without the remap they would re-plan on every call.
- **Mixed pools where named and anonymous coexist.** Anonymous
  statements get the same plan-cache benefit as named ones, without
  growing the per-client named cache.

## When the remap doesn't help

- **Ad-hoc / OLAP traffic.** Each query is unique. When the pool cache
  is full, each new query shape must find an old entry to evict with an
  O(N) scan. Disable prepared-statement remapping with
  `prepared_statements: false` if the instance only serves this traffic.
- **Single-statement scripts.** A connect → `Parse` → 1 `Bind` →
  disconnect pattern doesn't accumulate enough hits to repay the
  bookkeeping. The overhead per `Parse` is small (~700 ns) but
  measurable.
- **Async drivers in pipeline mode.** Each session gets a unique
  `DOORMAN_async_<N>` name to avoid name collisions between
  in-flight operations, so the server cache can't reuse entries
  across sessions. The pool-level cache still shares the query text
  across sessions; the backend planner still runs once per session.

Track effectiveness with
`rate(pg_doorman_servers_prepared_hits_total[5m])` and
`rate(pg_doorman_servers_prepared_misses_total[5m])`. A sustained miss
share above 30 % means the remap is spending CPU and memory without
enough backend plan reuse. Either disable it or raise
`prepared_statements_cache_size`.

## How other poolers handle this

| Pooler          | Parse/plan cache for anonymous prepared statements |
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

In this comparison, only PgDoorman caches anonymous `Parse`.

## Configuration

| Setting                                  | Default | Effect                                                                                                        |
| ---------------------------------------- | :-----: | ------------------------------------------------------------------------------------------------------------- |
| `prepared_statements`                    | `true`  | Enables prepared-statement remapping and caching. Set `false` to disable the feature.                         |
| `prepared_statements_cache_size`         | 8192    | Pool-level cache size in entries. Must be greater than 0 while `prepared_statements` is `true`.                |
| `server_prepared_statements_cache_size`  | inherits `prepared_statements_cache_size` | Per-backend LRU size for `DOORMAN_<N>` names. `0` disables backend retention but not the pool-level remap. |
| `client_anonymous_prepared_cache_size`   | inherits `prepared_statements_cache_size` | Per-client Anonymous LRU size. `0` means unlimited. Named is unbounded. |

The Named part of the per-client cache is always unlimited and is not
affected by `client_anonymous_prepared_cache_size`.

To disable prepared-statement remapping entirely (rare, for OLAP-only deployments):

```yaml
general:
  prepared_statements: false
```

There is no separate anonymous-only switch. Do not use
`prepared_statements_cache_size: 0` as the disable switch: pg_doorman
rejects that general config while `prepared_statements` is enabled.

## Differences from PostgreSQL semantics

The remap changes a few protocol-level behaviours that strict
applications may rely on:

- The same anonymous `Parse` issued twice does **not** discard the
  previous one. Each `(query, param_types)` lives independently in
  the pool cache under a separate `DOORMAN_<N>`.
- `Close` with an empty name is a no-op for PgDoorman's caches. The
  underlying `DOORMAN_<N>` lives until pool-level LRU evicts it or
  the pool shuts down.
- PostgreSQL's custom/generic plan decision is shared by all clients
  using the same `DOORMAN_<N>`. A named statement starts with custom
  plans; after five custom executions PostgreSQL may switch to a
  generic plan if its estimated cost is close enough. With PgDoorman,
  those executions can come from different clients, so a generic-plan
  decision can reflect mixed parameter distributions.

Applications that depend on PostgreSQL's "anonymous Parse discards
the previous one" semantics should switch to named statements with
explicit `Close`.

## Tuning

### Sizing the cache

PgDoorman's prepared-statement cache has three layers, governed by
three related config knobs:

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
- `client_anonymous_prepared_cache_size` (default: inherits from
  `prepared_statements_cache_size`) sizes the per-client Anonymous
  LRU. Set to `0` to disable the LRU and use an unlimited map; set
  to a number to bound the per-client cache independently of the
  pool size.

The pool-level and server-level knobs accept per-pool overrides:

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

Setting `prepared_statements: false` disables the entire remap and
forces the pool-level and server-level caches to 0. Setting
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

### Sizing `client_anonymous_prepared_cache_size`

When unset, the per-client Anonymous LRU inherits the resolved
`prepared_statements_cache_size` for the pool (default `8192`). Set
an explicit value to override that inheritance — `0` disables the
LRU and uses an unlimited map, any positive number caps the LRU at
that size.

Each entry holds a lightweight `(hash, async_name?, Arc<Parse>)`
record — the `Arc<Parse>` is shared with the pool-level cache, so the
per-client overhead is roughly `~80 bytes` of bookkeeping per entry.
At 10 000 connected clients × 256 entries × ~80 bytes that adds up to
about 200 MB of headroom on the pooler — predictable and bounded.

Raise the cap when:

- An ORM or generated SQL framework mints `stmt_<seq>` per query and
  the `Anonymous` LRU keeps recycling entries (visible as a sustained
  non-zero rate on `pg_doorman_clients_prepared_anonymous_evictions_total`).
- The application has a known wide working set per session and the
  eviction rate matches that pressure.

Lower the cap for very large connection counts (50 000+ clients): at
that scale `clients × cache_size × 80 bytes` of pooler bookkeeping can
cross 1 GB, and trimming the cap halves it. `max_memory_usage` does not
cap prepared-statement bookkeeping; it protects in-flight query buffers.

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
  `server_prepared_statements_cache_size` (or
  `prepared_statements_cache_size` when unset). When the cap is reached,
  the backend issues `Close` on the least recently used name, releasing
  the plan.
- **Backend rotation.** A backend reaches `server_lifetime`
  (default 20 min) and pg_doorman closes it; the new backend starts
  with an empty plan cache.

The worst-case memory footprint per backend is therefore
`server_prepared_statements_cache_size × average plan memory`
(`8192 × ~100 KB` is about 800 MB) on the PostgreSQL side. To shrink
the window:

- Lower `server_prepared_statements_cache_size` so the server-level LRU
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
  `client_anonymous_evictions_alive`. The last column sums the
  per-client eviction counters across the currently connected
  clients only — disconnected clients drop out of the sum, so this
  column is *not* monotonic. For the cumulative counter, scrape
  `pg_doorman_clients_prepared_anonymous_evictions_total` from the
  Prometheus surface instead.

Prometheus metrics (full list in [Prometheus](../reference/prometheus.md)):

- `pg_doorman_pool_prepared_cache_entries{user, database}`
- `pg_doorman_pool_prepared_cache_bytes`
- `pg_doorman_clients_prepared_cache_entries`
- `pg_doorman_clients_prepared_cache_bytes`
- `pg_doorman_clients_prepared_named_entries{user, database}`
- `pg_doorman_clients_prepared_anonymous_entries{user, database}`
- `pg_doorman_clients_prepared_anonymous_evictions_total{user, database}`
- `pg_doorman_servers_prepared_hits{user, database}`
- `pg_doorman_servers_prepared_misses{user, database}`
- `pg_doorman_servers_prepared_hits_total{user, database}`
- `pg_doorman_servers_prepared_misses_total{user, database}`
- `pg_doorman_async_clients_count`

Use the `_total` metrics for `rate()` and alerting. The non-`_total`
server metrics are live backend aggregates and can drop when backends
rotate.

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

Numbers near `server_prepared_statements_cache_size` per backend mean
the server-level LRU is at its cap. Backend rotation is the other
mechanism that releases plan memory. If the server cache inherits
`prepared_statements_cache_size`, use that value as the cap. Lowering
the server cap or `server_lifetime` releases plan-memory pressure at
the cost of more frequent re-Parses on the backend.

## Bounded query interner

The pool-level interner that deduplicates Parse query texts is split
into two halves:

- **NAMED** — text for named prepared statements. An entry stays alive
  as long as any pool or client cache holds an `Arc<str>` reference.
  The GC task collects entries when nothing outside the interner
  holds a reference any more, with a two-cycle grace period to avoid
  thrash on cold-but-still-needed hashes.
- **ANON** — text for anonymous prepared statements. An entry expires
  after `query_interner_anon_idle_ttl_seconds` of idle time (default
  60 seconds). Setting the knob to `0` disables TTL eviction — the
  pre-3.7 unbounded behaviour, kept as an escape hatch for legacy
  deployments.

If an anonymous `Bind` or `Describe` arrives after pg_doorman has lost
the matching anonymous prepared-statement state, pg_doorman returns
`ERROR: unnamed prepared statement does not exist` (SQLSTATE `26000`).
Common causes are client Anonymous LRU eviction, `RESET INTERNER`,
interner TTL eviction, or a driver pattern that reuses unnamed prepared
statements across batches. This is the same error native PostgreSQL
raises for the same condition; standard drivers handle it transparently
by re-issuing `Parse`.

Binary upgrade (`SIGUSR2`) carries both NAMED and ANON entries to
the new process. Anonymous entries land in the new ANON interner
with a fresh `last_used` timestamp, so the TTL clock starts over at
the upgrade moment.

### Operator surface

`SHOW INTERNER` (admin SQL) prints aggregate counts and bytes per
kind:

```
kind      | entries | bytes
named     |     420 |    87654
anonymous |    1337 |   234567
```

`SHOW INTERNER N` returns the top N entries by interned text length
with `hash`, `kind`, `bytes`, `idle_ms` (`-1` for named — the named
half tracks GC state, not last-used timestamps), and a 120-character
preview of the SQL.

`RESET INTERNER` clears both halves. In-flight clients re-Parse on
next reuse — diagnostics-only.

The Prometheus surface mirrors `SHOW INTERNER` plus a histogram for
sweep duration and a counter for the synthetic `26000`s. Raise
`query_interner_anon_idle_ttl_seconds` only when synthetic misses
correlate with anonymous TTL evictions or a known cross-batch unnamed
statement pattern. If misses correlate with
`pg_doorman_clients_prepared_anonymous_evictions_total`, increase
`client_anonymous_prepared_cache_size` instead.

## Reference

- [Pool Modes](../concepts/pool-modes.md) — transaction mode, where
  prepared-statement remapping is enabled.
- [General Settings](../reference/general.md) — `prepared_statements_cache_size`,
  `server_prepared_statements_cache_size`,
  `client_anonymous_prepared_cache_size`,
  `query_interner_gc_interval_seconds`,
  `query_interner_anon_idle_ttl_seconds`.
- [Admin Commands](../observability/admin-commands.md) — `SHOW PREPARED_STATEMENTS`,
  `SHOW POOLS_MEMORY`, `SHOW INTERNER`, `RESET INTERNER`.
- [Prometheus](../reference/prometheus.md) — full metric list.
