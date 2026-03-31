# Pool Coordinator — Design Document

## Problem

pg_doorman creates an isolated pool per (database, user) pair. Each pool is capped by `pool_size`, but pools are unaware of each other. If a database has `max_connections = 100` and two users each set `pool_size = 60`, they can collectively open 120 connections and overwhelm PostgreSQL. There is no database-level control.

pgbouncer addresses this with `max_db_connections` + `reserve_pool_size`, but lacks:
- guaranteed minimum connections per user
- eviction of other users' idle connections under pressure
- protection of freshly created connections from immediate eviction
- priority-based reserve distribution

## Solution: Pool Coordinator

One `PoolCoordinator` per database. Coordinates connections across all users of that database.

### Guarantees

| Guarantee | Description |
|-----------|-------------|
| **Hard limit** | Total connections to the database never exceed `max_db_connections` |
| **Guaranteed minimum** | Each user receives at least `min_pool_size` connections, even under full load |
| **Eviction with protection** | When a user needs a connection and the limit is reached, idle connections are taken from other users — only from those above their `min_pool_size`, and only connections older than `min_connection_lifetime` |
| **Reserve as last resort** | If eviction fails within `reserve_pool_timeout`, a connection is taken from the reserve (beyond the limit) |
| **Priority by need** | When multiple users compete for reserve, the one with the greatest need wins |

## Configuration

```toml
[pools.mydb]
server_host = "127.0.0.1"
server_port = 5432
pool_mode = "transaction"

# --- Pool Coordinator (opt-in) ---
# 0 = disabled (default, existing behavior)
max_db_connections = 100

# Don't evict connections younger than this (ms). Default: 5000
min_connection_lifetime = 5000

# Extra connections beyond max_db_connections, last resort. Default: 0
reserve_pool_size = 10

# Wait time (ms) before using reserve. Default: 3000
reserve_pool_timeout = 3000

[[pools.mydb.users]]
username = "app_service"
pool_size = 60          # max for this user
min_pool_size = 10      # guaranteed minimum

[[pools.mydb.users]]
username = "analytics"
pool_size = 80
min_pool_size = 3

[[pools.mydb.users]]
username = "migration"
pool_size = 5
min_pool_size = 0       # no guarantee, can be fully evicted
```

When `max_db_connections = 0` (default), no coordinator is created and pools behave as before.

## Behavior Scenarios

### Scenario 1: Normal operation

```
max_db_connections = 100
app_service: 40 active (pool_size=60, min=10)
analytics:   15 active (pool_size=80, min=3)
Total:       55 / 100
```

All requests served immediately. The coordinator does not intervene.

### Scenario 2: Limit reached — eviction frees a slot

```
max_db_connections = 100
app_service: 60 active (pool_size=60, min=10)
analytics:   40 active, 20 idle (pool_size=80, min=3)
Total:       100 / 100
```

A new `migration` client (min=0) needs a connection:

1. `db_semaphore.try_acquire()` fails (100/100)
2. **Eviction**: `analytics` has surplus = 40 − 3 = 37 above min. Oldest idle connection (age > 5s) is closed.
3. Permit freed. `migration` gets a connection.
4. Total stays 100/100, but `migration` is now connected.

### Scenario 3: Eviction blocked — all at minimum or all connections too fresh

```
max_db_connections = 20
app_service: 10 active (min=10) — exactly at minimum
analytics:    8 active (min=3), 2 idle (age=2s < min_lifetime=5s)
migration:    0 active, wants 1
Total:        20 / 20
```

1. Eviction scan: `app_service` surplus=0 (at min), skip. `analytics` surplus=7, but idle connections younger than 5s — skip.
2. **Wait**: `migration` blocks on `connection_returned` notify for up to `reserve_pool_timeout` (3s).
3. If someone returns a connection within 3s, `migration` gets it.
4. Otherwise → reserve.

### Scenario 4: Reserve used

```
(continuation of scenario 3)
3 seconds passed, no connections freed.
reserve_pool_size = 10, reserve_in_use = 0
```

1. `reserve_pool_timeout` expired.
2. `migration` submits `ReserveRequest { score: (0, 1) }` to the priority queue.
3. Arbiter grants it — reserve available.
4. `migration` gets a connection beyond the limit: total = 21 / 100+10.
5. **WARN log**: `[pool_coordinator: mydb] reserve connection used (1/10) for user 'migration'`

### Scenario 5: Multiple users compete for reserve

```
max_db_connections = 50
app_service: 30 active, 8 waiting clients (min=10, current=30 > min)
analytics:   20 active, 2 waiting clients (min=3, current=20 > min)
Total: 50/50, eviction impossible (all active, no idle)
```

Both hit `reserve_pool_timeout`. Scoring:

| User | deficit_priority | waiting_count | Score |
|------|-----------------|---------------|-------|
| app_service | 0 (30 > min 10) | 8 | (0, 8) |
| analytics | 0 (20 > min 3) | 2 | (0, 2) |

**app_service wins** — 8 waiting > 2.

### Scenario 6: User below minimum gets absolute priority

```
max_db_connections = 50
app_service: 50 active, 0 idle
analytics:   0 active, 3 waiting (min=3) — below minimum!
Total: 50/50
```

Eviction: `app_service` surplus = 40, but no idle connections. Wait expires.

Reserve scoring:

| User | deficit_priority | waiting_count | Score |
|------|-----------------|---------------|-------|
| analytics | **1** (0 < min 3) | 3 | **(1, 3)** |
| app_service | 0 (50 > min 10) | ... | (0, ...) |

**analytics wins** — `deficit_priority=1` gives absolute priority over users above their minimum.

### Scenario 7: Reserve exhausted

```
max_db_connections = 50
reserve_pool_size = 5, reserve_in_use = 5
All pools full, no idle, reserve full
```

New request → eviction fails → wait expires → reserve full → **PoolError::DbLimitExhausted**

Client receives: `All server connections to database 'mydb' are in use (max=50, reserve=5/5)`

## Architecture

### Components

```
┌─────────────────────────────────────────────────────────────┐
│                    PoolCoordinator (per DB)                  │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────┐ │
│  │ db_semaphore  │  │   reserve    │  │  Reserve Arbiter  │ │
│  │ (100 permits) │  │  semaphore   │  │  (priority queue) │ │
│  │               │  │ (10 permits) │  │                   │ │
│  └──────┬───────┘  └──────┬───────┘  └────────┬──────────┘ │
│         │                 │                    │            │
│  ┌──────┴─────────────────┴────────────────────┴──────┐    │
│  │              Eviction Engine                        │    │
│  │  - scan other users' idle connections               │    │
│  │  - respect min_pool_size, min_connection_lifetime    │    │
│  │  - pick user with max surplus, oldest idle first     │    │
│  └────────────────────────────────────────────────────┘    │
│                                                             │
│  Counters: total_connections, reserve_in_use,               │
│            evictions_total, reserve_acquisitions_total       │
└─────────────────────────────────────────────────────────────┘
         │                    │                    │
    ┌────┴────┐         ┌────┴────┐         ┌────┴────┐
    │ User A  │         │ User B  │         │ User C  │
    │ Pool    │         │ Pool    │         │ Pool    │
    │ (60max) │         │ (80max) │         │ (5max)  │
    │ (10min) │         │ (3min)  │         │ (0min)  │
    └─────────┘         └─────────┘         └─────────┘
```

### Connection Acquisition Flow

```
Client needs connection
│
├─ [1] Acquire per-user semaphore (existing, unchanged)
│     └─ User's pool_size enforced as before
│
├─ [2] Try reuse idle from user's VecDeque (existing, unchanged)
│     └─ Found → return (CoordinatorPermit already inside)
│
├─ [3] No idle → need new connection → coordinate:
│     │
│     ├─ [A] db_semaphore.try_acquire() → OK
│     │     └─ Create connection, store CoordinatorPermit
│     │
│     ├─ [B] Limit reached → Eviction Engine:
│     │     │  for each other user (sorted by surplus DESC):
│     │     │    skip if surplus ≤ 0 (at/below min)
│     │     │    find oldest idle with age > min_connection_lifetime
│     │     │    evict it → permit freed → try_acquire
│     │     └─ Success → create connection
│     │
│     ├─ [C] Eviction failed → Wait phase:
│     │     │  loop for reserve_pool_timeout:
│     │     │    wait on connection_returned Notify
│     │     │    retry try_acquire on db_semaphore
│     │     └─ Got permit → create connection
│     │
│     ├─ [D] Timeout → Reserve phase:
│     │     │  submit ReserveRequest { user, score }
│     │     │  score = (is_below_min, waiting_count)
│     │     │  arbiter grants to highest score
│     │     └─ Got reserve permit → create (is_reserve=true)
│     │
│     └─ [E] Reserve exhausted → error
│
└─ Connection returned to pool → CoordinatorPermit stays
   Connection destroyed → CoordinatorPermit dropped → permit freed
```

### Data Model

```rust
pub struct PoolCoordinator {
    db_semaphore: Semaphore,           // max_db_connections permits
    reserve_semaphore: Semaphore,      // reserve_pool_size permits
    total_connections: AtomicUsize,
    reserve_in_use: AtomicUsize,
    connection_returned: Notify,
    reserve_tx: mpsc::Sender<ReserveRequest>,
    config: CoordinatorConfig,
    evictions_total: AtomicU64,
    reserve_acquisitions_total: AtomicU64,
}

pub struct CoordinatorPermit {
    coordinator: Arc<PoolCoordinator>,
    is_reserve: bool,
}
// Drop returns permit to the correct semaphore and notifies waiters.

struct ObjectInner {
    obj: Server,
    metrics: Metrics,
    coordinator_permit: Option<CoordinatorPermit>,
}
```

### Reserve Arbiter

Background tokio task, one per PoolCoordinator:

```rust
struct ReserveRequest {
    user: String,
    score: (u8, usize),  // (deficit_priority, waiting_count)
    response: oneshot::Sender<CoordinatorPermit>,
}

async fn reserve_arbiter(
    mut rx: mpsc::Receiver<ReserveRequest>,
    coordinator: Arc<PoolCoordinator>,
) {
    let mut pending: BinaryHeap<ReserveRequest> = BinaryHeap::new();

    loop {
        while let Ok(req) = rx.try_recv() {
            pending.push(req);
        }

        while let Some(top) = pending.peek() {
            if coordinator.reserve_semaphore.try_acquire().is_ok() {
                let req = pending.pop().unwrap();
                let permit = CoordinatorPermit { is_reserve: true, .. };
                let _ = req.response.send(permit);
                coordinator.reserve_acquisitions_total.fetch_add(1, ..);
            } else {
                break;
            }
        }

        pending.retain(|req| !req.response.is_closed());

        tokio::select! {
            Some(req) = rx.recv() => pending.push(req),
            _ = coordinator.connection_returned.notified() => {},
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}
```

### Eviction Engine

```rust
impl PoolCoordinator {
    fn try_evict_one(&self, requesting_user: &str, db_name: &str) -> bool {
        let pools = get_all_pools_for_db(db_name);

        let mut candidates: Vec<_> = pools
            .iter()
            .filter(|(id, _)| id.user != requesting_user)
            .filter(|(_, pool)| pool.surplus_above_min() > 0)
            .collect();

        candidates.sort_by(|a, b| b.surplus().cmp(&a.surplus()));

        for (_, pool) in candidates {
            if pool.evict_one_idle(self.config.min_connection_lifetime_ms) {
                self.evictions_total.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }
}
```

### Retain Integration

The background `retain_connections()` task gains two behaviors:
- **Reserve pressure relief**: idle reserve connections (`is_reserve=true`) are closed once idle exceeds `min_connection_lifetime`, even before `idle_timeout`.
- **Coordinated replenish**: `min_pool_size` replenish acquires a `CoordinatorPermit` before creating a connection. If the permit is unavailable, it skips and retries next cycle.

## Observability

### SHOW POOL_COORDINATOR

```
database       | max_db_conn | current | reserve_size | reserve_used | evictions | reserve_acq
---------------+-------------+---------+--------------+--------------+-----------+------------
mydb           | 100         | 87      | 10           | 0            | 42        | 3
other_db       | 50          | 50      | 5            | 2            | 156       | 12
```

### Prometheus

```
pg_doorman_pool_coordinator_connections{database="mydb"}            87
pg_doorman_pool_coordinator_reserve_in_use{database="mydb"}         0
pg_doorman_pool_coordinator_evictions_total{database="mydb"}        42
pg_doorman_pool_coordinator_reserve_acquisitions_total{database="mydb"} 3
```

### Logging

| Level | Event |
|-------|-------|
| INFO | `[pool_coordinator: mydb] evicted idle conn from 'analytics' (surplus:12, age:45s) for 'app_service'` |
| WARN | `[pool_coordinator: mydb] reserve used (1/10) for 'migration' — eviction failed within 3000ms` |
| ERROR | `[pool_coordinator: mydb] all connections exhausted (100+10), user 'migration' denied` |

## Edge Cases

| Case | Behavior |
|------|----------|
| `sum(min_pool_size) > max_db_connections` | Config warning at startup. Not all minimums can be met simultaneously. |
| `user.pool_size > max_db_connections` | Config warning. User effectively capped at `max_db_connections`. |
| Config reload: `max_db_connections` changed | New PoolCoordinator created. Old connections untracked, drain naturally. |
| Config reload: feature disabled (set to 0) | PoolCoordinator removed. Existing connections continue without coordination. |
| Session mode | Eviction useless (no idle connections). Hard limit and reserve still apply. |
| All connections active, none idle | Eviction skipped. Goes straight to wait, then reserve, then error. |
| Reserve connection becomes idle | Closed by `retain_connections()` once idle exceeds `min_connection_lifetime`. |
| Concurrent eviction and checkout | VecDeque under Mutex. Cross-pool coordination via Notify. |
| `min_pool_size` replenish | Must acquire CoordinatorPermit. Skipped if unavailable. |

## Files to Modify

| File | Changes |
|------|---------|
| `src/pool/pool_coordinator.rs` | **New.** PoolCoordinator, CoordinatorPermit, eviction, arbiter |
| `src/config/pool.rs` | 4 new fields + validation |
| `src/pool/inner.rs` | `coordinator_permit` in ObjectInner, permit acquire before create in `timeout_get`, `evict_one_idle` method |
| `src/pool/mod.rs` | Create coordinators in `from_config()`, `surplus_above_min()` on ConnectionPool |
| `src/pool/retain.rs` | Reserve pressure relief, coordinated replenish |
| `src/pool/errors.rs` | `DbLimitExhausted` variant |
| `src/admin/show.rs` | `SHOW POOL_COORDINATOR` command |
| `src/prometheus/mod.rs` | 4 new metrics |
