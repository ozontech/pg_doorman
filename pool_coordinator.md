# Pool Coordinator — Design Document

## Problem

pg_doorman сегодня создаёт изолированный пул для каждой пары (database, user). Каждый пул ограничен `pool_size`, но пулы не знают друг о друге. Если у БД `max_connections = 100`, а у двух юзеров `pool_size = 60`, они совместно могут открыть 120 соединений и убить PostgreSQL. Контроля на уровне БД нет.

pgbouncer решает это через `max_db_connections` + `reserve_pool_size`, но у него нет:
- гарантированного минимума на юзера
- eviction чужих idle-соединений при нехватке
- защиты свежих соединений от мгновенного eviction
- приоритетного распределения reserve по нуждаемости

## Solution: Pool Coordinator

Один `PoolCoordinator` на database. Координирует соединения всех юзеров к одной БД.

### Ключевые гарантии

| Гарантия | Описание |
|----------|----------|
| **Hard limit** | Суммарное число соединений к БД не превышает `max_db_connections` |
| **Guaranteed minimum** | Каждый юзер получает минимум `min_pool_size` соединений, даже при полной загрузке |
| **Eviction with protection** | Если юзеру нужно соединение, а лимит исчерпан — забираем idle у других, но только если те выше своего `min_pool_size` и соединение старше `min_connection_lifetime` |
| **Reserve as last resort** | Если eviction не помог в течение `reserve_pool_timeout` — берём из резерва (сверх лимита) |
| **Priority by need** | При конкуренции за reserve выигрывает юзер с наибольшей нуждой |

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

Когда `max_db_connections = 0` (default) — координатор не создаётся, поведение как сейчас.

## Behavior Scenarios

### Scenario 1: Normal operation — all fits within limit

```
max_db_connections = 100
app_service: 40 active (pool_size=60, min=10)
analytics:   15 active (pool_size=80, min=3)
Total:       55 / 100
```

Все запросы обслуживаются мгновенно. Координатор не вмешивается.

### Scenario 2: Limit reached — eviction helps

```
max_db_connections = 100
app_service: 60 active (pool_size=60, min=10)
analytics:   40 active, 20 idle (pool_size=80, min=3)
Total:       100 / 100
```

Новый клиент `migration` (min=0) хочет соединение:

1. `db_semaphore.try_acquire()` → нет permits (100/100)
2. **Eviction**: `analytics` имеет surplus = 40-3 = 37 выше min. Берём самое старое idle-соединение (age > 5s)
3. Idle-соединение analytics закрывается → permit освобождается → migration получает соединение
4. Total: 100/100, но migration теперь подключён

### Scenario 3: Eviction blocked — all below minimum or all fresh

```
max_db_connections = 20
app_service: 10 active (min=10) — ровно на минимуме
analytics:    8 active (min=3), 2 idle (age=2s < min_lifetime=5s)
migration:    0 active, wants 1
Total:        20 / 20
```

1. Eviction attempt: app_service surplus=0 (at min), skip. analytics surplus=7, но idle connections age < 5s → too fresh, skip
2. **Wait**: migration ждёт на `connection_returned` notify (до `reserve_pool_timeout=3s`)
3. Если за 3 секунды кто-то вернёт соединение → migration получает его
4. Если нет → **reserve**

### Scenario 4: Reserve used

```
(continuation of scenario 3)
3 seconds passed, no connections freed.
reserve_pool_size = 10, reserve_in_use = 0
```

1. `reserve_pool_timeout` expired
2. migration отправляет `ReserveRequest { score: (0, 1) }` в priority queue
3. Arbiter видит 1 pending request, reserve доступен → grant
4. migration получает соединение сверх лимита: total = 21 / 100+10
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

**app_service wins** — 8 ожидающих > 2. Получает reserve первым.

### Scenario 6: User below minimum gets absolute priority

```
max_db_connections = 50
app_service: 50 active, 0 idle
analytics:   0 active, 3 waiting (min=3) — below minimum!
Total: 50/50
```

Eviction: app_service surplus = 50-10 = 40. Evict 1 oldest idle... но все active (нет idle). Wait timeout.

Reserve scoring:

| User | deficit_priority | waiting_count | Score |
|------|-----------------|---------------|-------|
| analytics | **1** (0 < min 3) | 3 | **(1, 3)** |
| app_service | 0 (50 > min 10) | ... | (0, ...) |

**analytics получает reserve** — deficit_priority=1 даёт абсолютный приоритет.

### Scenario 7: Reserve exhausted

```
max_db_connections = 50
reserve_pool_size = 5, reserve_in_use = 5
All pools full, no idle, reserve full
```

Новый запрос → eviction fails → wait timeout → reserve full → **PoolError::DbLimitExhausted**

Клиент получает ошибку: `All server connections to database 'mydb' are in use (max=50, reserve=5/5)`

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
// One per database, shared across user pools
pub struct PoolCoordinator {
    db_semaphore: Semaphore,           // max_db_connections permits
    reserve_semaphore: Semaphore,      // reserve_pool_size permits
    total_connections: AtomicUsize,
    reserve_in_use: AtomicUsize,
    connection_returned: Notify,       // wake waiters
    reserve_tx: mpsc::Sender<ReserveRequest>,  // to arbiter
    config: CoordinatorConfig,
    // counters
    evictions_total: AtomicU64,
    reserve_acquisitions_total: AtomicU64,
}

// RAII — lives as long as the server connection
pub struct CoordinatorPermit {
    coordinator: Arc<PoolCoordinator>,
    is_reserve: bool,
}
// Drop → returns permit to correct semaphore, notifies waiters

// Stored inside ObjectInner alongside Server and Metrics
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
        // Collect new requests
        while let Ok(req) = rx.try_recv() {
            pending.push(req);
        }

        // Grant to highest-scoring request if reserve available
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

        // Clean up cancelled requests (oneshot dropped by caller on timeout)
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

        // Sort by surplus (current - min_pool_size) descending
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

Background `retain_connections()` task:
- **Reserve pressure relief**: idle reserve connections (`is_reserve=true`) closed when idle > `min_connection_lifetime`, even before `idle_timeout`
- **Replenish with coordination**: `min_pool_size` replenish must acquire `CoordinatorPermit` before creating; skip if unavailable

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
| INFO | Eviction: `[pool_coordinator: mydb] evicted idle conn from 'analytics' (surplus:12, age:45s) for 'app_service'` |
| WARN | Reserve used: `[pool_coordinator: mydb] reserve used (1/10) for 'migration' — eviction failed within 3000ms` |
| ERROR | Exhausted: `[pool_coordinator: mydb] all connections exhausted (100+10), user 'migration' denied` |

## Edge Cases

| Case | Behavior |
|------|----------|
| `sum(min_pool_size) > max_db_connections` | Config warning at startup. System works, but not all minimums can be satisfied simultaneously |
| `user.pool_size > max_db_connections` | Config warning. User effectively capped at max_db_connections |
| Config reload: max_db_connections changed | New PoolCoordinator created, old connections untracked, drain naturally |
| Config reload: feature disabled (→ 0) | PoolCoordinator removed, all connections continue without coordination |
| Session mode | Eviction useless (no idle). Hard limit + reserve still work. Clients queue or fail |
| All connections active, no idle | Eviction skipped, go straight to wait → reserve → error |
| Reserve connection goes idle | Closed by retain_connections() when idle > min_connection_lifetime |
| Concurrent eviction + checkout | VecDeque under Mutex, cross-pool safe via Notify |
| Replenish min_pool_size | Must acquire CoordinatorPermit; skip if unavailable |

## Files to Modify

| File | Changes |
|------|---------|
| `src/pool/pool_coordinator.rs` | **New.** PoolCoordinator, CoordinatorPermit, eviction, arbiter |
| `src/config/pool.rs` | 4 new fields + validation |
| `src/pool/inner.rs` | `coordinator_permit` in ObjectInner, permit acquire before create in `timeout_get`, `evict_one_idle` method |
| `src/pool/mod.rs` | Create coordinators in `from_config()`, `surplus_above_min()` on ConnectionPool |
| `src/pool/retain.rs` | Reserve pressure relief, replenish awareness |
| `src/pool/errors.rs` | `DbLimitExhausted` variant |
| `src/admin/show.rs` | `SHOW POOL_COORDINATOR` |
| `src/prometheus/mod.rs` | 4 new metrics |
