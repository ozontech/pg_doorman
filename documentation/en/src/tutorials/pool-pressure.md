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
            |  phases 3   |  Phase 4: Notify loop        |
            |  and 4.     |   (bounded by remaining      |
            |  Go straight|    query_wait_timeout        |
            |  to phase 5 |    minus 500 ms reserve)     |
            |  (burst gate|  Then phase 5                |
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
connection is there and passes the recycle check, return it. The recycle
check rolls back any open transaction, runs a liveness probe if the
connection has been idle longer than `server_idle_check_timeout`, and
verifies that the connection's reconnect epoch matches the pool's
current epoch. The pool bumps its reconnect epoch on the `RECONNECT`
admin command and after detected backend failures; connections from
before the bump fail this check and are dropped instead of being
returned. A healthy steady-state pool only takes this path. Cost: a
mutex acquire and the recycle check.

**Phase 2 — Warm zone gate.** If the pool size is below the warm
threshold, skip anticipation and jump straight to creating a new backend
connection. Cold pools fill fast.

**Phase 3 — Anticipation spin.** Above the warm threshold, retry the
recycle 10 times in a tight `yield_now` loop (controlled by
`scaling_fast_retries`). This catches the case where another client
finished its query in the same microsecond range and is about to push
the connection back. Total cost is around 10–50 microseconds. No sleep,
no blocking I/O.

**Phase 4 — Anticipation loop.** If the spin did not catch a return,
enter a loop waiting for returned connections. Each iteration
registers a `Notify` future woken by any `return_object()`, runs
`try_recycle_one` before the wait to catch a buffered return, awaits
either the notify or a per-iteration sleep capped at 100 ms, then
runs `try_recycle_one` again after the wake. If another waiter
popped the returned item first, the iteration bumps a race-loss
counter and registers a fresh future for the next return. The loop
exits on three conditions: a successful recycle (return the
connection), the deadline expired at the top of the iteration, or
the race-loss counter reached `MAX_RACE_LOSSES` (20) — the caller
has woken repeatedly and lost the post-wake race every time, so
anticipation gives up and falls through to the create path.

The deadline is `min(query_wait_timeout - 500 ms, PHASE_4_HARD_CAP)`
where `PHASE_4_HARD_CAP = 500 ms`, measured against a timestamp
captured at the top of `timeout_get`. The hard cap bounds tail
latency under pathological wake orderings where a caller wakes on
every notify but loses every post-wake race — without it, a large
`query_wait_timeout` would let one caller spend tens of seconds in
anticipation. Phase 1/2 semaphore wait consumes from the same budget,
so the cumulative wait across phases cannot exceed the caller's
`query_wait_timeout`.

Each return wakes exactly one waiter. The loop is retry-driven
against races: when `return_object()` fires, both this Phase 4
waiter and any task parked in Phase 1/2 semaphore acquire wake
simultaneously, and the Phase 1/2 task often pops the returned item
first. Under a single-shot wait-then-recycle, every such race loss
forced a fresh `connect()` and grew the pool for no reason. With the
loop, the waiter registers a fresh notify and waits for the next
return. On a busy production shard with ~55% Phase 4 race loss,
`create_fallback` dropped from 24 000 per scrape window to zero and
`creates_started` dropped 90× against the same workload. The pool
stopped paying for connects that returns would have covered.

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
   |  Phase 4:    |  --- recycle  ----> return idle connection
   |  anticipate  |  --- race loss --> wait next return (loop)
   |  Notify loop |  --- deadline ----> fall through
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

### Conditional notify on return

When a connection is returned to the idle queue, `return_object` signals
two channels: `semaphore.add_permits(1)` to wake a Phase 1/2 waiter,
and `idle_returned.notify_one()` to wake a Phase 4 anticipation waiter.

At high concurrency (10k+ clients, pool fully utilized) the idle queue
cycles rapidly and no callers sit in Phase 4 — every checkout hits the
hot path. Calling `notify_one()` on every return in this case wakes a
task that races the semaphore waiter for the same connection, loses, and
wastes ~2-5 us of CPU per return. At 200k returns/sec this adds up to
~10% throughput loss.

To avoid this, `return_object` checks an `idle_returned_listeners`
atomic counter before calling `notify_one()`. Phase 4 anticipation and
the burst gate both increment the counter when they register a
`Notified` future and decrement it when their wait ends. When the
counter is zero (the common case under high throughput), `notify_one()`
is skipped entirely — one atomic load (~3 ns) instead of a notify cycle
(~104 ns) plus a wasted task wake.

Safety net: if a listener registers between the load and a missed
notify, Phase 4's `SLEEP_CAP` (100 ms) and the burst gate's
`BURST_BACKOFF` (5 ms) guarantee progress on the next iteration.

### Pre-replacement for lifetime expiry

When `server_lifetime` is configured, backend connections are closed
after reaching their individual lifetime limit (base ± 20% jitter).
Closing a connection means the pool has one fewer idle backend —
subsequent checkouts may enter the anticipation loop or create path,
adding several milliseconds to p99 during lifetime expiry clusters.

**Pre-replacement** removes this spike. When a checkout recycles a
connection whose age has reached **95%** of its lifetime, a background
task creates a replacement connection and places it in the idle queue.
When the old connection eventually fails recycle at 100% lifetime, the
next checkout finds the pre-created replacement via the hot path —
zero wait.

Up to 3 concurrent pre-replacements may run per pool. During the
overlap window the pool temporarily holds `max_size + 3` connections
and a matching number of extra semaphore permits. When old connections
die, `slots.size` drops back to `max_size`.

Guards that prevent runaway growth:

| Guard | Prevents |
|-------|----------|
| `!under_pressure()` | Creating extras when pool is saturated (old connection would survive via `skip_lifetime` anyway) |
| `idle_ratio < 25%` | Replacing connections in an oversized pool that should shrink |
| `coordinator headroom >= 2` | Stealing the last coordinator permit from a peer pool |
| `lifetime >= 60 s` | Firing on tiny lifetimes where the overlap window is too narrow |
| `slots.size <= max_size + cap` | Stacking multiple pre-replacement overshoots |
| `try_take_burst_slot` (cap=3) | Limiting concurrent background creates |

Pre-replacement only fires on the **checkout** path (`try_recycle_one`),
not from the retain loop. Idle connections that expire without being
checked out are closed by the retain loop **without replacement** — this
is how the pool shrinks naturally when load drops.

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

2. **A reserve pool.** When the cap is reached and `reserve_pool_size`
   has room, the coordinator grants a permit from the **reserve**
   immediately — a small extra pool above `max_db_connections` that
   acts as a burst buffer. This is Phase R (reserve-first) in the
   acquisition flow below: no peer backend is closed, no wait is
   incurred. The reserve is bounded by `reserve_pool_size` (default
   0, meaning disabled) and prioritised: starving users (those below
   their **effective minimum**) and users with many queued clients
   are served first by the arbiter.

3. **Eviction.** Fallback when the reserve is either disabled
   (`reserve_pool_size = 0`) or already fully used: the coordinator
   closes an idle connection from a different user's pool to free a
   main slot. The evicted pool loses a connection; the requesting
   pool gets one. This is fair: users with the largest surplus above
   their **effective minimum** lose connections first, and only
   connections older than `min_connection_lifetime` (default
   30 000 ms) are eligible. The 30-second floor is deliberate: it
   suppresses cyclic reconnect between peer pools that take turns
   stealing slots from each other.

   The **effective minimum** for a user pool is
   `max(user.min_pool_size, pool.min_guaranteed_pool_size)`. Both
   knobs protect connections from eviction; whichever is larger wins.
   Lowering either drops the floor.

### Coordinator acquisition phases

When the per-pool path reaches the new-connection step, the coordinator
walks six phases. The first phase that hands back a permit ends the
sequence.

**Phase A — Try-acquire.** Non-blocking semaphore acquire. If the cap
is not reached, take the slot and return.

**Phase R — Reserve-first.** Phase A proved the database is full.
Before closing any peer backend, the coordinator checks whether the
reserve pool has headroom (`reserve_in_use < reserve_pool_size`). If
yes, it asks the reserve arbiter for a permit directly. On success,
the caller gets a reserve permit — no eviction, no peer backend
closed, no wait on `connection_returned`. The arbiter responds in
sub-millisecond time under normal load.

Reserve-first is the p99-latency path: a reserve permit costs one
arbiter round-trip, while the old flow (Phase B + Phase C) could
block for the full `reserve_pool_timeout` even when the reserve had
empty slots. Phase R does not run when `reserve_pool_size = 0`, and
falls through to Phase B when the arbiter denies the grant (every
reserve permit is already in use, or the arbiter is racing another
caller).

**Phase B — Eviction.** Reached when Phase R did not hand back a
permit: either `reserve_pool_size = 0`, or the reserve semaphore was
fully in use at the check (`reserve_in_use == reserve_pool_size`), or
the arbiter denied the grant. Walk all *other* user pools for the
same database, find the one with the largest surplus above its
**effective minimum**, and close one of its idle connections older
than `min_connection_lifetime`. The evicted permit drops
synchronously, freeing the slot. Re-try the semaphore acquire. If
two callers race, the loser falls through to the next phase.

**Phase C — Wait.** Reached when reserve is disabled or fully in use
*and* Phase B found nothing evictable. Register a `Notify` woken on
two events:

1. A `CoordinatorPermit` was dropped — a peer's server connection was
   physically destroyed (`server_lifetime` expiry, `recycle` error,
   `RECONNECT`), and a semaphore slot is now free.
2. A peer pool returned a connection to its idle queue via
   `Pool::return_object` — the slot is NOT free, but the peer's
   `spare_above_min` may have just grown.

On every wake, Phase C runs `try_acquire` **first** and only calls
`try_evict_one` if the cheap path fails. A permit-drop wake leaves a
free slot in the semaphore — the cheap path takes it and no peer
backend is closed. An idle-return wake does not free a slot directly
but may have grown a peer's `spare_above_min`, so the eviction retry
finds a candidate that was not evictable a moment ago, drops the
peer's permit, and the subsequent `try_acquire` succeeds. This
ordering (cheap first, evict second) is pinned by a regression test
so a future refactor cannot re-introduce peer closes on permit-drop
wakes.

Wait up to `reserve_pool_timeout` (default 3000 ms) for a wake or the
deadline. **This timeout applies even when `reserve_pool_size = 0`**:
it is the wait-phase budget, not just the reserve gating window. If
your `query_wait_timeout` is shorter than `reserve_pool_timeout`, the
client gives up first and you see `wait timeout` errors instead of the
more diagnostic `all server connections to database 'X' are in use`.
See troubleshooting for the symptom.

**Phase D — Reserve retry.** Phase R already tried this path once.
Phase D runs again after Phase C exhausted its wait budget, in case
a peer reserve holder dropped its permit during the wait. Requests
are scored by `(starving, queued_clients)` so users that need
connections most get them first. The arbiter is a single tokio task
that drains reserve permits from a priority heap.

**Phase E — Error.** If Phase D also fails or reserve is not
configured, the client receives an error: `all server connections to
database 'X' are in use (max=N, ...)`.

### Reserve → main upgrade (retain task)

Reserve permits are a burst buffer, not persistent state. Once a
burst passes, the backend that held a reserve permit stays alive and
healthy, but its `CoordinatorPermit` still counts against
`reserve_in_use` — even when `current < max_db_connections` leaves
free slots in the main semaphore. Without active housekeeping,
`SHOW POOL_COORDINATOR` reports a reserve pool that looks occupied
while the real burst capacity is empty, and the next spike has
nowhere to grow.

The retain task runs every `retain_connections_time` (default 30 s)
and performs a book-keeping swap: for each pool not **under
pressure** (see definition below), it walks the idle vec and, for
every backend still holding a reserve permit, tries to steal a main
semaphore permit.

A pool is **under pressure** when its per-pool semaphore has zero
available permits. There is no single column in `SHOW POOLS` that
reports the semaphore state directly, and the observable columns
lag the internal state:

- **Strong proxy:** `sv_active == pool_size`. Every active server
  connection holds a permit, so when every server in the pool is
  active, every permit is taken. This direction is strict.
- **Weak proxy:** `cl_waiting > 0` means at least one client is
  inside `timeout_get`, which *often* means the semaphore is
  empty — but a client that already grabbed a permit and is
  parked in Phase 4 anticipation or coordinator Phase C still
  shows as waiting. Use it as an indicator, not a proof.

The retain task skips pools under pressure for two reasons:
upgrading a reserve permit at that moment hands the slot to the
waiting client (no effect on `reserve_used`), and closing a reserve
connection would force a fresh `connect()` in front of that
client. Cleanup runs on the next cycle. On success, the
reserve permit is released back to the reserve semaphore,
`reserve_in_use` drops by one, and the backend's permit flips from
reserve to main. No reconnect, no peer churn — just two atomic
operations. The walk stops on the first upgrade failure in a pool
because that proves the main semaphore is saturated; no point
checking the rest of the pool's idle vec. The same retain cycle
then runs `close_idle_reserve_connections` to close reserve
backends that could not be upgraded and have been idle longer than
`min_connection_lifetime`.

Under this scheme, `reserve_in_use > 0` means exactly one thing: a
burst is actually in flight *or* finished within the last
`retain_connections_time`. Historical reserve usage converges back
to zero as soon as main has headroom.

### JIT coordinator permits (burst gate first)

Inside the per-pool acquisition flow, the burst gate runs **before**
the coordinator permit is acquired. This is the **JIT (just-in-time)**
ordering: a coordinator permit is taken only when the caller actually
holds a burst gate slot and is about to call `connect()`.

The previous ordering (coordinator first, then gate) caused **phantom
permits**: N callers each acquired a coordinator permit and then queued
behind the burst gate (cap=2). Only 2 callers were actually creating
connections, but the coordinator saw N permits in use and started
issuing reserve permits to peer pools — even though the database was
far from full.

With JIT ordering, at most `max_parallel_creates` callers hold
coordinator permits at any instant. The rest wait for a gate slot
without consuming coordinator budget.

**Head-of-line blocking** is avoided by splitting the coordinator
acquire into a fast and a slow path. The fast path is a non-blocking
`try_acquire()` inside the gate slot — no time is wasted. If it fails,
the caller **releases the gate slot**, waits on the coordinator (may
evict / wait for a peer return), and then re-acquires a gate slot.

```
        Coordinator + plain mode acquisition flow (JIT)
        -----------------------------------------------

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
   Phase 4: anticipation loop  --- HIT ---> return
       | deadline                           |
       v                                    |
       | <----------------------------------+
       v
   Phase 5: bounded burst gate (scaling_max_parallel_creates)
              | slot acquired
              v
   +---------------------------+
   | JIT coordinator acquire   |  only when max_db_connections > 0
   |  fast: try_acquire()      |  non-blocking CAS
   |  slow: release gate slot  |  wait on coordinator (evict/return)
   |        → re-acquire slot  |  then proceed to create
   +------------+--------------+
                | permit granted
                v
   Phase 6: server_pool.create()
                |
                v
                return new connection
```

The phases are numbered identically to plain mode. The coordinator
acquire is **not** a numbered phase: it runs inside the burst gate
slot when `max_db_connections > 0`. In plain mode it does not run.

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

The scaling parameters are global by default, with per-pool overrides
for `scaling_warm_pool_ratio` and `scaling_fast_retries`.
`scaling_max_parallel_creates` is global only; per-pool overrides are
not supported.

| Parameter | Default | Where | What it does |
|---|---|---|---|
| `scaling_warm_pool_ratio` | `20` (percent) | `general`, per-pool | Threshold below which connections are created without anticipation. Below `pool_size × ratio / 100`, every new connection request goes straight to `connect()`. |
| `scaling_fast_retries` | `10` | `general`, per-pool | Number of `yield_now` spin retries before entering the event-driven anticipation loop. Each retry costs ~1–5 µs. |
| `scaling_max_parallel_creates` | `2` | `general` | Hard cap on concurrent backend `connect()` calls per pool. Tasks above the cap wait for an idle return or a peer create completion. Must be `>= 1`. |
| `max_db_connections` | unset (disabled) | per-pool | Cap on total backend connections to a database across all user pools. When unset, the coordinator does not exist. |
| `min_connection_lifetime` | `30000` (ms) | per-pool | Minimum age of an idle connection before the coordinator may evict it for another pool. The 30-second floor suppresses cyclic reconnect between peer pools that keep stealing slots from each other. |
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
Past that ratio the reserve stops behaving like a buffer. If half
your workload lives in the reserve continuously, raise
`max_db_connections` instead of extending the overflow.

`reserve_pool_timeout` is how long a client waits in coordinator phase
C before the reserve is consulted. Default 3000 ms is conservative.
Lower it if your `query_wait_timeout` is short and you would rather
fall through to the reserve fast than block clients on coordinator
wait.

#### Tuning recipe: bring checkout p99 down on a coordinator-managed database

Workload shape: PostgreSQL answers in ~1 ms (p99 query latency is low),
but clients see 100–500 ms p99 checkout latency on a coordinator-managed
pool. The checkout time is coming from the coordinator, not PostgreSQL.

1. Confirm the phase. Run `SHOW POOL_COORDINATOR` during a latency
   spike. Compute `main_used = current - reserve_used` — `current`
   includes reserve permits, and this recipe hinges on whether the
   **main** semaphore alone is full.
   - `main_used == max_db_conn` **and** `exhaustions` not climbing
     → wait-phase dominated. The client spends its budget in
     Phase C before falling into Phase D. Continue to step 2.
   - `main_used < max_db_conn` with no exhaustions → checkout latency
     is not coming from the coordinator. Check `SHOW POOL_SCALING`
     `create_fallback` and the plain-mode troubleshooting section.
2. Enable reserve-first if it is not already. Set
   `reserve_pool_size` to at least `max(2, 0.1 × max_db_connections)`.
   Reserve-first grants a permit in sub-ms when the reserve has
   headroom, so a client that used to sit in Phase C now pays
   one arbiter round-trip.
3. Shorten `reserve_pool_timeout` to `2 × p99 query latency`, never
   lower. For a 1 ms query the floor is typically 20 ms; start at
   50 ms and watch `reserve_acq` and `evictions` for a week.
4. Leave `min_connection_lifetime` at the 30 000 ms default unless
   you specifically want cross-pool rebalancing to react faster;
   lowering it increases eviction rate and connection churn.

What to watch after each change (all in `SHOW POOL_COORDINATOR`):

| Before                          | After                             | Verdict                                                                   |
|---|---|---|
| `reserve_acq` flat              | `reserve_acq` rising              | Reserve-first took over — checkout latency should drop; expected          |
| `evictions` steady              | `evictions` dropping              | Phase B stopped firing because Phase R caught the caller earlier; expected |
| `exhaustions` 0                 | `exhaustions` > 0                 | Over-tightened: `reserve_pool_timeout` is below the true peer-return time |
| `reserve_used` hovers > 0       | `reserve_used` returns to 0 in 30 s | Retain upgrade path is working; no action needed                          |

If checkout p99 does not drop after steps 2–3, the path is not
coordinator-bound. Re-read `SHOW POOL_SCALING` on the affected pool —
`create_fallback` > 0 means the pool itself cannot serve offered load
from returns, and the fix is `pool_size`, not `reserve_pool_size`.

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
| `antic_notify` | counter | Loop iterations where a real `idle_returned` signal woke the waiter. Incremented once per iteration that saw a notify, including iterations that then lost the post-await recycle race and looped back. |
| `antic_timeout` | counter | Loop exits where the sleep timer fired before any notify, or the waiter found the deadline already exhausted at loop entry. Increments exactly once per Phase 4 fall-through. |
| `create_fallback` | counter | Phase 4 exited without a recyclable connection and the caller fell through to `server_pool.create()`. Steady-state should be near zero. A sustained non-zero rate means offered load exceeds what returns can serve within the client's `query_wait_timeout - 500 ms` budget. |
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
| `reserve_used` | gauge | Reserve permits currently in use. Converges back to 0 when main has headroom — the retain task upgrades idle reserve permits to main every `retain_connections_time`. A sustained non-zero value indicates either an active burst or a database continuously pressed to `max_db_connections`. |
| `evictions` | counter | Total times the coordinator evicted an idle connection from a peer pool to free a slot. With reserve-first enabled, this counter only climbs under true cross-pool pressure — when the reserve is full *and* a peer has evictable connections. |
| `reserve_acq` | counter | Total reserve permits granted by the arbiter (Phase R fast path plus Phase D fallback combined) |
| `exhaustions` | counter | Times the coordinator returned an exhausted error to a client. **This is the primary pager signal.** |

#### Reading `SHOW POOL_COORDINATOR` output

Three snapshots and what each one means for the operator:

**Healthy idle database:**
```
 database | max_db_conn | current | reserve_size | reserve_used | evictions | reserve_acq | exhaustions
----------+-------------+---------+--------------+--------------+-----------+-------------+-------------
 mydb     |          80 |      24 |           10 |            0 |         0 |           0 |           0
```
Normal steady state. Plenty of headroom, reserve is dormant, no
evictions, no exhaustions. Alerts must be silent here.

**Post-burst, upgrade in progress:**
```
 database | max_db_conn | current | reserve_size | reserve_used | evictions | reserve_acq | exhaustions
----------+-------------+---------+--------------+--------------+-----------+-------------+-------------
 mydb     |          80 |      65 |           10 |            3 |         0 |          12 |           0
```
A burst consumed most of `max_db_connections` and spilled three
connections into the reserve. `current < max_db_conn` means main
has headroom, so the retain task will upgrade these three permits
to main on its next cycle; `reserve_used` should drop to 0 within
`retain_connections_time` (default 30 s). If it does not, see the
troubleshooting section below. `evictions = 0` and
`reserve_acq > 0` together confirm reserve-first absorbed the
burst without closing peer backends.

**Sustained overload:**
```
 database | max_db_conn | current | reserve_size | reserve_used | evictions | reserve_acq | exhaustions
----------+-------------+---------+--------------+--------------+-----------+-------------+-------------
 mydb     |          80 |      95 |           20 |           15 |       300 |         500 |           0
```
Main is full (`main_used = current - reserve_used = 80`, equal to
`max_db_conn`), reserve is 75% used, evictions are high, and
reserve grants are high. The database is not occasionally pressured
— it is permanently short of capacity and surviving only because
eviction rotates connections between users and reserve-first
absorbs every new arrival. `exhaustions = 0` means the arbiter
still keeps up, but any transient spike tips it over. **Action:**
raise `max_db_connections` after confirming PostgreSQL has
headroom, or find the runaway pool via `SHOW POOLS` and lower its
`pool_size`.

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

Each alert below has a **Runbook** block with one diagnostic
command and two or three branches tied to concrete counter values.

**Coordinator exhaustion (page).** A client received a "database
exhausted" error. Hard failure — reserve and eviction both failed.

```promql
rate(pg_doorman_pool_coordinator_total{type="exhaustions"}[5m]) > 0
```

**Runbook:**
```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOL_COORDINATOR'
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOLS'
```
`current` is the combined main+reserve count
(`current == max_db_conn + reserve_size` means both semaphores are
fully drained).
- `current == max_db_conn + reserve_size` → both semaphores are
  fully drained. Raise `max_db_connections` (verify PostgreSQL
  `max_connections` has headroom first) or add a larger reserve.
- `reserve_size == 0` and `current == max_db_conn` → reserve is
  disabled and main is full. Set `reserve_pool_size` to absorb
  bursts, then raise `max_db_connections` if `exhaustions` keeps
  firing after that.
- `current < max_db_conn + reserve_size` but `exhaustions` climbing
  → race in Phase R/D — should not happen sustained; file a bug
  with the matching `SHOW POOL_COORDINATOR` snapshot.
- One user in `SHOW POOLS` has `sv_idle` much larger than others →
  runaway pool is hoarding connections. Lower that pool's
  `pool_size`, or set `min_guaranteed_pool_size` to protect the
  victims.

**Burst gate saturated (warn).** The burst gate is waiting behind
other creates more often than it proceeds directly. Brief spikes
above the threshold during failover or restart are normal; sustained
values mean `scaling_max_parallel_creates` is too low for offered
load.

```promql
rate(pg_doorman_pool_scaling_total{type="burst_gate_waits"}[5m])
  > 0.5 * rate(pg_doorman_pool_scaling_total{type="creates_started"}[5m])
```

**Runbook:**
```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOL_SCALING'
```
- `inflight_creates` sits at the configured cap AND clients are
  visible in `SHOW POOLS` `cl_waiting` → `connect()` is slow on the
  backend side, see **Burst gate is the bottleneck even with low
  traffic** troubleshooting before raising the cap.
- `inflight_creates` cycles below the cap but `gate_waits` climbs →
  many short bursts. Raise `scaling_max_parallel_creates`, stay
  within the hard ceiling documented under tuning.
- Only one pool is hot → consider `min_guaranteed_pool_size` on the
  neighbours or lower that pool's `pool_size`.

**Create fallback firing (warn).** Phase 4 anticipation is giving up
without finding a return and falls through to a fresh `connect()`.
Steady-state should be zero.

```promql
rate(pg_doorman_pool_scaling_total{type="create_fallback"}[5m]) > 0.1
```

**Runbook:**
```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOL_SCALING'
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
    -c 'SHOW STATS' | grep -E 'database|avg_xact_time|avg_query_time'
```
- `create_fallback` is high on one pool AND `avg_xact_time` on that
  database is growing → slow queries are holding connections out of
  rotation. Fix the slow query first; the pool is sized for normal
  queries, not this transaction length.
- `create_fallback` is high across all pools AND `creates_started`
  rate is also high → offered load exceeds what returns can serve
  within the deadline. Raise `pool_size`.
- `create_fallback` is high but `query_wait_timeout` is short
  (< 1 s) → the anticipation deadline (`query_wait_timeout − 500 ms`
  capped at 500 ms) is too short to catch even normal returns. Raise
  `query_wait_timeout` to at least `2 × p99 query latency`.

**Replenish deferred persistently (warn).** Background replenish
cannot sustain `min_pool_size` because the burst gate is busy with
client traffic.

```promql
increase(pg_doorman_pool_scaling_total{type="replenish_deferred"}[1h]) > 60
```

**Runbook:**
```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOL_SCALING'
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOLS'
```
- The affected pool shows `sv_idle + sv_active < min_pool_size`
  while `gate_waits` is also climbing → replenish is losing to
  client traffic. Raise `scaling_max_parallel_creates` so the
  background task has spare bandwidth, or accept the defer as
  cosmetic (under load, client-driven creates will lift the pool
  above `min_pool_size` anyway).
- `inflight_creates` sits at the cap continuously → gate is full
  for a different reason (slow `connect()`); fix that first.

**Reserve pool continuously in use (warn).** Reserve permit gauge
has not returned to zero over 15 minutes. The retain task upgrades
idle reserve permits back to main every `retain_connections_time`
(default 30 s), so this alert means the upgrade path is *unable* to
run or succeed, not that it forgot to run.

```promql
min_over_time(pg_doorman_pool_coordinator{type="reserve_in_use"}[15m]) > 0
```

**Runbook:**
```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOL_COORDINATOR'
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOLS'
```
Compute `main_used = current - reserve_used` from the row — `current`
is the combined total of main and reserve permits, not main alone.
- `main_used == max_db_conn` → main is fully used; upgrade has no
  slot to steal. The database is undersized; raise
  `max_db_connections`.
- `main_used < max_db_conn` AND every pool in `SHOW POOLS` shows
  `sv_active == pool_size` (or `cl_waiting > 0` as an indicator)
  → every pool is under pressure, retain task skips upgrade.
  Increase `pool_size` on whichever pool has the highest
  `cl_waiting` or the tightest `sv_active / pool_size` ratio.
- `main_used < max_db_conn` AND no pool shows either sign, yet the
  gauge stays non-zero → file a bug with the `SHOW POOL_COORDINATOR`
  and `SHOW POOLS` snapshots; this should not happen.

**Coordinator approaching cap (warn).** Lead time before exhaustion.

```promql
pg_doorman_pool_coordinator{type="max_connections"} > 0
  and
  pg_doorman_pool_coordinator{type="connections"}
    / pg_doorman_pool_coordinator{type="max_connections"} > 0.85
```

**Runbook:**
```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOL_COORDINATOR'
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOLS'
```
- `current` climbing monotonically over hours → capacity planning
  problem. Raise `max_db_connections` (check PostgreSQL headroom
  first) before the next burst.
- `current` oscillating near the cap → burst-driven. Raise
  `reserve_pool_size` so bursts absorb without touching
  `max_db_connections`, and watch `reserve_acq` rate afterward.
- One pool dominates `SHOW POOLS` (`sv_active + sv_idle` much
  larger than peers) → runaway pool; lower its `pool_size` or add
  `min_guaranteed_pool_size` to the victims.

**Inflight stuck at cap (warn).** `inflight_creates` sitting at the
configured cap for 5+ minutes means `connect()` calls are not
finishing.

```promql
min_over_time(pg_doorman_pool_scaling{type="inflight_creates"}[5m])
  >= 2  # adjust to your scaling_max_parallel_creates value
```

**Runbook:**
```bash
time psql -h $PG_HOST -p $PG_PORT -U $PG_USER -d $PG_DB -c 'SELECT 1'
psql -h $PG_HOST -p $PG_PORT -c \
    "SELECT state, count(*) FROM pg_stat_activity GROUP BY state"
```
- `psql` timing shows `connect()` > 500 ms → backend connect is
  slow. Check `pg_stat_ssl` for SSL handshake cost, `pg_authid`
  for role lookup contention, and DNS resolution time from the
  pg_doorman host.
- `pg_stat_activity` shows many `startup` or `authenticating`
  sessions → backend is spawning but not clearing the handshake
  queue. Likely `max_connections` is hit at the backend level —
  run `SELECT setting FROM pg_settings WHERE name = 'max_connections'`
  and compare with actual active sessions.
- `pg_stat_activity` is empty on the pg_doorman-side user →
  network / firewall issue between pg_doorman and PostgreSQL.

**Coordinator thrashing (warn).** Cap is full *and* evictions are
happening: the coordinator is constantly closing peer connections
to make room. The pool is undersized for offered load.

```promql
pg_doorman_pool_coordinator{type="connections"}
    / pg_doorman_pool_coordinator{type="max_connections"} > 0.95
  and
  rate(pg_doorman_pool_coordinator_total{type="evictions"}[5m]) > 0
```

**Runbook:**
```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c 'SHOW POOL_COORDINATOR'
```
- `evictions` rate high AND `reserve_used == 0` → reserve is off
  or exhausted, eviction is the only release valve. Enable /
  raise `reserve_pool_size` to absorb the burst without closing
  peer backends.
- `evictions` AND `reserve_acq` both climbing → reserve is
  consumed and still not enough. Raise `max_db_connections` or
  `reserve_pool_size`; check PostgreSQL `max_connections` first.

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

# Pools where anticipation exhausted its deadline (undersized or slow returns).
# Sorts by the create_fallback share of total creates.
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
     -c 'SHOW POOL_SCALING' --no-align --field-separator='|' \
  | awk -F'|' 'NR>1 && $4>0 { printf "%-20s %-20s %.3f  fallback=%d  creates=%d\n", $1, $2, $8/$4, $8, $4 }' \
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

**First thing to check.** `create_fallback` rate in `SHOW POOL_SCALING`.
If it is above zero and growing, anticipation is exhausting the full
deadline (`query_wait_timeout - 500 ms`) without finding a return.
Clients are paying the wait plus a fresh `connect()` on top of their
query latency.

**Fix.** Two cases.

- **`create_fallback` is growing.** The pool cannot serve offered
  load from returns within the client's wait deadline. Raise
  `pool_size`, raise `query_wait_timeout` (if clients can tolerate
  it), or find the slow queries holding connections out of rotation.
- **`create_fallback` is flat at zero and `antic_notify` is climbing
  in step with pool turnover.** The anticipation loop is working:
  returns are being caught, no connection storm is firing. The
  latency is somewhere else. Check `SHOW STATS avg_wait_time`,
  PostgreSQL-side wait events, network, and client code.

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
   `min_connection_lifetime` (default 30 000 ms), or every other pool
   is at its `min_guaranteed_pool_size`. Lower `min_connection_lifetime`
   if your workload has very short queries and you explicitly want
   faster cross-pool rebalancing, or increase `max_db_connections`.
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

**Cause.** Phase C wait is consistently timing out. With reserve-first
enabled, reaching Phase C means the reserve was already full when the
caller arrived, so a peer return is the only way out. Either the
database is genuinely at the cap with no connections returning, or
`reserve_pool_size = 0` so the wait runs to completion before the
client receives any response.

**Fix.** Lower `reserve_pool_timeout` to fail fast, or set
`reserve_pool_size > 0` so Phase R / Phase D handles the overflow
within the same acquisition path without parking in Phase C at all.

### `reserve_used` stays non-zero but the pool looks idle

**Symptom.** `SHOW POOL_COORDINATOR` shows `reserve_used = 4` (or
any non-zero number) while `SHOW POOLS` shows no `cl_waiting`, low
`cl_active`, and `current < max_db_conn`. The reserve pool looks
occupied by "ghosts".

**Cause.** On builds before the reserve→main upgrade, a reserve
permit stayed attached to its backend until the backend aged out
past `min_connection_lifetime` *and* the retain cycle caught it
idle. Under steady client traffic, `last_used()` on the backend
kept refreshing faster than `min_connection_lifetime`, so the
permit was never released.

**Fix.** On current builds this is resolved automatically: the
retain task runs `upgrade_reserve_to_main` every
`retain_connections_time` (default 30 s). Each reserve backend in
a pool not under pressure gets its permit swapped for a main permit
as long as `db_semaphore` has headroom. Watch the `reserve_used`
gauge drop to zero within one retain cycle.

If `reserve_used` still sticks, the pool is either under sustained
pressure (`under_pressure() == true` skips upgrade, which is correct
— a queued client would re-grab the slot immediately) or
`current == max_db_connections` (no main slot to steal into).
Either condition means the database is genuinely full; the fix is
more capacity, not a workaround.

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
Each client request from that user hits Phase R (reserve-first) as
soon as the database is full and grabs a reserve permit — the
arbiter scores starving users highest, so they win the grant. **The
deeper question is why the user keeps needing fresh connections**:
either its `pool_size` is too low to absorb its own load, or its
traffic is bursty and the reserve is doing what reserves are for.

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
climbing as the first refilled connections start cycling back and the
loop recovers race losses. `create_fallback` should stay flat: the
deadline window is wide enough that the loop catches returns before
giving up. Within `pool_size / 2` × `connect()` seconds, the pool
returns to normal.

**Fix.** Usually nothing. The bounded burst gate is doing its job by
preventing a connection storm against a recovering primary. If
`connect()` is genuinely fast (< 50 ms) and your `max_connections`
has headroom, raise `scaling_max_parallel_creates` to 4 or 8 to
shorten recovery, but stay within the hard ceiling from the tuning
section.

## Glossary

- **`bounded burst gate`** — per-pool limiter capped at
  `scaling_max_parallel_creates` concurrent backend `connect()` calls.
  Tasks beyond the cap wait on a `Notify` until a slot frees up.
- **`CoordinatorPermit`** — RAII guard that accounts for one coordinator
  slot. Carries an `is_reserve` flag. Dropped when the backend is
  physically destroyed (not when it returns to the idle vec), at which
  point it releases its slot back to either `db_semaphore` (main) or
  `reserve_semaphore` (reserve).
- **effective minimum** — the eviction floor for a user pool, computed
  as `max(user.min_pool_size, pool.min_guaranteed_pool_size)`. The
  coordinator protects this many connections per user from being
  evicted by peers.
- **`MAX_RACE_LOSSES`** — compile-time constant (20). The Phase 4
  anticipation loop gives up after this many consecutive failed
  post-wake recycles. Not configurable.
- **Phase R (reserve-first)** — coordinator shortcut inserted between
  Phase A and Phase B. When the database is full but the reserve pool
  has headroom, Phase R grants a reserve permit directly via the
  arbiter instead of closing a peer backend or parking in Phase C.
- **`PHASE_4_HARD_CAP`** — compile-time constant (500 ms). Upper
  bound on Phase 4 anticipation wall time, regardless of
  `query_wait_timeout`. Protects tail latency under pathological
  wake orderings. Not configurable.
- **reserve arbiter** — single tokio task that owns the reserve
  permits. Reserve requests are scored by `(starving, queued_clients)`
  and drained from a priority heap so the neediest users are served
  first.
- **reserve → main upgrade** — retain-time book-keeping swap. When
  an idle backend holds a reserve permit and `db_semaphore` has
  headroom, the retain task steals a main permit, returns the reserve
  slot, and flips `is_reserve` on the permit. No reconnect.
- **`spare_above_min`** — `slots.size - effective_minimum` for a user
  pool, where `slots.size` is the pool's currently allocated
  connection count (active + idle together, not just idle). Used by
  the coordinator to pick eviction victims: the user pool with the
  largest `spare_above_min` loses a connection first. The underlying
  connection still has to be idle in the vec to be eligible for
  eviction — `spare_above_min` only selects the pool, not the
  specific connection.
- **`starving` user** — a user pool whose current connection count is
  below its effective minimum. The reserve arbiter gives starving
  users absolute priority over non-starving users.
- **`under_pressure()`** — predicate that returns `true` when a pool's
  per-pool semaphore has zero available permits, equivalent to every
  slot being checked out right now. Used by the retain task to skip
  upgrade/close on pools that would just hand the freed slot to a
  waiting client.
- **warm threshold** — `pool_size × scaling_warm_pool_ratio / 100`.
  Below this size, the pool skips anticipation and goes straight to
  `connect()`. Above it, anticipation is active and the pool tries to
  catch returns before creating new backends.
