# Global Pool Budget: Weighted Fair Connection Allocation for auth_query

## Problem

Currently, each auth_query user gets an isolated pool with `pool_size` connections to PostgreSQL.
There is **no global limit** on total server connections across all dynamic users.
If PostgreSQL has `max_connections = 100` and 10 users each get `pool_size = 40`,
the pooler can theoretically open 400 connections — far exceeding what PostgreSQL allows.

Additionally, not all users are equal: a production backend service must have higher priority
than an analytics job. Today there is no mechanism to express this — all users compete equally,
and a single noisy neighbor can exhaust the entire connection budget.

**No existing PostgreSQL connection pooler solves this.** PgBouncer, Odyssey, PgCat, and Supavisor
all use hard-partitioned per-user pools with no cross-pool coordination, no weighted allocation,
and no priority-based eviction. ProxySQL (MySQL) is the only proxy with QoS-like features
(delay-based throttling), but it does not implement fair queuing.

## Goals

1. **Global server connection budget** — total connections to PostgreSQL never exceed a configured limit
2. **Weighted fair allocation** — users with higher weight get proportionally more connections under contention
3. **Guaranteed minimum** — each user can reserve a minimum number of connections that are never taken away
4. **Graceful eviction** — when rebalancing is needed, minimize connection churn (open/close cycles are expensive in PostgreSQL: ~2-150ms per fork())
5. **No starvation** — even the lowest-priority user keeps their guaranteed minimum

## Non-goals

- Preempting active queries (cancelling running queries to free connections) — this is too disruptive and should remain an optional, off-by-default feature
- Changing PostgreSQL authentication model — we work within existing auth mechanisms

## Background Research

### How Other Systems Solve Resource Allocation Under Contention

| System | Mechanism | Key Insight |
|--------|-----------|-------------|
| Linux cgroups v2 | `memory.min` / `memory.high` / `memory.max` — three-tier limits with proportional reclaim | Protected reserve + soft pressure + hard cap |
| Kubernetes | QoS classes (Guaranteed/Burstable/BestEffort) + soft/hard eviction thresholds + PriorityClass | Graduated eviction with grace periods |
| VMware balloon driver | Asks guest OS to voluntarily release memory instead of forcibly taking it | Cooperative eviction — let the "owner" release resources naturally |
| Linux OOM killer | `score = (usage / allowed) * 1000 + oom_score_adj` | Scoring formula for victim selection |
| YARN | `total_preemption_per_round = 10%`, natural termination factor = 20% | Rate-limited eviction, accounting for natural turnover |
| TCP AIMD | Additive Increase, Multiplicative Decrease | Proven convergence to fair share |
| Redis | Approximated LRU/LFU with eviction pool of 16 candidates | Sampling-based victim selection |
| HikariCP | SynchronousQueue handoff — returning thread directly gives connection to waiting thread | Skip the pool, hand off directly |

### PostgreSQL Connection Cost (Why Eviction Must Be Gradual)

| Metric | Value |
|--------|-------|
| New connection (localhost, Unix socket) | 2–70 ms |
| New connection (TCP + TLS) | 6–150 ms |
| Idle connection memory (huge_pages=on) | ~1.2 MiB private |
| Idle connection memory (huge_pages=off) | ~4 MiB (incl. PTEs) |
| Catalog cache (fresh) | 512 KB |
| Catalog cache (after heavy use) | can grow to hundreds of MB |
| Postmaster max acceptance rate | ~1,400 conn/sec before saturation |
| TPS loss: 48 active + 10K idle (pre-PG14) | -49.5% |
| TPS loss: 48 active + 10K idle (PG14+) | ~-8% |

**Key constraint**: simultaneous open+close of many connections creates a fork() storm in PostgreSQL,
degrading performance for all users. Eviction rate must be throttled.

Sources:
- Andres Freund, "Measuring the Memory Overhead of a Postgres Connection" (2020)
- Andres Freund / Citus, "Analyzing the Limits of Connection Scalability in Postgres" (2020)
- Recall.ai, "Postgres Postmaster Does Not Scale" (2024)

### Existing Pooler Comparison

| Feature | PgBouncer | Odyssey | PgCat | Supavisor | RDS Proxy |
|---------|-----------|---------|-------|-----------|-----------|
| Global server conn limit | `max_db_connections` (per-DB only) | No | No | Per-tenant | % of max_conn |
| Per-user pool sizing | pool_size per (db,user) | pool_size per route | pool_size per user | Per-tenant only | No |
| Priority / Weight / QoS | **No** | **No** | **No** | **No** | **No** |
| Eviction across users | Oldest-idle only | No | No | Reject at limit | Opaque |
| Queue discipline | FIFO (no priority) | FIFO | FIFO | N/A (reject) | Opaque |

**Gap**: no PostgreSQL pooler implements weighted fair queuing or priority-based eviction.

## Design

### Per-User Configuration (Three Parameters, cgroups/VMware-inspired)

```toml
[pools.mydb.auth_query]
# Global limit for ALL server connections to PostgreSQL for this pool
total_max_connections = 100

[pools.mydb.auth_query.user_defaults]
min_pool_size = 0        # default: best-effort, no guarantees
max_pool_size = 10       # default: each user can use up to 10
weight = 100             # default weight

# Override for specific users
[pools.mydb.auth_query.user_overrides.admin_service]
min_pool_size = 5        # guaranteed: 5 connections always available
max_pool_size = 30       # can grow up to 30
weight = 1000            # 10x priority over default

[pools.mydb.auth_query.user_overrides.analytics_etl]
min_pool_size = 0        # no guarantees
max_pool_size = 5        # hard cap
weight = 10              # low priority
```

**Invariant (checked at config load):** `sum(all users' min_pool_size) <= total_max_connections`.
This ensures all guarantees can be honored simultaneously.

### Three-Zone Model

Each user's connections are classified into three zones:

```
┌──────────┬──────────────────┬──────────────────────┐
│PROTECTED │   FAIR SHARE     │      EXCESS          │
│(0..min)  │  (min..fair)     │   (fair..max)        │
│ never    │  light eviction  │   aggressive          │
│ evicted  │  under pressure  │   eviction            │
└──────────┴──────────────────┴──────────────────────┘
```

**Fair share calculation:**

```
reserved = sum(all users' min)
available = total_max - reserved
fair_share(user) = min + available * (user.weight / total_weight)
fair_share(user) = clamp(fair_share, user.min, user.max)
```

Example with `total_max=100`:
- User A (w=1, min=2, max=40): fair = 2 + 90*(1/6) = 17
- User B (w=5, min=5, max=30): fair = 5 + 90*(5/6) = 80 → capped at 30
- Remaining budget redistributed to others

### Four-Phase Eviction (Soft → Hard)

When the global pool is at capacity and a higher-priority user needs a connection:

#### Phase 0 — WFQ Queue Reordering (instant, zero cost)

When multiple users wait for a connection, serve them by weighted priority, not FIFO:

```
acquisition_priority = weight * (fair_share - held) / fair_share
                     * (1 + wait_time / base_timeout)
```

User with high weight and few connections relative to fair share goes first.
No connections are opened or closed — we just reorder the waiting queue.

#### Phase 1 — Passive Pressure / "Balloon" (instant, zero churn)

When an over-consuming user completes a transaction and returns a connection to the pool:

```
on connection_return(user, connection):
    if user.held > user.fair_share AND higher_priority_user_waiting():
        close(connection)              // don't return to user's pool
        notify(global_waiter_queue)    // waiting user can now create a new connection
    else:
        return_to_pool(user, connection)
```

**Convergence speed** (transaction pooling, OLTP):
- Average transaction: 1–50ms
- 40 connections × ~100 tx/sec = ~4,000 returns/sec
- Freeing 10 connections: **~25–50ms**

This is the **primary mechanism**. Zero connection churn — we simply decline to recycle
the connection back to the over-consumer.

**Limitation**: if the over-consumer runs long queries (60+ seconds), convergence is slow.

#### Phase 2 — Idle Connection Stealing (after grace period, ~100ms per connection)

If Phase 1 hasn't converged within `passive_pressure_grace` (default: 5 seconds):

**Victim selection** (OOM killer-inspired scoring):

```
eviction_score(user) = (held - fair_share) / (max - min)    // how far above fair share
                     * (1 / weight)                           // lower weight = higher score
                     + idle_ratio                             // more idle connections = higher score
```

Scan the highest-scoring user's idle connections (not running queries).
Close the longest-idle connection. The freed PostgreSQL slot allows the waiting user
to create a new connection (~100ms cost).

**Rate limiting** (YARN-inspired): max `total_max * 10%` evictions per rebalance cycle (1 second).
This prevents fork() storms in PostgreSQL.

**Hard invariant**: never evict below `min_pool_size`. Protected zone is sacred.

#### Phase 3 — Adaptive Throttling (ongoing, zero cost)

For users above fair share, add artificial delay to connection acquisition:

```
if user.held > user.fair_share:
    overshoot = (held - fair_share) / (max - fair_share)   // 0.0 .. 1.0
    delay = base_delay * overshoot                          // 0 .. 500ms
    sleep(delay) before granting connection
```

Analogous to Linux cgroup CPU throttling (`cpu.max`): we don't kill, we slow down.
This prevents the over-consumer from immediately reclaiming freed connections.

#### Phase 4 — Forceful Eviction (optional, off by default)

Only when: global pool at 100%, high-priority user waiting beyond `emergency_wait_threshold`,
no idle connections available from over-consuming users.

Target: `idle in transaction` connections exceeding a configurable timeout.
Method: `pg_cancel_backend(pid)` first (preserves connection), `pg_terminate_backend(pid)` as fallback.

**Must be explicitly enabled in config.**

### Rebalancing Controller

To prevent oscillation, the eviction algorithm runs within a control loop:

**AIMD convergence** (TCP-inspired):
- Growth: additive — user can acquire +1 connection per rebalance cycle (1 sec)
- Shrink: multiplicative — drain target = `held - (held - fair) / 2`

**Stabilization window** (Kubernetes HPA-inspired):
- Over the past 30 seconds, use the maximum (most conservative) drain target
- Prevents flapping when load oscillates

**Hysteresis band** (cgroups-inspired):
- Ignore deviations of ±2 connections from fair share
- Prevents micro-adjustments that cause unnecessary churn

**Rate limit** (YARN-inspired):
- Max `total_max * 10%` connection closures per rebalance cycle
- Accounts for natural termination: reduce eviction target by estimated natural turnover

### Dedicated vs Passthrough Mode

The eviction mechanism works differently depending on the auth_query mode:

| Aspect | Dedicated mode | Passthrough mode |
|--------|---------------|-----------------|
| Backend PG user | Same (`server_user`) for all | Different per user |
| Connections fungible? | **Yes** — same PG identity | No — different PG identities |
| Phase 1 (passive) | Redirect idle connection directly to waiting user (**0ms**) | Close + create new (**~100ms**) |
| Phase 2 (idle steal) | Reassign without close/open (**0ms**) | Close + create new (**~100ms**) |
| `RESET ROLE` needed? | Already done on checkin | N/A (connections not shared) |

**In dedicated mode, eviction is nearly free** — idle connections from User A can be
directly handed to User B without closing/reopening, because they share the same PostgreSQL
backend identity. The connection just moves between logical pools.

**In passthrough mode, eviction requires close+open** — the PostgreSQL connection is
authenticated as a specific user and cannot be reassigned. Each eviction costs ~100ms.

### Cost Summary

| Phase | Trigger | Latency | Connection churn | Risk |
|-------|---------|---------|-----------------|------|
| 0: WFQ queue | Instant | 0 | 0 | None |
| 1: Passive pressure | Instant | <100ms (OLTP) | 0 (close only, dedicated: 0) | None |
| 2: Idle steal | After grace period | ~100ms/conn (dedicated: ~0) | 1 close + 1 open per conn | Minimal |
| 3: Throttle | Parallel | Ongoing | 0 | None |
| 4: Force evict | After emergency timeout | Instant | Cancel + close | Client sees error |

### Worked Example

Setup: `total_max=100`, User A (w=1, min=2, max=40, held=40), User B (w=5, min=5, max=30, held=0).
Fair shares: A≈17, B≈30.

**t=0**: User B requests a connection. Global pool = 100/100.
- Phase 0: User B enters WFQ queue with top priority.
- Phase 1: Passive pressure activated on User A.

**t=0..50ms** (OLTP scenario): User A's transactions complete naturally. Returned connections
are closed instead of recycled. User B creates connections as slots free up.

**t=5s** (if User A has long queries): Phase 2 activates. Scan User A's idle connections.
Evict up to 10 per cycle (rate limit). User B creates new connections (~100ms each).

**t=10–30s**: System converges to fair share:
- User A: ~17 connections
- User B: ~30 connections (capped by max)
- User A's min=2 is **always protected**

## Open Questions

1. **Should `user_defaults` apply to statically configured users too, or only auth_query users?**
   Static users already have explicit `pool_size` in config. Global budget could optionally govern them as well.

2. **How to handle config reload when `total_max_connections` changes?**
   Shrinking the budget requires eviction. Growing it allows organic growth.

3. **Should fair share be recalculated when users connect/disconnect?**
   If only 2 of 100 configured users are active, they could share the entire budget.
   Recalculating fair share based on active users provides better utilization.

4. **Metrics and observability.**
   Need to expose: current held/fair/min/max per user, eviction counts by phase,
   queue depth and wait times, global budget utilization.

5. **Integration with `max_concurrent_creates` (existing feature).**
   Today this is per-pool. With a global budget, it should become global to prevent
   fork() storms during rebalancing.
