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

**Recommendation:** `sum(guaranteed) <= P * 0.8`. Reserve at least 20% of the budget
for above-guarantee competition. If `sum(guaranteed) = P`, users with `guaranteed=0`
can never get connections.

---

## Edge Cases

### Who can evict whom (above-guarantee only)

```
Requester →          service_api  batch_worker  analytics
Victim ↓               (w=100)      (w=50)       (w=10)
──────────────────────────────────────────────────────────
service_api (w=100)      —            ❌           ❌
batch_worker (w=50)      ✅            —           ❌
analytics (w=10)         ✅            ✅            —
──────────────────────────────────────────────────────────
✅ = can evict (victim weight < requester weight AND age >= min_lifetime)
❌ = cannot (victim weight >= requester weight)

Guaranteed requests (held < guaranteed) evict ANY above-guarantee
connection regardless of weight (treated as weight = ∞).
```

### EC-1: New user with guaranteed=0, pool full, equal weight

```
State: total_held=20=P. All connections belong to users with weight=100.

new_app (guaranteed=0, weight=100, max=5) requests a connection.
  FIND_EVICTABLE(weight=100):
    All above-guarantee connections have weight=100. 100 < 100? NO.
    No victims.

  new_app enters queue. Gets a connection on the next RETURN.
  SELECT_BEST_WAITER: new_app (weight=100, waiting=1) competes with
  the returning user (if they also have waiting requests).
  Tie-breaker: waiting count.
```

### EC-2: New user with guaranteed=0, pool full, lower weight than all

```
new_app (guaranteed=0, weight=5) requests a connection.
  Pool full. FIND_EVICTABLE(weight=5): no one has weight < 5.
  new_app enters queue.

  On RETURN from any user:
    SELECT_BEST_WAITER among all waiters.
    If service_api (weight=100) is also waiting → service_api wins.
    new_app gets a connection only when NO higher-weight user is waiting.
```

### EC-3: New user with guaranteed=2, pool full

```
State: total_held=20=P.
  service_api: held=12 (7 above-guarantee), weight=100
  batch_worker: held=5 (2 above-guarantee), weight=50
  analytics: held=3 (3 above-guarantee), weight=10

new_service (guaranteed=2, weight=80) requests first connection.
  held=0 < guaranteed=2 → IMMEDIATE.
  Pool full → FIND_EVICTABLE(weight=∞):
    All 12 above-guarantee connections are candidates.
    Lowest weight first: analytics (weight=10).
    Age >= min_lifetime? If YES → EVICT(analytics). CREATE(new_service).
    If NO (all connections < 30s old) → new_service waits.

  After second EVICT: new_service has guaranteed=2, held=2. Guarantee met.
```

### EC-4: All connections within guarantee, no above-guarantee to evict

```
P=8. service_api(guaranteed=5, held=5). batch_worker(guaranteed=3, held=3).
total_held=8=P. All within guarantee.

analytics(guaranteed=0, weight=10) requests a connection.
  Pool full. FIND_EVICTABLE: no above-guarantee connections exist.
  analytics enters queue.

  On RETURN(service_api): service_api held=4 < guaranteed=5.
    SELECT_BEST_WAITER:
      service_api: is_guaranteed=true (held=4 < guaranteed=5)
      analytics: is_guaranteed=false (held=0, but guaranteed=0)
    service_api wins (guaranteed > above-guarantee).
    Connection returns to service_api.

  ⚠ analytics NEVER gets a connection in this configuration.
  This is correct behavior: sum(guaranteed)=8=P leaves no room.
```

### EC-5: Many dynamic users with default guaranteed=0

```
P=20, 50 users via auth_query, all: guaranteed=0, weight=100, max=5.

First 4 users get 5 connections each = 20. POOL FULL.
Users 5-50 enter queue.

On each RETURN: SELECT_BEST_WAITER among all 46 waiters.
  All weight=100, all guaranteed=false.
  Tie-breaker: waiting count. User with most pending requests wins.

With avg transaction=10ms and 20 connections:
  ~2000 returns/sec → ~2000 grants/sec to waiters.
  All 50 users share 20 connections in round-robin fashion.
  Effective: 0.4 connections per user on average.
```

### EC-6: Dynamic users overflow guaranteed budget

```
default_guaranteed_pool_size = 1, P = 20
Static users: guaranteed sum = 8
Dynamic users: 15 connect → 15 × 1 = 15
Total guaranteed: 8 + 15 = 23 > P = 20. INVARIANT VIOLATED.

Solution: runtime check on each new dynamic user:

  fn can_grant_guarantee(new_user):
      current = sum(guaranteed[U] for U in active_users)
      return current + new_user.default_guaranteed <= P

  If false: new_user gets guaranteed=0 (no guarantee, competes by weight).
```

### EC-7: min_lifetime=0 (disabled protection)

```
t=0.0s  analytics gets 5 connections
t=0.1s  service_api requests → evicts analytics (weight 100 > 10)
t=0.2s  service_api load drops → analytics gets connections back
t=0.3s  service_api requests again → evicts analytics again
...
Each cycle: ~100ms, one fork() in PostgreSQL.
10 cycles/sec × fork() = postmaster degradation.

⚠ min_lifetime=0 causes connection flapping. Not recommended.
  Minimum recommended: 5s. Default: 30s.
```

### EC-8: Guaranteed user temporarily over-guaranteed, then load shifts

```
service_api: guaranteed=5, held=12 (7 above-guarantee).
All 7 above-guarantee connections are 45s old (past min_lifetime).

batch_worker requests 5 connections (within guarantee: held=0 < guaranteed=3).
  IMMEDIATE: FIND_EVICTABLE(weight=∞):
    service_api: 7 above-guarantee, age=45s > 30s → all evictable
    EVICT 3 (for batch_worker guaranteed). Then 2 more above-guarantee.
    service_api: held=7 (2 above-guarantee).
    batch_worker: held=5 (2 above-guarantee).

  service_api keeps its 5 guaranteed connections untouched.
  Only above-guarantee connections were evicted.
```

### Summary table

| # | Situation | Outcome | Notes |
|---|-----------|---------|-------|
| EC-1 | guaranteed=0, pool full, equal weight | Waits for RETURN | Tie-break by waiting count |
| EC-2 | guaranteed=0, pool full, lowest weight | Waits indefinitely | Gets conn only when no higher-weight waiter |
| EC-3 | guaranteed>0, pool full | Evicts lowest-weight above-guarantee | weight=∞ for guaranteed |
| EC-4 | sum(guaranteed)=P, no above-guarantee | Never gets connection | Configure sum(g) ≤ 80% P |
| EC-5 | 50 dynamic users, guaranteed=0 | Round-robin on P connections | Expected behavior |
| EC-6 | Dynamic users overflow guarantee budget | Runtime check, degrade to guaranteed=0 | Prevents invariant violation |
| EC-7 | min_lifetime=0 | Flap/fork storm | Not recommended, minimum 5s |
| EC-8 | Guaranteed evicts above-guarantee | Only above-guarantee affected | Guaranteed connections sacred |

---

## Dedicated vs Passthrough

**Dedicated mode** (all users share one PG server_user): connections are fungible.
EVICT = reassign to another user (RESET ROLE already done on checkin). Cost: 0 ms.

**Passthrough mode** (each user authenticates as themselves): connections are not fungible.
EVICT = close old connection + open new one. Cost: ~100 ms (one fork in PostgreSQL).

The algorithm is identical in both modes. Only the cost of eviction differs.

---

## Integration Contracts

Requirements for wiring BudgetController into the pool layer.
Violating any of these contracts causes held counter drift and budget capacity loss.

### Contract 1: Dead connection detection (CRITICAL)

When a server connection dies for ANY reason, `release()` MUST be called:

```
Trigger                              How pooler detects it
─────────────────────────────────────────────────────────────
DBA runs pg_terminate_backend()      Recycle check fails (next checkout)
PG kills via idle_in_transaction_    TCP read returns error
  session_timeout / statement_timeout
TCP keepalive timeout                OS reports connection reset
PG restart / Patroni failover        All connections fail simultaneously
OOM-killed application pod           TCP FIN or RST (eventually)
```

If `release()` is not called, `held[U]` stays inflated.
Phantom slots accumulate until pooler restart.

**Implementation**: wherever `slots.size` is decremented in `pool/inner.rs`
(failed recycle, retain removal, connection close), call `budget.release()`.

### Contract 2: Failover recovery (CRITICAL)

After PostgreSQL failover (Patroni promote, PG crash+restart), all server
connections are dead simultaneously. The budget controller needs a bulk reset:

```
fn reset_all(&self, now: Instant)
    // Set held=0 for all pools, clear connection_ages, total_held=0
    // Drain waiters (schedule as many as pool allows)
```

Called by the pool layer when it detects that the server address changed
or all health checks failed for a server target.

Without this, `total_held = P` after failover, and nobody can connect.

### Contract 3: CREATE failure rollback (CRITICAL)

`try_acquire()` increments `held[U]` immediately. If the actual PG connection
creation fails (max_connections reached, auth failure, network error),
the caller MUST call `release()` to undo the accounting.

Recommended pattern — RAII guard:

```rust
let guard = budget.try_acquire(pool, now)?;  // increments held
match server_pool.create().await {
    Ok(conn) => {
        guard.confirm();  // held stays incremented
        Ok(conn)
    }
    Err(e) => {
        drop(guard);      // held decremented automatically
        Err(e)
    }
}
```

### Contract 4: Periodic reconciliation

Even with contracts 1-3, counter drift can accumulate over time
(race conditions, bugs, edge cases). A background task should
periodically compare `held[U]` with actual live connection count:

```
every 60 seconds:
    for each pool U:
        actual = pool.slots.size  // real connections in this pool
        budget_held = budget.held(U)
        if budget_held != actual:
            log_warn("budget drift: pool={U} budget={budget_held} actual={actual}")
            budget.reconcile(U, actual)
```

`reconcile()` adjusts `held[U]` and `total_held` to match reality,
then calls `schedule()` to drain any waiters that can now be served.

---

## Failure Modes

### FM-1: PostgreSQL failover (Patroni/Stolon)

```
t=0     Primary crashes. All TCP connections broken.
t=1-30s Replica promoted. New primary accepting connections.

WITHOUT reset_all():
  Budget: total_held=P. All requests → WouldBlock.
  Pool layer detects dead connections on next recycle (idle_timeout).
  Recovery time: up to idle_timeout (minutes). FULL OUTAGE.

WITH reset_all():
  Pool layer detects failover (DNS change, connection refused).
  Calls budget.reset_all(). total_held=0.
  All pools reconnect. Budget grants up to P connections.
  Recovery time: ~1-5 seconds (PG fork time × concurrent creates).
```

### FM-2: Connection storm after restart

```
pg_doorman restarts (crash, rolling update). Budget state lost.
total_held=0. 200 clients reconnect simultaneously.
All try_acquire → Granted (up to P).
P simultaneous fork() calls to PG.

Mitigation:
  Existing max_concurrent_creates (default 4 per pool) limits fork parallelism.
  With budget controller, this should become GLOBAL (not per-pool):
  max 10-20 concurrent creates total across all pools.
```

### FM-3: Network partition

```
pg_doorman ↔ PG network breaks. Budget allows new acquires (total_held < P).
Pool layer tries CREATE → TCP timeout (30s). Client waits 30s → timeout.
Budget held[U] was incremented on acquire.
CREATE fails → release() called (Contract 3) → held[U] decremented.

With RAII guard: safe. Without: held counter inflated permanently.
```

### FM-4: Long-running transactions blocking waiters

```
batch_worker runs 20-minute pg_dump. Holds 3 guaranteed connections.
No RETURN events for 20 minutes.
Waiters for batch_worker's pool queue indefinitely.

Mitigation: add max_wait_timeout parameter (default: query_wait_timeout).
After timeout, return error instead of waiting forever.
```

---

## Calculating max_db_connections

pg_doorman is deployed as a single instance in front of PostgreSQL
(not as a sidecar per application pod).

```
P = PG_max_connections
    - superuser_reserved_connections   (default 3)
    - replication_slots                (streaming + logical)
    - monitoring_connections           (Zabbix, pg_exporter, etc.)
    - DBA_reserve                      (pgAdmin, psql)
```

Example: PG max_connections=200, superuser_reserved=3, replication=2,
monitoring=2, DBA=3:

```
P = 200 - 3 - 2 - 2 - 3 = 190
```

### Startup validation

On first connection to PG, the pooler should check:

```sql
SELECT current_setting('max_connections')::int
     - current_setting('superuser_reserved_connections')::int AS available
```

If `P > available`, log a warning:
"max_db_connections ({P}) exceeds PG available connections ({available}).
 Connections will be rejected by PostgreSQL when budget is saturated."

---

## Applicability

### Where the algorithm fits well

- **Transaction pooling** with mixed-priority users (API + batch + analytics)
- **P = 20-200**, 5-30 pools (static or dynamic via auth_query)
- **Oversubscribed environments** where sum(desired) > max_connections
- **Multi-tenant SaaS** with per-tenant auth_query pools

### Where the algorithm does NOT fit

| Scenario | Why | Alternative |
|----------|-----|-------------|
| Session-mode pooling | Eviction kills client sessions | Per-user pool_size |
| All users equal priority | Weight adds no value | Global semaphore |
| P < 5 | Configuration overhead | Fixed per-user limits |
| P > 500 | Mutex contention risk | Partition by service/database |
| Single service, single user | No contention to manage | Simple pool_size |

### Risk: budget controller makes things worse

The controller can degrade performance compared to independent pools when:

1. **min_lifetime deadlock**: pool full, all connections young, guaranteed user
   blocked for up to min_lifetime seconds. Without the controller, PG would
   accept the connection directly (until max_connections).

2. **Eviction cascades**: high-weight user triggers 10 evictions in succession.
   In passthrough mode, 10 close+open cycles degrade postmaster. Independent
   pools would never evict each other.

3. **Priority inversion**: high-weight user fills pool, returns slowly.
   Low-weight user with guaranteed=0 starves. Without the controller,
   the low-weight user could connect directly to PG.

**Mitigation**: the controller is opt-in (disabled by default). Operators
who enable it accept these trade-offs for the benefit of global budget control.

---

## Observability

### Required Prometheus metrics

```
pg_doorman_budget_total_held{server}                                 gauge
pg_doorman_budget_held{server, pool}                                 gauge
pg_doorman_budget_waiting{server, pool}                              gauge
pg_doorman_budget_above_guarantee{server, pool}                      gauge
pg_doorman_budget_max_connections{server}                             gauge (config)
pg_doorman_budget_guaranteed{server, pool}                           gauge (config)
pg_doorman_budget_weight{server, pool}                               gauge (config)
pg_doorman_budget_evictions_total{server, victim_pool, requester}    counter
pg_doorman_budget_eviction_blocked_total{server, pool}               counter
pg_doorman_budget_acquire_denied_total{server, pool, reason}         counter
```

### Recommended alerts

| Alert | Condition | Severity |
|-------|-----------|----------|
| BudgetSaturated | total_held == max_connections for > 2 min | warning |
| BudgetWaitersStuck | waiting{pool} > 0 for > 30s | warning |
| BudgetEvictionStorm | rate(evictions_total) > 5/s for > 1 min | warning |
| BudgetDrift | held{pool} != actual pool size for > 60s | critical |

### Configuration recommendations

```
max_db_connections:
  Formula: (PG max_connections - reserves) / instance_count
  Typical: 50-100 per instance

min_connection_lifetime:
  Default: 30s
  High churn: 10-15s
  Stable load: 60s
  Never: 0 (causes flap)

guaranteed_pool_size:
  Formula: ceil(p80_active_transactions * 0.8)
  Monitoring: guaranteed = 2, max = 2
  Default for dynamic users: 0

weight:
  Critical API:     100
  Background jobs:   70
  Batch/ETL:         30
  Analytics:          10
  Monitoring:         50 (with guaranteed=2)
```
