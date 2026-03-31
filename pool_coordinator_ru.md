# Pool Coordinator — проектный документ

## Проблема

pg_doorman создаёт изолированный pool на каждую пару (database, user). Каждый pool ограничен `pool_size`, но pool'ы не знают друг о друге. Если у БД `max_connections = 100`, а у двух user'ов `pool_size = 60`, вместе они могут открыть 120 server connections и перегрузить PostgreSQL. Контроля на уровне database нет.

pgbouncer решает часть проблемы через `max_db_connections` + `reserve_pool_size`, но не умеет:
- гарантировать minimum connections конкретному user'у
- забирать чужие idle connections при нехватке (eviction)
- защищать свежие connections от немедленного eviction (lifetime protection)
- распределять reserve по степени нуждаемости (priority scoring)

## Решение: Pool Coordinator

Один `PoolCoordinator` на database. Координирует server connections всех user'ов к одной БД.

### Гарантии

| Гарантия | Что это значит |
|----------|---------------|
| **Hard limit** | Суммарное число server connections к БД не превышает `max_db_connections` |
| **Guaranteed minimum** | Каждый user получает не менее `min_pool_size` соединений при любой нагрузке |
| **Eviction with protection** | Если user'у нужно соединение, а limit исчерпан — idle connections забираются у других, но только у тех, кто выше своего `min_pool_size`, и только если connection старше `min_connection_lifetime` |
| **Reserve as last resort** | Если eviction не помог за `reserve_pool_timeout` — берётся connection из reserve pool (сверх лимита) |
| **Priority by need** | При конкуренции за reserve побеждает user с наибольшим числом waiting clients |

## Конфигурация

```toml
[pools.mydb]
server_host = "127.0.0.1"
server_port = 5432
pool_mode = "transaction"

# --- Pool Coordinator (включается явно) ---
# 0 = выключен (по умолчанию, поведение как раньше)
max_db_connections = 100

# Не изымать соединения моложе этого (мс). По умолчанию: 5000
min_connection_lifetime = 5000

# Дополнительные соединения сверх max_db_connections. По умолчанию: 0
reserve_pool_size = 10

# Сколько ждать (мс) перед использованием резерва. По умолчанию: 3000
reserve_pool_timeout = 3000

[[pools.mydb.users]]
username = "app_service"
pool_size = 60          # максимум для этого пользователя
min_pool_size = 10      # гарантированный минимум

[[pools.mydb.users]]
username = "analytics"
pool_size = 80
min_pool_size = 3

[[pools.mydb.users]]
username = "migration"
pool_size = 5
min_pool_size = 0       # без гарантий, все соединения могут быть изъяты
```

При `max_db_connections = 0` (default) coordinator не создаётся, pool'ы работают как раньше.

## Сценарии поведения

### Сценарий 1: normal operation

```
max_db_connections = 100
app_service: 40 активных (pool_size=60, min=10)
analytics:   15 активных (pool_size=80, min=3)
Всего:       55 / 100
```

Все запросы обслуживаются сразу. Coordinator не вмешивается.

### Сценарий 2: limit reached — eviction помогает

```
max_db_connections = 100
app_service: 60 активных (pool_size=60, min=10)
analytics:   40 активных, 20 idle (pool_size=80, min=3)
Всего:       100 / 100
```

Новый клиент `migration` (min=0) запрашивает соединение:

1. `db_semaphore.try_acquire()` — нет свободных permits (100/100).
2. **Изъятие**: у `analytics` surplus = 40 − 3 = 37 выше минимума. Закрывается самое старое idle-соединение (возраст > 5с).
3. Permit освобождён. `migration` получает соединение.
4. Итого по-прежнему 100/100, но `migration` подключён.

### Сценарий 3: eviction невозможен — все на minimum или connections слишком свежие

```
max_db_connections = 20
app_service: 10 активных (min=10) — ровно на минимуме
analytics:    8 активных (min=3), 2 idle (возраст 2с < min_lifetime=5с)
migration:    0 активных, нужно 1
Всего:        20 / 20
```

1. Eviction attempt: `app_service` surplus=0, skip. `analytics` surplus=7, но idle connections моложе 5с — skip.
2. **Wait phase**: `migration` ждёт на `connection_returned` notify до `reserve_pool_timeout` (3с).
3. Если за 3 секунды кто-то вернёт connection — `migration` его получает.
4. Если нет — reserve.

### Сценарий 4: reserve pool используется

```
(продолжение сценария 3)
Прошло 3 секунды, соединения не освободились.
reserve_pool_size = 10, reserve_in_use = 0
```

1. Таймаут `reserve_pool_timeout` истёк.
2. `migration` отправляет `ReserveRequest { score: (0, 1) }` в очередь приоритетов.
3. Арбитр видит 1 запрос, резерв доступен — выдаёт.
4. `migration` получает соединение сверх лимита: всего 21 / 100+10.
5. **WARN лог**: `[pool_coordinator: mydb] reserve connection used (1/10) for user 'migration'`

### Сценарий 5: несколько пользователей претендуют на резерв

```
max_db_connections = 50
app_service: 30 активных, 8 ожидающих клиентов (min=10, текущих=30 > min)
analytics:   20 активных, 2 ожидающих клиента (min=3, текущих=20 > min)
Всего: 50/50, изъятие невозможно (все активные, нет idle)
```

Оба дождались `reserve_pool_timeout`. Скоринг:

| Пользователь | deficit_priority | waiting_count | Score |
|-------------|-----------------|---------------|-------|
| app_service | 0 (30 > min 10) | 8 | (0, 8) |
| analytics | 0 (20 > min 3) | 2 | (0, 2) |

**app_service побеждает** — 8 ожидающих против 2.

### Сценарий 6: пользователь ниже минимума получает абсолютный приоритет

```
max_db_connections = 50
app_service: 50 активных, 0 idle
analytics:   0 активных, 3 ожидающих (min=3) — ниже минимума!
Всего: 50/50
```

Изъятие: у `app_service` surplus=40, но нет idle-соединений. Таймаут.

Скоринг резерва:

| Пользователь | deficit_priority | waiting_count | Score |
|-------------|-----------------|---------------|-------|
| analytics | **1** (0 < min 3) | 3 | **(1, 3)** |
| app_service | 0 (50 > min 10) | ... | (0, ...) |

**analytics побеждает** — `deficit_priority=1` даёт абсолютный приоритет над пользователями выше минимума.

### Сценарий 7: резерв исчерпан

```
max_db_connections = 50
reserve_pool_size = 5, reserve_in_use = 5
Все пулы заняты, idle нет, резерв заполнен
```

Новый запрос → изъятие не удалось → таймаут → резерв заполнен → **PoolError::DbLimitExhausted**

Клиент получает: `All server connections to database 'mydb' are in use (max=50, reserve=5/5)`

## Архитектура

### Компоненты

```
┌─────────────────────────────────────────────────────────────┐
│                    PoolCoordinator (на БД)                   │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────┐ │
│  │ db_semaphore  │  │   reserve    │  │  Reserve Arbiter  │ │
│  │ (100 permits) │  │  semaphore   │  │  (очередь приор.) │ │
│  │               │  │ (10 permits) │  │                   │ │
│  └──────┬───────┘  └──────┬───────┘  └────────┬──────────┘ │
│         │                 │                    │            │
│  ┌──────┴─────────────────┴────────────────────┴──────┐    │
│  │              Eviction Engine                        │    │
│  │  - сканирует idle connections чужих user'ов          │    │
│  │  - учитывает min_pool_size, min_connection_lifetime  │    │
│  │  - evict у user с макс. surplus                      │    │
│  │    (surplus = connections - min_pool_size)            │    │
│  │  - oldest idle connection first                      │    │
│  └────────────────────────────────────────────────────┘    │
│                                                             │
│  Счётчики: total_connections, reserve_in_use,               │
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

### Получение соединения

```
Клиенту нужно соединение
│
├─ [1] Захват per-user semaphore (как сейчас)
│     └─ pool_size пользователя ограничивает как раньше
│
├─ [2] Попытка взять idle из VecDeque (как сейчас)
│     └─ Нашли → возвращаем (CoordinatorPermit уже внутри)
│
├─ [3] Нет idle → нужно новое соединение → координация:
│     │
│     ├─ [A] db_semaphore.try_acquire() → OK
│     │     └─ Создаём соединение, сохраняем CoordinatorPermit
│     │
│     ├─ [B] Лимит достигнут → Eviction Engine:
│     │     │  для каждого чужого пользователя (по surplus DESC):
│     │     │    пропуск если surplus ≤ 0 (на/ниже минимума)
│     │     │    ищем старейший idle с age > min_connection_lifetime
│     │     │    изымаем → permit освобождён → try_acquire
│     │     └─ Успех → создаём соединение
│     │
│     ├─ [C] Изъятие не удалось → фаза ожидания:
│     │     │  цикл до reserve_pool_timeout:
│     │     │    ждём на connection_returned Notify
│     │     │    повторяем try_acquire на db_semaphore
│     │     └─ Получили permit → создаём соединение
│     │
│     ├─ [D] Таймаут → фаза резерва:
│     │     │  отправляем ReserveRequest { user, score }
│     │     │  score = (is_below_min, waiting_count)
│     │     │  арбитр выдаёт самому нуждающемуся
│     │     └─ Получили reserve permit → создаём (is_reserve=true)
│     │
│     └─ [E] Резерв исчерпан → ошибка
│
└─ Соединение возвращено в пул → CoordinatorPermit остаётся
   Соединение уничтожено → CoordinatorPermit дропается → permit возвращён
```

### Модель данных

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
// Drop возвращает permit в нужный semaphore и будит ожидающих.

struct ObjectInner {
    obj: Server,
    metrics: Metrics,
    coordinator_permit: Option<CoordinatorPermit>,
}
```

### Reserve Arbiter

Фоновый tokio task, по одному на PoolCoordinator:

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

### Интеграция с retain

Фоновый task `retain_connections()` получает два новых поведения:
- **Сброс давления резерва**: idle reserve-соединения (`is_reserve=true`) закрываются при idle > `min_connection_lifetime`, даже до `idle_timeout`.
- **Пополнение с координацией**: replenish до `min_pool_size` требует `CoordinatorPermit`. Если permit недоступен — пропуск, повтор в следующем цикле.

## Наблюдаемость

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

### Логирование

| Уровень | Событие |
|---------|---------|
| INFO | `[pool_coordinator: mydb] evicted idle conn from 'analytics' (surplus:12, age:45s) for 'app_service'` |
| WARN | `[pool_coordinator: mydb] reserve used (1/10) for 'migration' — eviction failed within 3000ms` |
| ERROR | `[pool_coordinator: mydb] all connections exhausted (100+10), user 'migration' denied` |

## Граничные случаи

| Случай | Поведение |
|--------|-----------|
| `sum(min_pool_size) > max_db_connections` | Предупреждение при старте. Не все минимумы могут быть выполнены одновременно. |
| `user.pool_size > max_db_connections` | Предупреждение. Фактически пользователь ограничен `max_db_connections`. |
| Перезагрузка конфига: `max_db_connections` изменён | Создаётся новый PoolCoordinator. Старые соединения доживают естественным образом. |
| Перезагрузка конфига: фича выключена (значение 0) | PoolCoordinator удалён. Существующие соединения продолжают работать без координации. |
| Session mode | Изъятие бесполезно (нет idle). Лимит и резерв работают. Клиенты ждут или получают ошибку. |
| Все соединения активны, idle нет | Изъятие пропускается. Сразу ожидание → резерв → ошибка. |
| Reserve-соединение стало idle | Закрывается `retain_connections()` при idle > `min_connection_lifetime`. |
| Параллельное изъятие и checkout | VecDeque под Mutex, кросс-пул координация через Notify. |
| Пополнение `min_pool_size` | Требуется CoordinatorPermit. Пропускается если недоступен. |

## Изменяемые файлы

| Файл | Изменения |
|------|-----------|
| `src/pool/pool_coordinator.rs` | **Новый.** PoolCoordinator, CoordinatorPermit, eviction, arbiter |
| `src/config/pool.rs` | 4 новых поля + валидация |
| `src/pool/inner.rs` | `coordinator_permit` в ObjectInner, захват permit перед create в `timeout_get`, метод `evict_one_idle` |
| `src/pool/mod.rs` | Создание координаторов в `from_config()`, `surplus_above_min()` на ConnectionPool |
| `src/pool/retain.rs` | Сброс давления резерва, пополнение с координацией |
| `src/pool/errors.rs` | Вариант `DbLimitExhausted` |
| `src/admin/show.rs` | Команда `SHOW POOL_COORDINATOR` |
| `src/prometheus/mod.rs` | 4 новые метрики |
