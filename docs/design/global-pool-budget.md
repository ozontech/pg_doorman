# Global Pool Budget: Weighted Connection Allocation for auth_query

## Problem

Each auth_query user gets an isolated pool with no global limit on total server connections.
If PostgreSQL has `max_connections = 100` and 10 users each get `pool_size = 40`,
the pooler can attempt 400 connections. No existing PostgreSQL pooler (PgBouncer, Odyssey,
PgCat, Supavisor) solves this with weighted allocation.

## Parameters

```
Global:
  P                  — max_db_connections (hard limit on total PG connections)
  min_lifetime       — min_connection_lifetime (default: 30s)
                       connection cannot be evicted before this age

Per user:
  guaranteed         — guaranteed_pool_size (always available, opens immediately)
  weight             — priority when competing for above-guarantee connections
  max                — max_pool_size (per-user hard cap)

Invariant: sum(guaranteed for all configured users) <= P
```

## State

```
Per user U:
  held[U]            — server connections currently assigned to U
  waiting[U]         — queued requests from U waiting for a connection

Per server connection C:
  C.user             — which user owns this connection
  C.created_at       — when the PG backend was created (fork timestamp)

Derived:
  total_held         = sum(held[U] for all users)
  above_guarantee[U] = max(0, held[U] - guaranteed[U])
```

## Formulas

**Waiter priority** (who gets the next available connection):

```
priority(U) = (is_guaranteed(U), weight[U], waiting[U])
              compared lexicographically, descending

where is_guaranteed(U) = (held[U] < guaranteed[U])
```

Guaranteed requests always win. Among above-guarantee waiters: highest weight wins.
Equal weight: user with more queued requests wins (higher pressure).

**Eviction eligibility** (can connection C be evicted for requester R?):

```
evictable(C, R) =
    held[C.user] > guaranteed[C.user]          // C is above guarantee
    AND now() - C.created_at >= min_lifetime   // C is old enough
    AND (is_guaranteed(R)                       // R is guaranteed (beats everything)
         OR weight[C.user] < weight[R])         // OR R has higher weight
```

**Eviction order** (which connection to evict first):

```
eviction_score(C) = (weight[C.user] ASC, age(C) DESC)
```

Evict from the lowest-weight user first. Among equal weight: evict the oldest connection.

---

## Algorithm

Three events drive the system:

### Event 1: REQUEST — User U needs a connection

```
                          REQUEST(U)
                              │
                 ┌────────────┴────────────┐
                 │ held[U] < guaranteed[U]? │
                 └────────────┬────────────┘
                      yes     │      no
                      ▼       │      ▼
               ┌──────────┐   │  ┌──────────────────┐
               │ IMMEDIATE │   │  │ held[U] < max[U]? │
               │ (see      │   │  └────────┬─────────┘
               │  below)   │   │    yes    │     no
               └──────────┘   │    ▼       │     ▼
                              │ ENQUEUE(U) │  ERROR
                              │ SCHEDULE() │  "user at max"
                              │            │
                              └────────────┘
```

**IMMEDIATE (guaranteed request):**

```
┌──────────────────┐    yes    ┌───────────────┐
│ idle available?  ├──────────►│ GRANT(U, idle)│
└────────┬─────────┘           └───────────────┘
         │ no
         ▼
┌──────────────────┐    yes    ┌───────────────┐
│ total_held < P?  ├──────────►│ CREATE(U)     │
└────────┬─────────┘           └───────────────┘
         │ no
         ▼
┌──────────────────┐  found    ┌───────────────┐
│ FIND_EVICTABLE() ├──────────►│ EVICT(victim) │
│ (weight = ∞)     │           │ CREATE(U)     │
└────────┬─────────┘           └───────────────┘
         │ not found (all too young)
         ▼
┌──────────────────────────────┐
│ ENQUEUE(U) — wait until      │
│ a connection ages past       │
│ min_lifetime, then retry     │
└──────────────────────────────┘
```

### Event 2: RETURN — User U finishes a transaction

```
                     RETURN(U, connection)
                              │
                              ▼
                      held[U] -= 1
                              │
                              ▼
                         SCHEDULE()
```

The returned connection goes to the idle pool. SCHEDULE() decides who gets it.

### Event 3: SCHEDULE — Assign available connections to waiters

```
                         SCHEDULE()
                              │
                 ┌────────────┴────────────┐
                 │ any waiters?            │
                 └────────────┬────────────┘
                       no     │     yes
                       ▼      │      ▼
                    (done)    │  best = SELECT_BEST_WAITER()
                              │      │
                 ┌────────────┴──────┴──────────┐
                 │                               │
                 ▼                               ▼
       ┌─────────────────┐            ┌─────────────────┐
       │ idle available   │            │ total_held < P   │
       │ OR total_held<P? │            │ (but no idle)?   │
       └────────┬────────┘            └────────┬────────┘
           yes  │                          yes │
                ▼                              ▼
       ┌─────────────────┐            ┌─────────────────┐
       │ GRANT(best)     │            │ CREATE(best)    │
       └─────────────────┘            └─────────────────┘

       If total_held = P and no idle:
       ┌─────────────────┐  found     ┌─────────────────┐
       │ FIND_EVICTABLE  ├───────────►│ EVICT(victim)   │
       │ (weight = best)  │           │ CREATE(best)    │
       └────────┬────────┘            └─────────────────┘
                │ not found
                ▼
       (best stays in queue, retry on next RETURN)
```

### Helper: SELECT_BEST_WAITER

```
fn select_best_waiter():
    // Guaranteed waiters first, then by weight, then by waiting count
    return waiters.max_by(|W|
        (held[W] < guaranteed[W],     // true > false (guaranteed first)
         weight[W],                    // higher weight wins
         waiting[W])                   // more queued requests wins (higher pressure)
    )
```

### Helper: FIND_EVICTABLE

```
fn find_evictable(requester_weight):
    candidates = []
    for each connection C assigned to any user:
        if held[C.user] <= guaranteed[C.user]:   continue  // within guarantee: sacred
        if age(C) < min_lifetime:                 continue  // too young: protected
        if requester_weight != ∞                            // not a guaranteed request
           AND weight[C.user] >= requester_weight: continue // same/higher weight: safe
        candidates.push(C)

    if candidates.is_empty(): return None

    // Pick victim: lowest weight first, oldest connection first
    return candidates.min_by(|C| (weight[C.user], -(age(C))))
```

---

## Behavior Diagrams

### Setup

```
P = 20 (max_db_connections)
min_lifetime = 30s

service_api:  guaranteed=5, weight=100, max=15
batch_worker: guaranteed=3, weight=50,  max=10
analytics:    guaranteed=0, weight=10,  max=5
```

### Scenario 1: Normal startup

```
t=0s    All users start. Pool empty.

        service_api requests 8 connections:
          5 within guarantee → CREATE immediately (held=5)
          3 above guarantee → ENQUEUE, SCHEDULE:
            no other waiters → CREATE immediately (held=8)
        total_held = 8

        batch_worker requests 5 connections:
          3 within guarantee → CREATE immediately (held=3)
          2 above guarantee → ENQUEUE, SCHEDULE:
            no other waiters → CREATE immediately (held=5)
        total_held = 13

        analytics requests 3 connections:
          0 within guarantee (guaranteed=0)
          3 above guarantee → ENQUEUE, SCHEDULE:
            no other waiters → CREATE immediately (held=3)
        total_held = 16

        Final state:
        ┌──────────────┬──────┬────────────┬───────────────┐
        │ User         │ held │ guaranteed │ above-guarantee│
        ├──────────────┼──────┼────────────┼───────────────┤
        │ service_api  │    8 │          5 │             3 │
        │ batch_worker │    5 │          3 │             2 │
        │ analytics    │    3 │          0 │             3 │
        ├──────────────┼──────┼────────────┼───────────────┤
        │ total        │   16 │         8  │             8 │
        └──────────────┴──────┴────────────┴───────────────┘
        Pool: 16/20. 4 slots free.
```

### Scenario 2: Pool fills up, weight competition

```
t=1s    service_api requests 4 more connections (wants 12 total).
        All above guarantee. ENQUEUE, SCHEDULE:
          total_held=16, P=20 → room → CREATE 4.
          service_api: held=12. total_held=20. POOL FULL.

t=1s    analytics requests 2 more connections (wants 5 total).
        Above guarantee. ENQUEUE, SCHEDULE:
          total_held=20 = P → pool full.
          FIND_EVICTABLE(weight=10):
            Scan above-guarantee connections:
              service_api has 7 above-guarantee, weight=100 > 10 → NOT evictable
              batch_worker has 2 above-guarantee, weight=50 > 10 → NOT evictable
              analytics has 3 above-guarantee, weight=10 = 10 → NOT evictable (not <)
            No victims found.
          analytics stays in queue. Waits for natural returns.

        State:
        ┌──────────────┬──────┬─────────┬─────────┐
        │ User         │ held │ waiting │ above-g │
        ├──────────────┼──────┼─────────┼─────────┤
        │ service_api  │   12 │       0 │       7 │
        │ batch_worker │    5 │       0 │       2 │
        │ analytics    │    3 │       2 │       3 │
        └──────────────┴──────┴─────────┴─────────┘
        Pool: 20/20. analytics waiting.
```

### Scenario 3: Transaction returns, weight decides

```
t=1.01s batch_worker finishes a transaction. RETURN(batch_worker, conn).
        batch_worker: held=4. total_held=19.
        SCHEDULE():
          Waiters: analytics (weight=10, waiting=2, above-guarantee)
          No guaranteed waiters.
          idle=1, total_held=19 < P=20.
          → GRANT(analytics). analytics: held=4, waiting=1.
          total_held=20.

        SCHEDULE() again for 2nd analytics waiter:
          total_held=20 = P. Pool full.
          FIND_EVICTABLE(weight=10): no victims (all same or higher weight).
          analytics stays in queue.

t=1.02s service_api finishes a transaction. RETURN(service_api, conn).
        service_api: held=11. total_held=19.
        SCHEDULE():
          Waiters: analytics (weight=10, waiting=1, above-guarantee)
          → GRANT(analytics). analytics: held=5=max. waiting=0.
          total_held=20.
```

### Scenario 4: High-weight user arrives, evicts low-weight

```
t=35s   (All connections are now >30s old, past min_lifetime)

        service_api requests 3 more connections (wants 15=max).
        Above guarantee. ENQUEUE, SCHEDULE:
          total_held=20 = P. Pool full.
          FIND_EVICTABLE(weight=100):
            analytics: 5 above-guarantee, weight=10 < 100, age=34s > 30s → EVICTABLE
            batch_worker: 1 above-guarantee, weight=50 < 100, age=34s > 30s → EVICTABLE
            Pick lowest weight first: analytics (weight=10).
            Pick oldest connection: analytics conn from t=1.01s.
          EVICT(analytics oldest conn). analytics: held=4.
          CREATE(service_api). service_api: held=12.

        SCHEDULE() for 2nd service_api request:
          FIND_EVICTABLE(weight=100):
            analytics: 4 above-guarantee, weight=10 → evictable
          EVICT(analytics). analytics: held=3.
          CREATE(service_api). service_api: held=13.

        SCHEDULE() for 3rd service_api request:
          FIND_EVICTABLE(weight=100):
            analytics: 3 above-guarantee, weight=10 → evictable
          EVICT(analytics). analytics: held=2.
          CREATE(service_api). service_api: held=14.

        State after evictions:
        ┌──────────────┬──────┬─────────┬───────────────────┐
        │ User         │ held │ above-g │ evicted from      │
        ├──────────────┼──────┼─────────┼───────────────────┤
        │ service_api  │   14 │       9 │                   │
        │ batch_worker │    4 │       1 │                   │
        │ analytics    │    2 │       2 │ 3 conns evicted   │
        └──────────────┴──────┴─────────┴───────────────────┘

        analytics lost 3 connections to service_api because:
        weight(analytics)=10 < weight(service_api)=100
        AND all connections were older than min_lifetime=30s.
```

### Scenario 5: Guaranteed request evicts from any weight

```
t=40s   New user "admin" configured with guaranteed=2, weight=1, max=2.
        admin requests 2 connections. Both within guarantee.

        IMMEDIATE: total_held=20=P. Pool full.
        FIND_EVICTABLE(weight=∞):  // guaranteed request beats any weight
          analytics: 2 above-guarantee, weight=10, age>30s → evictable
          → EVICT(analytics). analytics: held=1. CREATE(admin).
          → EVICT(analytics). analytics: held=0. CREATE(admin).

        admin: held=2. Even though admin has weight=1 (lowest),
        guaranteed requests evict above-guarantee connections regardless of weight.

        ┌──────────────┬──────┬─────────┬──────────────────────┐
        │ User         │ held │ above-g │ note                 │
        ├──────────────┼──────┼─────────┼──────────────────────┤
        │ service_api  │   14 │       9 │                      │
        │ batch_worker │    4 │       1 │                      │
        │ analytics    │    0 │       0 │ fully evicted         │
        │ admin        │    2 │       0 │ guarantee honored     │
        └──────────────┴──────┴─────────┴──────────────────────┘
```

### Scenario 6: Flap protection (min_lifetime prevents oscillation)

```
t=40s   analytics has 0 connections. Requests 3.
        Above guarantee (guaranteed=0). ENQUEUE.
        SCHEDULE: total_held=20=P.
        FIND_EVICTABLE(weight=10): no victims with lower weight.
        analytics waits.

t=40.01s service_api finishes a transaction. RETURN. held=13. total_held=19.
        SCHEDULE: analytics waiting (weight=10).
        total_held=19 < P → CREATE(analytics). analytics: held=1.

t=40.05s Two more service_api returns free slots.
        analytics: held=3. All three connections are FRESH (age < 1s).

t=45s   service_api requests 3 more connections.
        FIND_EVICTABLE(weight=100):
          analytics: 3 above-guarantee, weight=10 < 100
          BUT age = 5s < min_lifetime = 30s → PROTECTED!
          No evictable connections.
        service_api stays in queue.

        ╔══════════════════════════════════════════════════════╗
        ║ Flap protection: analytics connections are too young ║
        ║ to evict. service_api must wait for natural returns  ║
        ║ or until analytics connections reach 30s age.        ║
        ╚══════════════════════════════════════════════════════╝

t=70s   analytics connections are now 30s old. min_lifetime reached.
        If service_api is still waiting:
          FIND_EVICTABLE(weight=100): analytics now evictable.
          Eviction proceeds.
```

---

## Configuration

```toml
[pools.mydb.auth_query]
# Hard limit on total server connections to PostgreSQL
max_db_connections = 50

# Flap protection: minimum age before a connection can be evicted
min_connection_lifetime = 30000   # ms, default 30s

# Defaults for auth_query users
default_guaranteed_pool_size = 0
default_weight = 100
default_max_pool_size = 5

# Per-user overrides
[pools.mydb.auth_query.user_overrides.service_api]
guaranteed_pool_size = 5
weight = 100
max_pool_size = 40

[pools.mydb.auth_query.user_overrides.batch_worker]
guaranteed_pool_size = 3
weight = 50
max_pool_size = 20

[pools.mydb.auth_query.user_overrides.analytics]
guaranteed_pool_size = 0
weight = 10
max_pool_size = 10
```

**Validation:**
1. `sum(guaranteed_pool_size for all configured users) <= max_db_connections`
2. `each user: guaranteed_pool_size <= max_pool_size`
3. `each user: max_pool_size <= max_db_connections`
4. `min_connection_lifetime > 0`

---

## Dedicated vs Passthrough

**Dedicated mode** (all users share one PG server_user): connections are fungible.
EVICT = reassign to another user (RESET ROLE already done on checkin). Cost: 0 ms.

**Passthrough mode** (each user authenticates as themselves): connections are not fungible.
EVICT = close old connection + open new one. Cost: ~100 ms (one fork in PostgreSQL).

The algorithm is identical in both modes. Only the cost of eviction differs.
