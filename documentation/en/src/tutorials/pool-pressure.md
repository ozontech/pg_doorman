# Pool pressure

Pool pressure is how pg_doorman handles many clients asking for a backend
connection at the same time when the idle pool is empty. Two mechanisms
decide who gets a connection, who waits, who triggers a fresh backend
connect, and who is rejected: per-pool **anticipation + bounded burst**
inside each `(database, user)` pool, and the cross-pool **coordinator**
that caps total backend connections per database.

Audience: DBA or production operator who already knows PgBouncer and
wants to understand how pg_doorman differs and what to watch.

## Why pool pressure exists

Take a pool with `pool_size = 40` and a workload of 200 short transactions
arriving in the same millisecond. The pool has 4 idle connections. In a
naive pooler the first 4 clients pick the idle connections, and the
remaining 196 each independently call `connect()` against PostgreSQL.
PostgreSQL receives 196 simultaneous TCP connect attempts, each followed
by SCRAM authentication and parameter negotiation, only to discover that
the pool allows 36 more. Backend `pg_authid` lookups spike, the
`max_connections` ceiling is hit, the kernel `accept()` queue saturates,
and tail latency for already-connected clients climbs because the
PostgreSQL postmaster is spawning backends instead of running queries.
This is the **thundering herd** problem.

```
Time:  ----------------------------------------->

Client_1   -[idle hit]--[query]-----[done]
Client_2   -[idle hit]--[query]-----[done]
Client_3   -[idle hit]--[query]-----[done]
Client_4   -[idle hit]--[query]-----[done]
Client_5   -[connect]-[auth]-[query]-[done]
Client_6   -[connect]-[auth]-[query]-[done]
   .             ^
   .             196 backend connect()s
   .             fired in the same instant
Client_200 -[connect]-[auth]-[query]-[done]

PostgreSQL: 196 spawning backends + 4 running queries
```

Pool pressure suppresses this. pg_doorman makes most of those 196 callers
reuse a connection that another client is about to release, or wait a few
milliseconds behind a small number of in-flight backend connects. The
`connect()` rate against PostgreSQL stays bounded even when client arrival
is bursty.

## Plain pool mode

This runs when `max_db_connections` is not configured. Pools are
independent, no cross-pool coordination, and pressure is managed inside
each `(database, user)` pool. This is the default, and most deployments
live here.

### Pool growth from cold

A pool with `pool_size = 40` and `min_pool_size = 0` starts with zero
connections. The first client to arrive does not wait: pg_doorman creates
a backend connection immediately. The second does the same, the third
does the same, until the pool reaches the **warm threshold**.

The warm threshold is `pool_size × scaling_warm_pool_ratio / 100`. With
the default ratio of 20% and `pool_size = 40`, the threshold is 8
connections. Below it, pg_doorman creates connections without hesitation:
the pool is cold, the cost of a wait is higher than the cost of a
connect, and clients cannot contend for idle connections that do not
exist.

Above the threshold, the **anticipation zone** activates. When a client
misses the idle pool, pg_doorman first tries to catch a connection that
another client is about to return.

A third zone overlays both: at any pool size, if `inflight_creates`
reaches `scaling_max_parallel_creates` (default 2), the pool enters the
**burst-capped state** for new creates. Additional callers wait for a
slot regardless of how many idle connections exist.

```
                        Three pressure zones
                        --------------------

Pool size:  0 ----------- 8 ---------------------------- 40
            ^             ^                              ^
            |             |                              |
            |  WARM ZONE  |  ANTICIPATION ZONE           |
            |             |                              |
            |  size <     |  size >= warm_threshold      |
            |  warm_thr   |                              |
            |             |                              |
            |  Skip       |  Phase 3: fast spin          |
            |  phases 3   |  Phase 4: Notify wait        |
            |  and 4.     |   (<= scaling_max_           |
            |  Go straight|      anticipation_wait_ms)   |
            |  to phase 5 |  Then phase 5                |
            |  (burst gate|                              |
            |  + connect) |                              |

                  Burst-capped state (orthogonal)
                  -------------------------------

inflight_creates: 0 ---- 1 ---- 2 (= scaling_max_parallel_creates)
                                ^
                                |  At cap: any caller reaching the
                                |  burst gate waits on a Notify
                                |  for either an idle return or
                                |  a peer create completion.
```

The warm/anticipation zones track *current pool size*. The burst-capped
state tracks *concurrent backend creates*. A pool can be in the
anticipation zone and the burst-capped state at the same time; this is
the common case under load. A pool below the warm threshold can also
hit the burst cap if many clients arrive at once during cold-start fill.

### Acquiring a connection

When a client requests a connection through `pool.get()`, pg_doorman
walks through the following phases. Each phase either returns a
connection or hands off to the next phase.

**Phase 1 — Hot path recycle.** Pop the front of the idle queue. If a
connection is there and passes the recycle check (rollback, validity,
epoch), return it. A healthy steady-state pool only takes this path.
Cost: a mutex acquire and a recycle check.

**Phase 2 — Warm zone gate.** If the pool size is below the warm
threshold, skip anticipation and jump straight to creating a new backend
connection. Cold pools fill fast.

**Phase 3 — Anticipation spin.** Above the warm threshold, retry the
recycle 10 times in a tight `yield_now` loop (controlled by
`scaling_fast_retries`). This catches the case where another client
finished its query in the same microsecond range and is about to push
the connection back. Total cost is around 10–50 microseconds. No sleep,
no blocking I/O.

**Phase 4 — Anticipation wait.** If the spin did not catch a return,
register a `Notify` future that wakes when *any* client returns a
connection. Wait on that future, bounded by:

- `scaling_max_anticipation_wait_ms` (default 100 ms), and
- half of the client's remaining `query_wait_timeout` budget.

The lower of the two is used, with a 1 ms floor so the wait has a chance
to register. If a return fires during the wait, exactly **one** task
wakes, never all of them at once. If the wait elapses without a return,
drop through to phase 5.

**Phase 5 — Bounded burst gate.** Try to take one of
`scaling_max_parallel_creates` slots (default 2) for in-flight backend
connects. If a slot is free, take it and call `connect()` against
PostgreSQL. If all slots are full, wait on a `Notify` woken by either an
idle return or another in-flight create finishing, then re-try the
recycle and the gate. A 5 ms backoff acts as a safety net if both wake
sources are missed.

**Phase 6 — Backend connect.** Run `connect()`, authenticate, hand the
connection to the client. The burst slot is released automatically when
this phase finishes, regardless of success or failure.

```
                  Plain mode acquisition flow
                  ---------------------------

   pool.get()
       |
       v
   +--------------+
   |  Phase 1:    |  --- HIT ----> return idle connection
   |  recycle pop |
   +------+-------+
          | MISS
          v
   +--------------+
   |  Phase 2:    |  --- below warm ---> jump to phase 5
   |  warm gate   |
   +------+-------+
          | above warm
          v
   +--------------+
   |  Phase 3:    |  --- HIT ----> return idle connection
   |  fast spin   |
   +------+-------+
          | MISS
          v
   +--------------+
   |  Phase 4:    |  --- HIT     ----> return idle connection
   |  anticipate  |  --- notify  ----> return idle connection
   |  Notify wait |  --- timeout ----> fall through
   +------+-------+
          |
          v
   +--------------+
   |  Phase 5:    |  --- slot taken --> proceed to phase 6
   |  burst gate  |  --- slot full  --> wait, retry recycle
   +------+-------+
          |
          v
   +--------------+
   |  Phase 6:    |
   |  connect()   | ----> return new connection
   +--------------+
```

### Burst suppression in action

The same 200-client thundering herd scenario, this time with plain mode
and `scaling_max_parallel_creates = 2`:

```
Time:   t=0ms     t=5ms    t=10ms   t=15ms   t=20ms   t=25ms

C_1     [idle]--[query]-[done]
C_2     [idle]--[query]-[done]
C_3     [idle]--[query]-[done]
C_4     [idle]--[query]-[done]
C_5     [spin/wait]------[recycled C_1]--[query]-[done]
C_6     [spin/wait]------[recycled C_2]--[query]-[done]
C_7     [gate=1]-[connect]----[auth]--[query]-[done]
C_8     [gate=2]-[connect]----[auth]--[query]-[done]
C_9     [gate full, wait]---[recycled C_3]--[query]
C_10    [gate full, wait]---[recycled C_4]--[query]
  .
  .     [...196 clients use a mix of recycle, anticipation, and at
  .      most 2 in-flight connects...]
  .
C_200   [gate=2]-[connect]--[auth]--[query]--[done]

PostgreSQL: at most 2 spawning backends at any moment
            + the 4 connections that were already there
```

The same pool serves all 200 clients, but PostgreSQL never sees more
than `scaling_max_parallel_creates` (default 2) concurrent backend
spawns from this pool. Most clients land on a recycled connection from
a peer that finished moments earlier, not a fresh `connect()`.

### Non-blocking checkout

When a client sets `query_wait_timeout = 0` it asks for either an
immediate idle hit or a fresh connect, with no waiting. The anticipation
phase and the burst-gate wait are both skipped. pg_doorman runs the
hot-path recycle, tries the burst gate once, then either creates a
connection or returns a wait timeout error.

**Limitation when the coordinator is enabled.** Non-blocking only skips
the anticipation and burst-gate waits inside the per-pool path. If
`max_db_connections` is configured and the coordinator's wait phases
(B–D) take time, a non-blocking caller still blocks inside
`coordinator.acquire()` for up to `reserve_pool_timeout` (default 3000
ms) before returning. For a strict zero-wait deadline on
coordinator-managed databases, set `reserve_pool_timeout` low enough to
fit your tolerance.

### Background replenish

When `min_pool_size` is set, a background task periodically tops up the
pool to its minimum. It uses the same burst gate as client traffic.
**It does not queue** behind a busy gate: it gives up immediately and
retries on the next retain cycle (default every 30 seconds, controlled
by `retain_connections_time`).

The reasoning: during a load spike, clients are already saturating the
gate creating connections they need *right now*. Having the replenish
task fight them for slots buys nothing; client-driven creates will lift
the pool above `min_pool_size` anyway. The `replenish_deferred` counter
increments each time the background task backs off this way.

Consequence: `min_pool_size` is best-effort under load. For a hard
floor, see the troubleshooting section.

## Sizing the cap against PostgreSQL

Before reading about the coordinator, check that your worst-case backend
connection count fits PostgreSQL. Without `max_db_connections` set, the
worst case for one database is:

```
N pools (users) × pool_size  =  ceiling on backend connections
```

Worked example: three pools, `pool_size = 40` each, no
`max_db_connections`. Worst case is **120 simultaneous backend
connections** to that database, throttled only by
`scaling_max_parallel_creates` per pool (default 2 each, so up to 6
concurrent `connect()` calls in flight). If PostgreSQL is configured
with `max_connections = 100`, the database refuses new connections
during a workload-wide spike and clients see `FATAL: too many
connections`.

Two fixes:

- Lower `pool_size` so `N × pool_size` fits below `max_connections`,
  with margin for `superuser_reserved_connections`, replication slots,
  and any direct connectors that bypass pg_doorman.
- Set `max_db_connections` to enforce a hard cap (next section).

Rule of thumb: keep aggregate pg_doorman demand at most 80% of
PostgreSQL `max_connections - superuser_reserved_connections`. The
remaining 20% is headroom for admin connections, replication, and
burst.

## Coordinator mode

Coordinator mode activates when you set `max_db_connections` on a pool.
It adds a second pressure layer **above** the per-pool one: a shared
semaphore that caps total backend connections to a database across all
user pools serving it. Without it, the `N × pool_size` ceiling from the
previous section is the only limit. With `max_db_connections = 80`,
only 80 can exist at once regardless of pool configuration, and the
coordinator decides which pools may grow.

When `max_db_connections = 0` (the default), the coordinator does not
exist. When set, every plain-mode mechanism described above still runs;
the coordinator adds a single permit acquisition step on the
new-connection path. Idle reuse never touches the coordinator.

### What the coordinator adds

Three things:

1. **A hard cap** on total connections per database. If 80 are in use,
   the 81st request waits or fails, regardless of which pool asks.

2. **Eviction.** When the cap is reached and a new pool needs a slot,
   the coordinator can close an idle connection from a different user's
   pool to free one. The evicted pool loses a connection; the
   requesting pool gets one. This is fair: users with the largest
   surplus above their **effective minimum** lose connections first,
   and only connections older than `min_connection_lifetime` (default
   5000 ms) are eligible.

   The **effective minimum** for a user pool is
   `max(user.min_pool_size, pool.min_guaranteed_pool_size)`. Both knobs
   protect connections from eviction; whichever is larger wins.
   Lowering either drops the floor.

3. **A reserve pool.** If the cap is reached, eviction yields nothing,
   and waiting for a return times out, the coordinator can grant a
   permit from the **reserve**: a small extra pool above
   `max_db_connections`. The reserve is bounded by `reserve_pool_size`
   (default 0, meaning disabled) and prioritised: starving users (those
   below their **effective minimum**) and users with many queued
   clients are served first.

### Coordinator acquisition phases

When the per-pool path reaches the new-connection step, the coordinator
runs five phases. The first phase that hands back a permit ends the
sequence.

**Phase A — Try-acquire.** Non-blocking semaphore acquire. If the cap
is not reached, take the slot and return.

**Phase B — Eviction.** Walk all *other* user pools for the same
database, find the one with the largest surplus above its **effective
minimum**, and close one of its idle connections older than
`min_connection_lifetime`. The evicted permit drops synchronously,
freeing the slot. Re-try the semaphore acquire. If two callers race,
the loser falls through to the next phase.

**Phase C — Wait.** Register a `Notify` woken when any in-use
connection is returned to the coordinator. Wait up to
`reserve_pool_timeout` (default 3000 ms) for the notify or the
deadline. **This timeout applies even when `reserve_pool_size = 0`**:
it is the wait-phase budget, not just the reserve gating window. If
your `query_wait_timeout` is shorter than `reserve_pool_timeout`, the
client gives up first and you see `wait timeout` errors instead of the
more diagnostic `all server connections to database 'X' are in use`.
See troubleshooting for the symptom.

**Phase D — Reserve.** If the wait expired and `reserve_pool_size > 0`,
ask the reserve arbiter for a permit. Requests are scored by
`(starving, queued_clients)` so users that need connections most get
them first. The arbiter is a single tokio task that drains reserve
permits from a priority heap.

**Phase E — Error.** If the reserve is exhausted or not configured,
the client receives an error: `all server connections to database 'X'
are in use (max=N, ...)`.

### Why coordinator runs before the burst gate

Inside the per-pool acquisition flow, the coordinator permit is
acquired **before** the burst gate. The order is deliberate.

The coordinator can wait *seconds* (up to `reserve_pool_timeout`,
default 3000 ms). The burst gate wakes in *milliseconds*. If the gate
came first, two callers in one pool could grab the only two slots,
both block on the coordinator for seconds waiting for a peer pool to
return, and the rest of the clients in their own pool would starve
waiting for those two, even though the pool itself has nothing to do
but `connect()`. This is **head-of-line blocking inside one pool**.

With coordinator first, the gate caps **actual `connect()` calls**,
not *waiting time on a peer pool*. A caller blocked in coordinator
wait holds zero burst slots. The gate sees at most one caller per
slot, each about to issue `connect()`.

```
        Coordinator + plain mode acquisition flow
        -----------------------------------------

   pool.get()
       |
       v
   Phase 1: hot path recycle   --- HIT ---> return
       | MISS
       v
   Phase 2: warm gate          --- below ---+
       | above warm                         |
       v                                    |
   Phase 3: fast spin          --- HIT ---> return
       | MISS                               |
       v                                    |
   Phase 4: anticipation wait  --- HIT ---> return
       | timeout                            |
       v                                    |
       | <----------------------------------+
       v
   +----------------------+
   |  Coordinator acquire |   <-- inserted between phase 4 and phase 5
   |   A: try_acquire     |       only when max_db_connections > 0
   |   B: evict from peer |
   |   C: wait for return |   up to reserve_pool_timeout
   |   D: reserve permit  |   scored priority
   |   E: error           |   client gets DB exhausted error
   +----------+-----------+
              | permit granted
              v
   Phase 5: bounded burst gate (scaling_max_parallel_creates)
              | slot acquired
              v
   Phase 6: server_pool.create()
              |
              v
              return new connection
```

The phases are numbered identically to plain mode. The coordinator
acquire is **not** a numbered phase: it is a separate gate inserted
between phase 4 and phase 5 when `max_db_connections > 0`. In plain
mode it does not run.

### When the coordinator is configured but the cap is not reached

If `max_db_connections = 80` and current usage is 30, the coordinator's
phase A always succeeds. Phases B–E never run. The behaviour is
identical to plain mode plus one atomic semaphore increment per new
connection. The hot path (idle reuse) does not touch the coordinator at
all, so it has no measurable cost there. Only *new* connection creation
does, and only by the duration of one atomic operation.

By design, the coordinator is a *cap*, not a *queue*: it costs you
only when you bump against the limit.

### Background replenish under coordinator

`replenish` acquires its coordinator permit using `try_acquire`
(non-blocking). If the database is at the cap, replenish gives up and
retries on the next retain cycle. Same logic as the burst gate
backoff: don't have a background task fight client traffic for scarce
permits.

## Tuning parameters

All four scaling parameters are global by default, with per-pool
overrides for `scaling_warm_pool_ratio` and `scaling_fast_retries`.
The two anticipation/burst knobs are global only; per-pool overrides
are not supported.

| Parameter | Default | Where | What it does |
|---|---|---|---|
| `scaling_warm_pool_ratio` | `20` (percent) | `general`, per-pool | Threshold below which connections are created without anticipation. Below `pool_size × ratio / 100`, every new connection request goes straight to `connect()`. |
| `scaling_fast_retries` | `10` | `general`, per-pool | Number of `yield_now` spin retries in the anticipation phase before falling through to the event-driven wait. |
| `scaling_max_anticipation_wait_ms` | `100` (ms) | `general` | Upper bound on the event-driven wait for an idle return before falling through to backend connect. Capped at half the client's remaining `query_wait_timeout`. |
| `scaling_max_parallel_creates` | `2` | `general` | Hard cap on concurrent backend `connect()` calls per pool. Tasks above the cap wait for an idle return or a peer create completion. Must be `>= 1`. |
| `max_db_connections` | unset (disabled) | per-pool | Cap on total backend connections to a database across all user pools. When unset, the coordinator does not exist. |
| `min_connection_lifetime` | `5000` (ms) | per-pool | Minimum age of an idle connection before the coordinator may evict it for another pool. Lower bound on connection churn. |
| `reserve_pool_size` | `0` (disabled) | per-pool | Extra coordinator permits above `max_db_connections`, granted by priority when the main pool is exhausted. |
| `reserve_pool_timeout` | `3000` (ms) | per-pool | Maximum coordinator wait time before falling through to the reserve pool. |
| `min_guaranteed_pool_size` | `0` | per-pool | Per-user minimum protected from coordinator eviction. A user with `current_size <= min_guaranteed_pool_size` has its connections immune to eviction by other users. |

### When to raise `scaling_max_parallel_creates`

Raise when:

- `burst_gate_waits` is consistently growing across scrapes and
  `replenish_deferred` is also non-zero, meaning client traffic and the
  background task are both fighting for slots that don't exist;
- backend `connect()` is fast (< 50 ms) and PostgreSQL has spare
  `max_connections`;
- connection latency spikes correlate with `burst_gate_waits` rate
  increases.

**Hard ceiling.** Never raise `scaling_max_parallel_creates` above
either of these limits:

- `pool_size / 4` for the smallest pool that uses this setting. Above
  this, the cap loses meaning: half the pool can be in flight at once,
  defeating the smoothing.
- `(PostgreSQL max_connections - superuser_reserved_connections) / (10 × N pools)`
  where `N pools` counts all pools sharing this PostgreSQL instance.
  Above this, the aggregate concurrent connect rate exceeds what the
  backend can absorb without `accept()` queue overflow.

Lower when:

- PostgreSQL `connect()` is expensive (> 200 ms, e.g., SSL with cert
  verification, or a slow `pg_authid` lookup);
- `pg_authid` contention shows up in PostgreSQL logs;
- the backend shows `accept()` queue overflow.

Symptom of too low: `burst_gate_waits` rate climbs faster than client
arrival rate. Symptom of too high: PostgreSQL `connect()` latency
climbs and the connection storm reappears.

**Sizing for many pools.** The aggregate concurrent connect ceiling is
`N pools × scaling_max_parallel_creates`. If you operate one PostgreSQL
behind 10 pools and want at most 8 concurrent backend connects across
all of them at any moment, set `scaling_max_parallel_creates` to
roughly `desired_aggregate / N pools`, rounding down. Below 1 is not
allowed; if the math gives <1, lower `N pools` by consolidating users.

### When to raise `scaling_max_anticipation_wait_ms`

Raise when:

- `anticipation_wakes_timeout` is much larger than
  `anticipation_wakes_notify` and the pool is *not* under-sized (most
  queries finish faster than 100 ms but the anticipation budget is too
  short to catch them);
- latency p99 spikes correlate with `create_fallback` rate.

Lower when:

- `query_wait_timeout` is short (< 200 ms) and you cannot afford to
  burn 100 ms on anticipation;
- `anticipation_wakes_notify` is high (the optimistic path works) but
  individual clients see the wait inflate their tail latency.

Symptom of too low: `create_fallback` and `anticipation_wakes_timeout`
both grow faster than connection creation could justify. Symptom of
too high: client tail latency includes a steady contribution from
anticipation wait when the pool is genuinely undersized.

### When to raise `scaling_warm_pool_ratio`

Raise when:

- pools are slow to warm at startup and `min_pool_size` is not used;
- clients wait for anticipation when the pool is mostly empty
  (anticipation only activates above the warm threshold, so this
  shouldn't happen, but a high ratio narrows the window where it
  *can*).

Lower when:

- pools are over-sized and you want anticipation to suppress creates
  earlier in the size range.

This knob rarely needs touching. The default of 20% works for most
workloads.

### When to set `max_db_connections`

Set it when:

- one PostgreSQL host serves multiple `(database, user)` pools and the
  sum of `pool_size` across pools exceeds the database's
  `max_connections`;
- you want a hard ceiling that survives misconfiguration of any single
  pool;
- you want cross-pool fairness via eviction.

Leave it unset when:

- one pool serves one database and `pool_size` is the whole story;
- you don't want any cross-pool eviction (some workloads prefer hard
  per-user isolation).

### `reserve_pool_size` and `reserve_pool_timeout`

The reserve is a *temporary overflow valve*, not extra steady-state
capacity. It prevents client-visible exhaustion errors during brief
bursts. Under normal operation `reserve_in_use` should be 0 most of
the time.

Sizing rule of thumb: `reserve_pool_size ≤ 0.25 × max_db_connections`.
The reserve absorbs a spike; it does not double the cap.

`reserve_pool_timeout` is how long a client waits in coordinator phase
C before the reserve is consulted. Default 3000 ms is conservative.
Lower it if your `query_wait_timeout` is short and you would rather
fall through to the reserve fast than block clients on coordinator
wait.

**Floor.** Never lower `reserve_pool_timeout` below `2 × your p99
query latency`. Below that floor, the wait phase always times out
before a peer returns a connection, and the reserve becomes a
required permit for every new connection rather than an overflow
valve. Reserve permits are scarce by design; using them as steady
state defeats the purpose.

**Trap: `query_wait_timeout < reserve_pool_timeout`.** When the
client deadline is shorter than the coordinator wait phase, the
client gives up first and you see `wait timeout` errors instead of
the more diagnostic `all server connections to database 'X' are in
use`. The coordinator's wait and reserve phases run their full course
but no client is left to receive the result. The pg_doorman config
validator emits a warning at startup; act on it.

## Observability

pg_doorman exposes pool pressure state through the admin console and
through Prometheus. Both show the same counters; pick whichever fits
your monitoring stack.

### Admin: `SHOW POOL_SCALING`

Per-pool counters for the anticipation + bounded burst path. Connect
to the `pgdoorman` admin database and run:

```sql
pgdoorman=> SHOW POOL_SCALING;
```

| Column | Type | Meaning |
|---|---|---|
| `user` | text | Pool user |
| `database` | text | Pool database |
| `inflight` | gauge | Backend `connect()` calls currently in progress for this pool. Bounded by `scaling_max_parallel_creates`. |
| `creates` | counter | Total backend connections this pool has started creating since startup. Pairs with `gate_waits` to compute the gate hit rate. |
| `gate_waits` | counter | Total times a caller observed the burst gate at capacity and had to wait on a `Notify`. High values indicate `scaling_max_parallel_creates` is too low. |
| `antic_notify` | counter | Anticipation waits that woke on a real idle return. The optimistic path paid off. |
| `antic_timeout` | counter | Anticipation waits that fell through on the budget timeout instead of catching a return. Ratio against `antic_notify` shows whether `scaling_max_anticipation_wait_ms` is well-calibrated. |
| `create_fallback` | counter | Times anticipation completed but `try_recycle` still found the pool empty, forcing a fresh `connect()`. |
| `replenish_def` | counter | Background replenish runs that hit the burst cap and deferred to the next retain cycle. Persistent non-zero values mean `min_pool_size` cannot be sustained under current load. |

All counters are monotonic since startup. Compute deltas between
scrapes; absolute values are only useful for ratios.

### Admin: `SHOW POOL_COORDINATOR`

Per-database coordinator state. Only present for databases with
`max_db_connections > 0`.

```sql
pgdoorman=> SHOW POOL_COORDINATOR;
```

| Column | Type | Meaning |
|---|---|---|
| `database` | text | Database name |
| `max_db_conn` | gauge | Configured `max_db_connections` |
| `current` | gauge | Total backend connections currently held under this coordinator (across all user pools) |
| `reserve_size` | gauge | Configured `reserve_pool_size` |
| `reserve_used` | gauge | Reserve permits currently in use |
| `evictions` | counter | Total times the coordinator evicted an idle connection from a peer pool to free a slot |
| `reserve_acq` | counter | Total reserve permits granted by the arbiter |
| `exhaustions` | counter | Times the coordinator returned an exhausted error to a client. **This is the primary pager signal.** |

### Prometheus metrics

Two metric families per pool, two per coordinator. All four use
`pg_doorman_pool_scaling*` and `pg_doorman_pool_coordinator*` namespaces.

| Metric | Type | Labels | Source |
|---|---|---|---|
| `pg_doorman_pool_scaling{type="inflight_creates"}` | gauge | `user`, `database` | `inflight` from `SHOW POOL_SCALING` |
| `pg_doorman_pool_scaling_total{type="creates_started"}` | counter | `user`, `database` | `creates` |
| `pg_doorman_pool_scaling_total{type="burst_gate_waits"}` | counter | `user`, `database` | `gate_waits` |
| `pg_doorman_pool_scaling_total{type="anticipation_wakes_notify"}` | counter | `user`, `database` | `antic_notify` |
| `pg_doorman_pool_scaling_total{type="anticipation_wakes_timeout"}` | counter | `user`, `database` | `antic_timeout` |
| `pg_doorman_pool_scaling_total{type="create_fallback"}` | counter | `user`, `database` | `create_fallback` |
| `pg_doorman_pool_scaling_total{type="replenish_deferred"}` | counter | `user`, `database` | `replenish_def` |
| `pg_doorman_pool_coordinator{type="connections"}` | gauge | `database` | `current` from `SHOW POOL_COORDINATOR` |
| `pg_doorman_pool_coordinator{type="reserve_in_use"}` | gauge | `database` | `reserve_used` |
| `pg_doorman_pool_coordinator{type="max_connections"}` | gauge | `database` | `max_db_conn` |
| `pg_doorman_pool_coordinator{type="reserve_pool_size"}` | gauge | `database` | `reserve_size` |
| `pg_doorman_pool_coordinator_total{type="evictions"}` | counter | `database` | `evictions` |
| `pg_doorman_pool_coordinator_total{type="reserve_acquisitions"}` | counter | `database` | `reserve_acq` |
| `pg_doorman_pool_coordinator_total{type="exhaustions"}` | counter | `database` | `exhaustions` |

### Alerts to set

The following alerts cover the failure modes that warrant a page or
warn. They're written in Prometheus syntax; adapt to your stack. All
use sustained-condition windows so brief bursts do not page the
on-call.

If you reload pg_doorman frequently and pools come and go, scope the
alerts to recently-active pools (e.g., add
`pg_doorman_pool_scaling_total{type="creates_started"} > 0` as a
gating filter).

**Coordinator exhaustion (page).** A client received a "database
exhausted" error. Hard failure.
**Runbook:** see Troubleshooting → "`max_db_connections` exhausted".

```promql
rate(pg_doorman_pool_coordinator_total{type="exhaustions"}[5m]) > 0
```

**Burst gate saturated (warn).** Roughly half the new-connection
attempts queued at least once. Brief spikes above 0.5 during failover
or restart are normal; sustained values mean
`scaling_max_parallel_creates` is too low for offered load.

```promql
rate(pg_doorman_pool_scaling_total{type="burst_gate_waits"}[5m])
  > 0.5 * rate(pg_doorman_pool_scaling_total{type="creates_started"}[5m])
```

**Anticipation calibration drifting (warn).** More anticipation waits
fall through on timeout than are caught by a real return, suggesting
`scaling_max_anticipation_wait_ms` is below the typical query latency.
**Action:** raise `scaling_max_anticipation_wait_ms` to roughly your
p90 query latency.

```promql
rate(pg_doorman_pool_scaling_total{type="anticipation_wakes_timeout"}[5m])
  > 2 * rate(pg_doorman_pool_scaling_total{type="anticipation_wakes_notify"}[5m])
```

**Replenish deferred persistently (warn).** The background task cannot
sustain `min_pool_size` because the burst gate is busy with client
traffic. Sustained over an hour, not a brief spike.

```promql
increase(pg_doorman_pool_scaling_total{type="replenish_deferred"}[1h]) > 60
```

**Reserve pool continuously in use (warn).** The reserve is meant for
brief bursts. This rule fires only when the reserve has been in use
**continuously** for 15 minutes, not momentary use.

```promql
min_over_time(pg_doorman_pool_coordinator{type="reserve_in_use"}[15m]) > 0
```

**Coordinator approaching cap (warn).** Lead time before exhaustion.
The `> 0` guard avoids dividing by zero on databases where the
coordinator is disabled.

```promql
pg_doorman_pool_coordinator{type="max_connections"} > 0
  and
  pg_doorman_pool_coordinator{type="connections"}
    / pg_doorman_pool_coordinator{type="max_connections"} > 0.85
```

**Inflight stuck at cap (warn).** `inflight_creates` sitting at the
configured cap for 5+ minutes means `connect()` calls are not
finishing. Check backend health.

```promql
min_over_time(pg_doorman_pool_scaling{type="inflight_creates"}[5m])
  >= 2  # adjust to your scaling_max_parallel_creates value
```

**Coordinator thrashing (warn).** Cap is full *and* evictions are
happening: the coordinator is constantly closing peer connections to
make room. The pool is undersized for offered load, not "occasionally
pressured".

```promql
pg_doorman_pool_coordinator{type="connections"}
    / pg_doorman_pool_coordinator{type="max_connections"} > 0.95
  and
  rate(pg_doorman_pool_coordinator_total{type="evictions"}[5m]) > 0
```

### Reading the admin output during an incident

The admin console accepts only `SHOW <subcommand>`, `SET`, `RELOAD`,
`SHUTDOWN`, `UPGRADE`, `PAUSE`, `RESUME`, and `RECONNECT`. `SHOW` is
not a virtual table, so there is no `SELECT` against the admin
database. To query the counters in shell pipelines, run `SHOW` from
`psql` and post-process the output.

The patterns below use `psql` against the admin listener (default
credentials `admin/admin`):

```bash
# Highest burst-gate-wait ratio first (the hot pool).
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
     -c 'SHOW POOL_SCALING' --no-align --field-separator='|' \
  | awk -F'|' 'NR>1 && $4>0 { printf "%-20s %-20s %.3f  inflight=%d  defer=%d\n", $1, $2, $5/$4, $3, $9 }' \
  | sort -k3 -nr | head

# Highest anticipation miss ratio (timeout vs notify).
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
     -c 'SHOW POOL_SCALING' --no-align --field-separator='|' \
  | awk -F'|' 'NR>1 && ($6+$7)>0 { printf "%-20s %-20s %.3f  notify=%d  timeout=%d\n", $1, $2, $7/($6+$7), $6, $7 }' \
  | sort -k3 -nr | head

# Coordinator: closest databases to exhaustion.
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
     -c 'SHOW POOL_COORDINATOR' --no-align --field-separator='|' \
  | awk -F'|' 'NR>1 && $2>0 { printf "%-30s %.3f  used=%d/%d  reserve=%d  exhaustions=%d\n", $1, $3/$2, $3, $2, $5, $8 }' \
  | sort -k2 -nr
```

Field positions in `awk` follow the column order documented above:
`POOL_SCALING` is `user|database|inflight|creates|gate_waits|antic_notify|antic_timeout|create_fallback|replenish_def`,
`POOL_COORDINATOR` is `database|max_db_conn|current|reserve_size|reserve_used|evictions|reserve_acq|exhaustions`.

## Comparison with PgBouncer

PgBouncer and pg_doorman both pool, but they handle pressure differently.

| Concern | PgBouncer | pg_doorman |
|---|---|---|
| Per-pool size cap | `pool_size` | `pool_size` |
| Cross-pool DB-level cap | `max_db_connections` (hard cap, no eviction; per-database/per-user `pool_size` overrides for isolation) | `max_db_connections` (hard cap, plus cross-pool eviction and reserve pool) |
| Reserve pool | `reserve_pool_size`, `reserve_pool_timeout` | `reserve_pool_size`, `reserve_pool_timeout` (plus arbiter prioritisation by starving/queued) |
| Eviction across users | Not supported. A user holding idle connections starves a peer needing them. | Coordinator evicts idle connections from the user with the largest surplus above the **effective minimum** (`max(user.min_pool_size, min_guaranteed_pool_size)`). |
| Concurrent backend `connect()` per pool | Single-threaded, processes events serially per pool — `connect()` calls fire one at a time. | Bounded by `scaling_max_parallel_creates` (default 2 per pool): up to N concurrent backend connects per pool, capped against the offered load. |
| Anticipation of returns | None. Clients wait on `wait_timeout` for the next available connection in arrival order. | Event-driven anticipation: a returning connection wakes exactly one queued waiter, often before any new `connect()` is issued. |
| `min_pool_size` prewarm | Maintained on every event-loop tick (no separate replenish task). | Periodic background replenish (`retain_connections_time`, default 30 s) that defers when the burst gate is busy. |
| Backend login retry-after-failure | `server_login_retry` (default 15 s) blocks new login attempts after a backend rejection. | No equivalent. Backend login failures propagate directly to the client per attempt. |
| Lifetime jitter | None. `server_lifetime` is exact. | ±20% jitter on both `server_lifetime` and `idle_timeout` to avoid synchronised mass closures. |
| Pool lookup key | `(database, user, auth_type)` | `(database, user)` |
| Fairness across users on a shared cap | First come first served on `max_db_connections`. | Reserve arbiter scores requests by `(starving, queued_clients)`. |
| Observability of new-connection pressure | `SHOW POOLS`, `SHOW STATS`. No insight into in-flight connects or anticipation outcomes. | `SHOW POOL_SCALING` and `SHOW POOL_COORDINATOR` expose every counter the new code path uses. |

Two differences matter most in production:

1. **Bounded burst gate.** PgBouncer's pool size limits how many
   *connections* you have, but does not limit how many `connect()`
   calls fire at the same time when many clients arrive in the same
   instant. pg_doorman caps the simultaneous backend `connect()`
   rate independently of pool size, so a sudden traffic spike does
   not translate into a connection storm against PostgreSQL.

2. **Cross-pool eviction.** PgBouncer's `max_db_connections` is a
   hard ceiling with no way to redistribute. If user A holds 80 idle
   connections and user B needs one but the cap is reached, user B
   waits or fails. pg_doorman's coordinator can close one of A's
   idle connections (if older than `min_connection_lifetime`) and
   give the slot to B.

## Troubleshooting

### Multiple simultaneous backend connect log lines

**Symptom.** Server logs (or pg_doorman debug logs) show 5 or more
backend `connect()` events in the same millisecond, suggesting the
burst gate is not working.

**Cause.** Either `scaling_max_parallel_creates` is set too high
(verify in `SHOW CONFIG` or your `pg_doorman.yaml`), or there are 5 or
more pools each independently issuing concurrent connects (the gate is
per-pool, not global).

**Fix.** Lower `scaling_max_parallel_creates`. The default of 2 fits
most workloads. With many pools, the *aggregate* concurrent connect
rate is `pools × scaling_max_parallel_creates`, which is expected.
To bound the aggregate, set `max_db_connections` per database; the
coordinator will then queue creates beyond the cap.

### `min_pool_size` is not being maintained

**Symptom.** A pool with `min_pool_size = 10` shows `sv_idle = 4` in
`SHOW POOLS` and stays there for minutes.

**Cause.** Background replenish is deferring because the burst gate is
busy with client traffic. Check `replenish_def` in `SHOW POOL_SCALING`.
If it keeps growing, replenish skips every retain cycle.

**Fix.** By design, under load, client-driven creates own the gate.
The pool reaches `min_pool_size` once client traffic eases. For a
hard floor, raise `scaling_max_parallel_creates` so replenish has
spare capacity, or shorten `retain_connections_time` so replenish
runs more often.

For **transaction pooling** (`pool_mode = transaction`), setting
`min_pool_size` higher than `pool_size / 2` usually indicates an
undersized pool: most connections should be available for client
checkouts, not pinned at minimum. For **session pooling** the
heuristic does not apply: `min_pool_size = pool_size` is a
legitimate setup to keep all session-scoped state hot.

### Latency p99 climbing without obvious cause

**Symptom.** Client p99 latency rises while p50 stays flat. Pool size
looks fine, no errors in logs.

**Cause.** Anticipation is timing out and clients are paying 100 ms of
wait time on top of their query latency. Check
`antic_timeout / antic_notify` ratio in `SHOW POOL_SCALING`.

**Fix.** Two cases.

- If the ratio is high (timeout > notify) and `create_fallback` is
  also growing: anticipation is failing to catch returns. Either
  raise `scaling_max_anticipation_wait_ms` so anticipation can wait
  longer for a return, or accept that the pool is undersized and
  raise `pool_size`.
- If the ratio is low (notify > timeout) but p99 is still high: the
  pool is fine, the latency is somewhere else (PostgreSQL, network,
  client side). Check `SHOW STATS avg_wait_time` to confirm
  pg_doorman is not the bottleneck.

### `max_db_connections` exhausted, clients receive errors

**Symptom.** Clients see errors like `all server connections to database
'X' are in use (max=80, ...)`. `pg_doorman_pool_coordinator_total{type="exhaustions"}`
is climbing.

**Cause.** All five coordinator phases failed: try-acquire failed,
nothing was evictable, the wait timed out, and either the reserve was
exhausted or `reserve_pool_size = 0`.

**Fix.** Walk the phases in order.

1. Check `current` vs `max_db_conn` in `SHOW POOL_COORDINATOR`. If
   `current` is at the cap consistently, your offered load exceeds the
   cap. Either raise `max_db_connections` or look for a runaway pool.
2. Check `evictions` rate. If it's zero or near-zero, eviction is not
   helping: every pool's idle connections are younger than
   `min_connection_lifetime` (default 5000 ms), or every other pool is
   at its `min_guaranteed_pool_size`. Lower `min_connection_lifetime`
   if your workload has very short queries, or increase
   `max_db_connections`.
3. Check `reserve_used` vs `reserve_size`. If the reserve is fully
   occupied, raise `reserve_pool_size`. If it's empty but `exhaustions`
   are happening, the reserve is not configured (`reserve_pool_size = 0`).
   Set it to absorb bursts.
4. Look at `SHOW POOLS` for the database. If one user has a much larger
   `sv_idle` than others, that user is hoarding connections; consider
   `min_guaranteed_pool_size` to protect smaller users from being
   crushed by it, or lower the hoarder's `pool_size`.

### Coordinator wait phase is the bottleneck

**Symptom.** Clients pay 3 seconds of latency on average, exactly
matching `reserve_pool_timeout`.

**Cause.** Phase C wait is consistently timing out. Either the database
is genuinely at the cap and no connections are returning, or
`reserve_pool_size = 0` so the wait runs to completion before the
client receives any response.

**Fix.** Lower `reserve_pool_timeout` to fail fast, or set
`reserve_pool_size > 0` so phase D handles the overflow within the same
acquisition path.

### Burst gate is the bottleneck even with low traffic

**Symptom.** `gate_waits` rate is significant but `creates` rate is low,
and `inflight_creates` is at the cap continuously.

**Cause.** Backend `connect()` is slow. Each create holds a slot for
seconds; even with two slots, you can only create roughly `2 / connect_seconds`
connections per second.

**Fix.** Investigate why `connect()` is slow on the PostgreSQL side
(SCRAM iterations too high, `pg_authid` lock contention, slow DNS,
SSL handshake). Once `connect()` is fast, the gate stops being the
bottleneck. Raising `scaling_max_parallel_creates` papers over the
problem and pushes the storm to PostgreSQL. Investigate first, raise
the cap second.

### `is_starving` users keep getting reserve permits

**Symptom.** `reserve_acquisitions_total` keeps increasing. The same
small user is the one acquiring most reserves.

**Cause.** A user is below its **effective minimum**
(`max(user.min_pool_size, min_guaranteed_pool_size)`) and the
coordinator cannot satisfy that minimum without evicting from peers.
Each client request from that user hits coordinator phase D and
grabs a reserve. **The deeper question is why the user keeps
needing fresh connections**: either its `pool_size` is too low to
absorb its own load, or its traffic is bursty and the reserve is
doing what reserves are for.

**Fix.** Three options, pick by the deeper cause:

- If the user's `pool_size` is genuinely too small for steady-state
  load, raise `pool_size` and (if needed) `max_db_connections` so the
  larger pool fits.
- If the user has a high effective minimum that the coordinator
  cannot satisfy, lower **whichever knob is actually setting the
  floor** (check both `user.min_pool_size` and `min_guaranteed_pool_size`).
- If the traffic is genuinely bursty and reserves are catching the
  bursts, leave it alone. Brief reserve usage is the design.

### Clients receive `wait timeout`, not `database exhausted`

**Symptom.** Under coordinator pressure clients see
`PoolError::Timeout(Wait)`, but `pg_doorman_pool_coordinator_total{type="exhaustions"}`
stays at zero. The coordinator never declared exhaustion, but every
client times out.

**Cause.** `query_wait_timeout` is shorter than `reserve_pool_timeout`.
The client gives up before the coordinator's wait phase finishes. The
`exhaustions` counter never increments because the coordinator
eventually gets a permit for a request that no longer has a waiting
client.

**Fix.** Either raise `query_wait_timeout` above `reserve_pool_timeout`
plus typical `connect()` time, or lower `reserve_pool_timeout` (within
the floor noted in the tuning section). The startup config validator
emits a warning for this configuration; act on it.

### PostgreSQL was restarted, what now

**Symptom.** PostgreSQL master restarted (failover, crash, planned).
You see a flash mob of clients hitting the burst gate, `inflight_creates`
sitting at the cap, and `creates_started` rate spiking.

**Cause.** When pg_doorman detects an unusable backend (via
`server_idle_check_timeout` or a failed query), it bumps the pool's
reconnect epoch and drains all idle connections at once. Every client
that arrives after the drain misses the hot path and hits the
anticipation → burst-gate → connect path. With `scaling_max_parallel_creates = 2`,
the pool refills at most 2 connections at a time per pool, gated by
PostgreSQL's `connect()` latency.

**What healthy recovery looks like.** `inflight_creates = 2` continuously
for the first few seconds, `creates_started` rate climbing rapidly,
`burst_gate_waits` rate climbing in lockstep, `anticipation_wakes_notify`
quickly overtaking `anticipation_wakes_timeout` as the new connections
start cycling. Within `pool_size / 2` × `connect()` seconds, the pool
returns to normal.

**Fix.** Usually nothing. The bounded burst gate is doing its job by
preventing a connection storm against a recovering primary. If
`connect()` is genuinely fast (< 50 ms) and your `max_connections`
has headroom, raise `scaling_max_parallel_creates` to 4 or 8 to
shorten recovery, but stay within the hard ceiling from the tuning
section.
