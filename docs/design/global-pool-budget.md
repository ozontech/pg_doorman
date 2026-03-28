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
4. **Zero eviction** — never cancel active queries, never forcibly close active connections
5. **No starvation** — lowest-priority user keeps their guaranteed minimum
6. **30-second convergence** — adapts to load changes within 30 seconds

## Non-goals

- Eviction of active connections (cancelling running queries, closing active connections)
- Changing PostgreSQL authentication model
- Preemption of any kind

## Background Research

### How Other Systems Solve Resource Allocation Under Contention

| System | Mechanism | Key Insight |
|--------|-----------|-------------|
| Linux cgroups v2 | `memory.min` / `memory.high` / `memory.max` — three-tier limits with proportional reclaim | Protected reserve + soft pressure + hard cap |
| Kubernetes | QoS classes (Guaranteed/Burstable/BestEffort) + soft/hard eviction thresholds + PriorityClass | Graduated eviction with grace periods |
| VMware balloon driver | Asks guest OS to voluntarily release memory instead of forcibly taking it | Cooperative eviction — let the "owner" release resources naturally |
| TCP AIMD | Additive Increase, Multiplicative Decrease | Proven convergence to fair share |
| HikariCP | SynchronousQueue handoff — returning thread directly gives connection to waiting thread | Skip the pool, hand off directly |

### PostgreSQL Connection Cost (Why Eviction Is Unacceptable)

| Metric | Value |
|--------|-------|
| New connection (localhost, Unix socket) | 2–70 ms |
| New connection (TCP + TLS) | 6–150 ms |
| Idle connection memory (huge_pages=on) | ~1.2 MiB private |
| Catalog cache (fresh) | 512 KB |
| Catalog cache (after heavy use) | can grow to hundreds of MB |
| Postmaster max acceptance rate | ~1,400 conn/sec before saturation |

Eviction (close + reopen) costs 2–150 ms per connection and triggers fork() in PostgreSQL.
Simultaneous eviction of N connections creates a fork storm that degrades all users.
This design avoids eviction. Rebalancing relies on natural transaction lifecycle instead.

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
| Queue discipline | FIFO (no priority) | FIFO | FIFO | N/A (reject) | Opaque |

No PostgreSQL pooler implements weighted fair queuing or priority-based scheduling.

---

## Algorithm

### Core Idea

In transaction pooling mode, connections are held only for the duration of a transaction.
When a transaction completes, the connection returns to the pool. The scheduler decides
who gets the next available connection. No connections are forcibly taken away.

Same principle as Linux CFS: processes yield the CPU when their time slice ends,
and the scheduler picks who runs next. No preemption needed for fairness.

### Parameters

```
Global:
  P              — total pool size (fixed, global budget)

Per user (from config or user_defaults):
  w_i            — weight (relative priority, default: 100)
  m_i            — min guaranteed connections (default: 0)
  M_i            — max allowed connections (default: P)

Invariant: sum(m_i for all configured users) <= P
```

### Runtime State

```
Per user i:
  held_i         — connections currently checked out (executing transactions)
  waiting_i      — requests queued waiting for a connection
  demand_i       — held_i + waiting_i (total desired connections)

Global:
  total_held     — sum(held_i) across all users
  idle_count     — P - total_held (connections available in pool)
  quota_i        — current fair share allocation for user i (recalculated dynamically)
```

### Algorithm 1: Quota Calculation (Water-Filling)

Triggered when: a user's demand changes (new request, disconnect), or periodically (every 1 second).

```
fn calculate_quotas(users, P):
    active = [u for u in users if u.demand > 0]
    if active is empty:
        set all quotas to 0
        return

    // Phase 1: everyone starts at their guaranteed minimum
    for u in active:
        quota[u] = u.min

    remaining = P - sum(quota[u] for u in active)

    // Phase 2: distribute remaining by weight (water-filling)
    // Repeat until stable — users hitting max or demand cap free surplus for others
    unsatisfied = set(active)
    loop:
        if remaining <= 0 or unsatisfied is empty:
            break

        total_weight = sum(u.weight for u in unsatisfied)
        any_capped = false

        for u in unsatisfied:
            raw_share = remaining * (u.weight / total_weight)
            effective_cap = min(u.max, u.demand)

            if quota[u] + raw_share >= effective_cap:
                added = effective_cap - quota[u]
                quota[u] = effective_cap
                remaining -= added
                unsatisfied.remove(u)
                any_capped = true
                break  // restart loop with updated remaining

        if not any_capped:
            // No one is capped — distribute proportionally and finish
            total_weight = sum(u.weight for u in unsatisfied)
            for u in unsatisfied:
                share = remaining * (u.weight / total_weight)
                quota[u] += share
            remaining = 0
            break
```

**Example**: P=50, three users all active with high demand:

| User | weight | min | max | Step 1 (min) | Step 2 (water-fill) | Final quota |
|------|--------|-----|-----|:----------:|:-------------------:|:-----------:|
| service_api | 100 | 5 | 40 | 5 | +30.7 = 35.7 → capped 36 | **36** |
| batch_worker | 30 | 2 | 20 | 2 | +9.2 = 11.2 → 11 | **11** |
| analytics | 10 | 0 | 10 | 0 | +3.1 = 3.1 → 3 | **3** |
| **Total** | | **7** | | 7 | | **50** |

### Algorithm 2: On Connection Request

When user U sends a query and needs a server connection:

```
fn on_request(user_U):
    // Case 1: idle connection available AND user is within quota
    if idle_count > 0 AND held[U] < quota[U]:
        grant_connection(U)
        return

    // Case 2: idle connection available BUT user is above quota
    //         AND someone else is below quota
    if idle_count > 0 AND held[U] >= quota[U]:
        below_quota = [V for V in waiting_users if held[V] < quota[V], V != U]
        if below_quota is not empty:
            // U must wait — let underserved users take the idle connection first
            enqueue(U, priority = scheduling_priority(U))
            return

        // No one else is underserved — grant to U (up to max)
        if held[U] < max[U]:
            grant_connection(U)
            return

    // Case 3: no idle connections
    enqueue(U, priority = scheduling_priority(U))
    wait(up to query_wait_timeout)
    if timed_out:
        return error "query_wait_timeout"
```

**Scheduling priority** (determines queue order):

```
fn scheduling_priority(user_U):
    if held[U] < min[U]:
        // Below guaranteed minimum — highest urgency
        return (TIER_0, U.weight, U.wait_time)

    if held[U] < quota[U]:
        // Below fair share — proportional urgency
        deficit_ratio = (quota[U] - held[U]) / quota[U]   // 0.0 .. 1.0
        return (TIER_1, deficit_ratio * U.weight, U.wait_time)

    // At or above quota — lowest urgency
    return (TIER_2, U.weight, U.wait_time)
```

Priority is compared lexicographically: TIER_0 > TIER_1 > TIER_2, then by score descending, then by wait_time descending.

### Algorithm 3: On Connection Return

When user U's transaction completes and the connection returns to the pool:

```
fn on_return(user_U, connection):
    held[U] -= 1

    // Find the best candidate from all waiting users
    best = highest_priority_waiter()

    if best is None:
        // No one waiting — return connection to idle pool
        recycle(connection)
        return

    if best.user == U:
        // U itself is the most deserving waiter — recycle connection for U
        grant_to(best, connection)
        return

    // A different user V has higher priority — redirect the connection

    if dedicated_mode:
        // Connection is fungible (same PG server_user) — hand off directly
        // Cost: 0 (just RESET ROLE, already done on checkin)
        grant_to(best, connection)

    if passthrough_mode:
        // Connection is NOT fungible (different PG users)
        // Close U's connection, let V create a new one
        // Cost: ~100ms (one close + one open, NOT a storm — happens one at a time)
        close(connection)          // free the global slot
        total_held -= 1
        notify(best)               // V can now create a new PG connection
```

No other rebalancing mechanism exists. Connections flow from over-quota to under-quota
users as transactions complete.

### Algorithm 4: Hard Limit Enforcement (Prevent Guarantee Violations)

To ensure user U can never occupy so many connections that another user's minimum becomes unsatisfiable:

```
fn hard_max(user_U, active_users):
    other_mins = sum(V.min for V in active_users if V != U)
    return min(U.max, P - other_mins)
```

Enforced at grant time:

```
fn grant_connection(user_U):
    if held[U] >= hard_max(U, active_users):
        enqueue(U)  // cannot grant — would violate others' guarantees
        return
    // ... proceed with grant
```

**Example**: P=50, service_api (min=5, max=40), batch_worker (min=2, max=20), analytics (min=0, max=10).
- hard_max(service_api) = min(40, 50 - 2 - 0) = 40
- hard_max(batch_worker) = min(20, 50 - 5 - 0) = 20
- hard_max(analytics) = min(10, 50 - 5 - 2) = 10

When only service_api is active:
- hard_max(service_api) = min(40, 50 - 0 - 0) = 40 (can use up to 40)
- The remaining 10 sit idle (service_api.max = 40)

When batch_worker becomes active:
- hard_max(service_api) = min(40, 50 - 2) = 40 (still 40, since batch_worker.min=2 is small)
- batch_worker can grow to 20 as service_api's transactions return connections

---

## Convergence Analysis

### Model

- User A holds H connections, quota is Q (H > Q, A is over-quota by H-Q)
- Average transaction duration: T seconds
- Each of A's connections completes and returns at average rate 1/T
- Combined return rate for all A's connections: H/T per second

### How Convergence Works

When a connection is returned by over-quota user A, the scheduler gives it to under-quota user B
instead of recycling it to A. This reduces A's held count by 1 per return.

A's excess = H - Q connections need to "not be recycled".
The first excess connection returns after ~T/H seconds (any of H connections can be the first).
All excess connections return within ~T seconds (one full transaction cycle).

### Convergence Time by Transaction Duration

| Avg transaction duration | Convergence time | Scenario |
|:------------------------:|:----------------:|----------|
| 1 ms | < 50 ms | Simple key-value lookups |
| 10 ms | < 100 ms | Typical OLTP |
| 100 ms | < 500 ms | Complex OLTP with joins |
| 1 s | ~1–2 s | Reports, aggregations |
| 10 s | ~10–15 s | Heavy analytical queries |
| 30 s | ~30 s | Long-running batch queries |

**For the 30-second convergence target**: the system converges within 30 seconds
as long as the average transaction duration is ≤ 30 seconds. For OLTP workloads
(1–100 ms transactions), convergence is nearly instant.

### What If Transactions Are Longer Than 30 Seconds?

If all of user A's connections are running 60-second queries, the scheduler cannot
rebalance until those queries complete. Active queries are never cancelled.
Convergence happens within max(30 seconds, longest running transaction).

For workloads with very long queries, operators should set appropriate `max_pool_size`
to prevent a single user from occupying the entire pool for extended periods.

---

## Worked Example: Three Users, Step by Step

### Setup

```
P = 50 (total pool budget)

service_api:  weight=100, min=5, max=40
batch_worker: weight=30,  min=2, max=20
analytics:    weight=10,  min=0, max=10

Avg transaction duration: 10ms (OLTP)
```

### Phase 1: Only service_api Active

```
Quotas: service_api = min(5 + 45*100/100, 40) = 40
State:  service_api: held=40, idle=10
```

service_api uses 40 connections. 10 sit idle (service_api is at max).

### Phase 2: batch_worker Comes Online (t=0)

10 clients connect via auth_query as batch_worker.

```
Quota recalculation (both active):
  reserved = 5 + 2 = 7, distributable = 43
  service_api: 5 + 43*(100/130) = 5 + 33 = 38
  batch_worker: 2 + 43*(30/130) = 2 + 10 = 12

State:  service_api: held=40, quota=38 (OVER by 2)
        batch_worker: held=0, quota=12 (UNDER by 12)
        idle=10
```

**t=0 ms**: batch_worker requests 12 connections.
- 10 idle connections available. batch_worker.held < quota. Grant 10 immediately.
- batch_worker: held=10, waiting=2. idle=0.

**t≈10 ms**: service_api returns a connection (transaction completes).
- service_api: held=39 (above quota 38)
- Highest-priority waiter: batch_worker (held=10, quota=12, TIER_1)
- **Grant to batch_worker**, not back to service_api.
- service_api: held=39, batch_worker: held=11

**t≈20 ms**: service_api returns another connection.
- service_api: held=38 (now AT quota)
- Highest-priority waiter: batch_worker (held=11, quota=12, TIER_1)
- **Grant to batch_worker.**
- service_api: held=38, batch_worker: held=12

**t≈20 ms onward**: Steady state reached.
```
service_api: held=38, quota=38  ✓
batch_worker: held=12, quota=12 ✓
Total: 50/50
```

**Convergence time: ~20 ms** (2 transaction completions).

### Phase 3: analytics Comes Online (t=1s)

3 clients connect as analytics.

```
Quota recalculation (all three active):
  reserved = 5 + 2 + 0 = 7, distributable = 43
  service_api: 5 + 43*(100/140) = 5 + 30.7 = 36 (rounded)
  batch_worker: 2 + 43*(30/140) = 2 + 9.2 = 11
  analytics: 0 + 43*(10/140) = 3.1 = 3

State: service_api: held=38, quota=36 (OVER by 2)
       batch_worker: held=12, quota=11 (OVER by 1)
       analytics: held=0, quota=3 (UNDER by 3)
       idle=0
```

**t=1.010 s**: batch_worker returns a connection.
- batch_worker: held=11 (above quota 11? exactly at quota, no — 12-1=11 = quota)
- Wait, batch_worker had 12, returns one, now 11 = quota.
- Highest-priority waiter: analytics (TIER_1, deficit=3/3=1.0, score=10)
- **Grant to analytics.** analytics: held=1.

**t=1.020 s**: service_api returns a connection.
- service_api: held=37 (above quota 36)
- Highest-priority waiter: analytics (held=1, quota=3, TIER_1)
- **Grant to analytics.** analytics: held=2.

**t=1.030 s**: service_api returns another.
- service_api: held=36 (now AT quota)
- Highest-priority waiter: analytics (held=2, quota=3, TIER_1)
- **Grant to analytics.** analytics: held=3.

**t=1.030 s onward**: Steady state.
```
service_api:  held=36, quota=36 ✓
batch_worker: held=11, quota=11 ✓
analytics:    held=3,  quota=3  ✓
Total: 50/50
```

**Convergence time: ~30 ms.**

### Phase 4: analytics Disconnects (t=2s)

All analytics clients disconnect. analytics demand drops to 0.

```
Quota recalculation (service_api + batch_worker):
  reserved = 5 + 2 = 7, distributable = 43
  service_api: 38, batch_worker: 12

State: analytics: held=3, demand=0 (connections still held, draining)
```

As analytics' 3 transactions complete, connections return. analytics.demand=0,
so nobody enqueues for analytics. Returned connections go to the idle pool
(or to service_api/batch_worker if they have waiting requests).

Within ~10 ms, all 3 analytics connections return. System rebalances to:
```
service_api:  held=38, quota=38 ✓
batch_worker: held=12, quota=12 ✓
analytics:    held=0            ✓
Total: 50/50
```

### Phase 5: Burst — service_api Under Pressure (t=3s)

service_api gets a traffic spike: 100 clients all sending queries simultaneously.
service_api.demand jumps to 100, but max=40 and quota=38.

The scheduler grants connections to service_api up to quota (38). The remaining 62 requests
queue and wait for connections to return (transaction complete → immediate re-grant to service_api).
With 38 connections cycling at 10 ms average, service_api processes ~3,800 transactions/sec
despite having only 38 connections.

batch_worker is unaffected — it keeps its 12 connections and continues processing normally.

---

## Dedicated vs Passthrough Mode

### Dedicated Mode (server_user)

All connections are authenticated as the same PostgreSQL user. Connections are **fungible**.

When the scheduler grants User A's returned connection to User B:
- The connection is handed off directly (RESET ROLE already done on checkin)
- Cost: 0 ms. No close, no open, no fork().

### Passthrough Mode (each user authenticates as themselves)

Connections are **NOT fungible** — User A's PG connection is authenticated as User A
and cannot be used by User B.

When User A is over-quota and User B is under-quota:
1. User A's returned connection is closed instead of recycled
2. User B creates a new PG connection using the freed global slot
3. Cost: ~100 ms per rebalanced connection (one close + one open)

One connection at a time, spread across the convergence period. Not a fork storm.

| Property | Dedicated | Passthrough |
|----------|-----------|-------------|
| Rebalance cost per connection | 0 ms | ~100 ms |
| Fork() calls during rebalance | 0 | 1 per migrated connection |
| Connection warm cache preserved | Yes | No (new connection, cold cache) |
| Convergence overhead | None | Minimal (spread over time) |

---

## Configuration

```toml
[pools.mydb.auth_query]
# Global connection budget for this database pool
total_max_connections = 50

# Defaults for all auth_query users (override per user below)
default_weight = 100
default_min_pool_size = 0
default_max_pool_size = 10

# Per-user overrides (matched by username from auth_query result)
[pools.mydb.auth_query.user_overrides.service_api]
weight = 100
min_pool_size = 5
max_pool_size = 40

[pools.mydb.auth_query.user_overrides.batch_worker]
weight = 30
min_pool_size = 2
max_pool_size = 20

[pools.mydb.auth_query.user_overrides.analytics]
weight = 10
min_pool_size = 0
max_pool_size = 10
```

**Validation at config load:**
1. `sum(min_pool_size for all configured users) <= total_max_connections`
2. `each user: min_pool_size <= max_pool_size`
3. `each user: max_pool_size <= total_max_connections`
4. `total_max_connections > 0`

Users not listed in `user_overrides` get `default_*` values.

---

## Open Questions

1. **Dynamic users without overrides.** When a previously unknown user authenticates
   via auth_query, they get default values. If many such users appear, the sum of their
   defaults plus configured overrides must still fit within total_max_connections.
   Option: enforce `default_max_pool_size` such that even worst-case user count fits.
   Option: track active user count and adjust defaults dynamically.

2. **Should fair share be recalculated based on active users only?**
   Current algorithm: yes — only users with demand > 0 participate in quota calculation.
   This means a single active user can use up to their max (not total_max). When a second
   user appears, quotas shift and the system rebalances.

3. **Metrics and observability.** Need to expose per-user: held, quota, min, max, waiting,
   grant rate, wait time p50/p99. Global: total_held, idle_count, rebalance events.

4. **Integration with existing pool architecture.** The global budget sits above individual
   user pools. In dedicated mode, it wraps the single shared pool. In passthrough mode,
   it coordinates across per-user pools. The semaphore in PoolInner needs to be governed
   by the global budget rather than acting independently.
