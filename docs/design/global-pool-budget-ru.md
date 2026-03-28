# Глобальный бюджет пула: взвешенное распределение коннектов для auth_query

## Проблема

Сейчас каждый пользователь auth_query получает изолированный пул с `pool_size` коннектами к PostgreSQL.
Глобального лимита на суммарное количество серверных коннектов нет.
Если в PostgreSQL `max_connections = 100`, а у 10 пользователей по `pool_size = 40`,
пулер может попытаться открыть 400 коннектов — далеко за пределами возможностей PostgreSQL.

Кроме того, не все пользователи равны: production-сервис должен иметь приоритет над
аналитикой. Сегодня механизма для этого нет — все конкурируют одинаково,
и один noisy neighbor способен исчерпать весь бюджет коннектов.

Ни один из существующих PostgreSQL-пулеров (PgBouncer, Odyssey, PgCat, Supavisor)
не решает эту задачу. Все используют жёстко партиционированные per-user пулы
без cross-pool координации, взвешенного распределения или приоритетного планирования.

## Цели

1. **Глобальный бюджет серверных коннектов** — суммарное число коннектов к PostgreSQL не превышает заданный лимит
2. **Взвешенное распределение** — пользователи с большим весом получают пропорционально больше коннектов при контенции
3. **Гарантированный минимум** — каждый пользователь может зарезервировать минимум коннектов, который никогда не отбирается
4. **Нулевой eviction** — никогда не отменять активные запросы, никогда не закрывать активные коннекты принудительно
5. **Отсутствие голодания** — пользователь с минимальным приоритетом сохраняет свой гарантированный минимум
6. **Конвергенция за 30 секунд** — система адаптируется к изменениям нагрузки в пределах 30 секунд

## Не-цели

- Eviction активных коннектов (отмена запросов, принудительное закрытие)
- Изменение модели аутентификации PostgreSQL
- Preemption в любой форме

## Предпосылки

### Стоимость коннектов PostgreSQL (почему eviction неприемлем)

| Метрика | Значение |
|---------|----------|
| Новый коннект (localhost, Unix socket) | 2–70 мс |
| Новый коннект (TCP + TLS) | 6–150 мс |
| Память idle-коннекта (huge_pages=on) | ~1.2 MiB |
| Catalog cache (свежий коннект) | 512 KB |
| Catalog cache (после интенсивного использования) | сотни MB |
| Максимальная скорость приёма коннектов postmaster | ~1 400/сек |

Eviction (close + reopen) стоит 2–150 мс за коннект и вызывает fork() в PostgreSQL.
Одновременный eviction N коннектов создаёт fork storm, деградирующий всех пользователей.
Этот дизайн избегает eviction. Ребалансировка опирается на естественный жизненный цикл транзакций.

Источники:
- Andres Freund, "Measuring the Memory Overhead of a Postgres Connection" (2020)
- Andres Freund / Citus, "Analyzing the Limits of Connection Scalability in Postgres" (2020)
- Recall.ai, "Postgres Postmaster Does Not Scale" (2024)

### Сравнение существующих пулеров

| Возможность | PgBouncer | Odyssey | PgCat | Supavisor | RDS Proxy |
|-------------|-----------|---------|-------|-----------|-----------|
| Глобальный лимит серверных коннектов | `max_db_connections` (per-DB) | Нет | Нет | Per-tenant | % от max_conn |
| Per-user размер пула | pool_size per (db,user) | pool_size per route | pool_size per user | Per-tenant | Нет |
| Приоритет / Вес / QoS | **Нет** | **Нет** | **Нет** | **Нет** | **Нет** |
| Дисциплина очереди | FIFO (без приоритетов) | FIFO | FIFO | N/A (reject) | Непрозрачная |

Ни один PostgreSQL-пулер не реализует взвешенный fair queuing или приоритетное планирование.

---

## Алгоритм

### Ключевая идея

В режиме transaction pooling коннект занят только на время транзакции.
Когда транзакция завершается, коннект возвращается в пул. Планировщик решает,
кто получит следующий свободный коннект. Ни один коннект не отбирается принудительно.

Тот же принцип, что в Linux CFS: процесс отдаёт CPU по завершении time slice,
планировщик выбирает следующего. Для справедливости preemption не нужен.

### Параметры

```
Глобальные:
  P              — размер пула (фиксированный, глобальный бюджет)

Per-user (из конфига или user_defaults):
  w_i            — вес (относительный приоритет, по умолчанию: 100)
  m_i            — гарантированный минимум коннектов (по умолчанию: 0)
  M_i            — максимум коннектов (по умолчанию: P)

Инвариант: sum(m_i для всех сконфигурированных пользователей) <= P
```

### Состояние (runtime)

```
Per-user i:
  held_i         — коннекты, выданные пользователю (выполняются транзакции)
  waiting_i      — запросы в очереди ожидания коннекта
  demand_i       — held_i + waiting_i (сколько коннектов пользователь хочет)

Глобальные:
  total_held     — sum(held_i) по всем пользователям
  idle_count     — P - total_held (доступные коннекты в пуле)
  quota_i        — текущая квота пользователя (пересчитывается динамически)
```

### Алгоритм 1: Расчёт квот (Water-Filling)

Запускается при изменении demand любого пользователя (новый запрос, дисконнект)
или периодически (раз в секунду).

```
fn calculate_quotas(users, P):
    active = [u for u in users if u.demand > 0]
    if active is empty:
        все квоты = 0
        return

    // Фаза 1: каждый начинает с гарантированного минимума
    for u in active:
        quota[u] = u.min

    remaining = P - sum(quota[u] for u in active)

    // Фаза 2: оставшееся распределяем по весу (water-filling)
    // Повторяем до стабилизации — пользователи, упёршиеся в max или demand,
    // освобождают излишек для остальных
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
                break  // перезапуск цикла с обновлённым remaining

        if not any_capped:
            // Никто не упёрся в лимит — распределяем пропорционально (дробно)
            total_weight = sum(u.weight for u in unsatisfied)
            for u in unsatisfied:
                share = remaining * (u.weight / total_weight)
                quota[u] += share
            remaining = 0
            break

    // Фаза 3: целочисленное округление (метод наибольших остатков / Хэйра-Нимейера)
    // Коннекты дискретны — дробные квоты нужно округлить до целых,
    // сохраняя sum(quota) == P точно.
    floored = {u: floor(quota[u]) for u in active}
    remainder = P - sum(floored.values())
    fractions = sorted(
        [(quota[u] - floor(quota[u]), u) for u in active],
        descending
    )
    for i in 0..remainder:
        floored[fractions[i].user] += 1
    quota = floored
```

**Пример**: P=50, три пользователя активны с высоким demand:

| Пользователь | weight | min | max | Шаг 1 (min) | Шаг 2 (water-fill) | Итоговая квота |
|--------------|--------|-----|-----|:-----------:|:-------------------:|:--------------:|
| service_api | 100 | 5 | 40 | 5 | +30.7 → capped 36 | **36** |
| batch_worker | 30 | 2 | 20 | 2 | +9.2 → 11 | **11** |
| analytics | 10 | 0 | 10 | 0 | +3.1 → 3 | **3** |
| **Итого** | | **7** | | 7 | | **50** |

### Алгоритм 2: Запрос коннекта

Когда пользователь U отправляет запрос и ему нужен серверный коннект:

```
fn on_request(user_U):
    // Случай 1: есть idle-коннект И пользователь в пределах квоты
    if idle_count > 0 AND held[U] < quota[U]:
        grant_connection(U)
        return

    // Случай 2: есть idle-коннект, НО пользователь выше квоты
    //           И кто-то другой ниже квоты
    if idle_count > 0 AND held[U] >= quota[U]:
        below_quota = [V for V in waiting_users if held[V] < quota[V], V != U]
        if below_quota is not empty:
            // U ждёт — пропускаем недообслуженных пользователей вперёд
            enqueue(U, priority = scheduling_priority(U))
            return

        // Никто другой не ниже квоты — выдаём U (до max)
        if held[U] < max[U]:
            grant_connection(U)
            return

    // Случай 3: idle-коннектов нет
    enqueue(U, priority = scheduling_priority(U))
    wait(до query_wait_timeout)
    if timed_out:
        return error "query_wait_timeout"
```

**Приоритет планирования: Stride Scheduling (аналог Linux CFS)**

Наивная формула `deficit_ratio * weight` сломана: она нормализует по размеру квоты,
из-за чего пользователи с малой квотой систематически побеждают пользователей с большим весом.
Пример: analytics (w=10, quota=3, held=0) получает score=10.0, а service_api (w=100, quota=36, held=35) — score=2.8.

Замена — stride scheduling (тот же алгоритм что в Linux CFS, WFQ, cgroups v2 cpu.weight):

```
// Состояние per-user (инициализируется один раз):
const STRIDE_BASE: u64 = 1_000_000;
stride[U] = STRIDE_BASE / U.weight      // вычисляется при загрузке конфига
pass[U] = 0                              // растёт при каждой выдаче

// При каждой выдаче коннекта пользователю U:
pass[U] += stride[U]

// При активации нового пользователя:
pass[U] = min(pass[V] for V in active_users)   // честный старт без всплеска

fn scheduling_priority(user_U):
    if held[U] < min[U]:
        return (TIER_0, pass[U])    // наименьший pass побеждает

    if held[U] < quota[U]:
        return (TIER_1, pass[U])    // наименьший pass побеждает

    return (TIER_2, pass[U])        // наименьший pass побеждает
```

Приоритет сравнивается лексикографически: TIER_0 > TIER_1 > TIER_2, затем по pass по возрастанию (наименьший побеждает).
Пользователи с большим весом имеют меньший stride, их pass растёт медленнее — они побеждают чаще.
За любой период выдачи пропорциональны весам: weight=100 получает в 10 раз больше чем weight=10.

Stride-значения для наших трёх пользователей:
```
service_api:  stride = 1_000_000 / 100 = 10_000   (побеждает чаще всего)
batch_worker: stride = 1_000_000 / 30  = 33_333
analytics:    stride = 1_000_000 / 10  = 100_000   (побеждает реже всего)
```

### Алгоритм 3: Возврат коннекта

Когда транзакция пользователя U завершается и коннект возвращается в пул:

```
fn on_return(user_U, connection):
    held[U] -= 1

    // Ищем лучшего кандидата среди всех ожидающих
    best = highest_priority_waiter()

    if best is None:
        // Никто не ждёт — возвращаем коннект в idle-пул
        recycle(connection)
        return

    if best.user == U:
        // U сам — самый заслуженный ожидающий. Переиспользуем коннект для U.
        grant_to(best, connection)
        return

    // Другой пользователь V имеет более высокий приоритет — перенаправляем коннект

    if dedicated_mode:
        // Коннекты взаимозаменяемы (один PG server_user) — передаём напрямую
        // Стоимость: 0 (RESET ROLE уже выполнен при checkin)
        grant_to(best, connection)

    if passthrough_mode:
        // Коннекты НЕ взаимозаменяемы (разные PG-пользователи)
        // Закрываем коннект U, V создаст новый
        // Стоимость: ~100мс (один close + один open, по одному за раз)
        close(connection)          // освобождаем глобальный слот
        total_held -= 1
        notify(best)               // V может создать новый PG-коннект
```

Других механизмов ребалансировки нет. Коннекты перетекают от пользователей с превышением квоты
к пользователям с недостатком по мере завершения транзакций.

### Алгоритм 4: Жёсткий лимит (защита гарантированных минимумов)

Пользователь U не может занять столько коннектов, что минимум другого пользователя станет неудовлетворим:

```
fn hard_max(user_U, active_users):
    other_mins = sum(V.min for V in active_users if V != U)
    return min(U.max, P - other_mins)
```

Проверяется при выдаче:

```
fn grant_connection(user_U):
    if held[U] >= hard_max(U, active_users):
        enqueue(U)  // нельзя выдать — нарушит гарантии других
        return
    // ... выдаём коннект
```

**Пример**: P=50, service_api (min=5, max=40), batch_worker (min=2, max=20), analytics (min=0, max=10).
- hard_max(service_api) = min(40, 50 - 2 - 0) = 40
- hard_max(batch_worker) = min(20, 50 - 5 - 0) = 20
- hard_max(analytics) = min(10, 50 - 5 - 2) = 10

Когда активен только service_api:
- hard_max(service_api) = min(40, 50 - 0 - 0) = 40 (может использовать до 40)
- Оставшиеся 10 простаивают (service_api.max = 40)

Когда подключается batch_worker:
- hard_max(service_api) = min(40, 50 - 2) = 40 (по-прежнему 40, batch_worker.min=2 мал)
- batch_worker растёт до 20 по мере возврата коннектов service_api

### Алгоритм 5: Защита от флапа (min_connection_lifetime)

Без защиты коннекты могут осциллировать между пользователями при колебаниях нагрузки:
analytics дренируется 10→3 (стоит 7 fork() в passthrough), нагрузка смещается,
analytics снова растёт 3→10 (ещё 7 fork()), повторяется. Каждый цикл — ~700мс fork()-оверхеда.

**Решение**: коннект, созданный для пользователя, остаётся с ним минимум
`min_connection_lifetime` (default: 30 секунд) вне зависимости от изменений квоты.

```
fn on_return(user_U, connection):
    held[U] -= 1

    // Защита от флапа: молодой коннект всегда возвращается своему пользователю
    if now() - connection.created_at < min_connection_lifetime:
        if U.has_waiting_requests():
            grant_to(U, connection)
        else:
            recycle(connection)   // idle в пуле U, но закреплён за U
        return

    // Коннект старше min_lifetime — обычное планирование
    best = highest_priority_waiter()
    // ... (как в Алгоритме 3)
```

Параметр гасит осцилляции, не блокируя перераспределение.
Коннекты старше 30с свободно перераспределяются; молодые — остаются на месте.

Аналоги в других системах:
- BGP route flap damping (RFC 2439): penalty + suppress threshold + half-life
- Kubernetes HPA: 300с stabilization window для scale-down
- YARN: 10% deadband (`max_ignored_over_capacity`)
- PgBouncer: 30с `server_check_delay` (неявный trust period)

**Почему 30 секунд**:
- Совпадает с implicit trust period PgBouncer
- ~2x типичного времени установления коннекта PostgreSQL с TLS
- Достаточно коротко для конвергенции при реальных сдвигах нагрузки (в пределах минуты)
- Достаточно длинно для поглощения кратковременных пиков без churn

### Алгоритм 6: Резервный пул (вдохновлено PgBouncer)

В PgBouncer есть `reserve_pool_size` + `reserve_pool_timeout`: когда клиент ждёт дольше
`reserve_pool_timeout`, PgBouncer открывает дополнительные коннекты сверх `pool_size`
(до `pool_size + reserve_pool_size`). Градуированная реакция на давление без отказа.

Тот же механизм для глобального бюджета:

```
total_max_connections = 50     // основной бюджет (мягкий лимит)
reserve_pool_size = 5          // запас на давление (default: 10% от total_max)
reserve_pool_timeout = 5s      // время ожидания перед использованием запаса

// Жёсткий лимит: total_max + reserve_pool_size = 55
```

```
fn on_wait_timeout(user_U, elapsed):
    if elapsed >= reserve_pool_timeout
       AND total_held < total_max + reserve_pool_size:
        // Открываем резервный коннект для U
        connection = create_connection(U)
        connection.is_reserve = true
        grant_to(U, connection)
        return

    if elapsed >= query_wait_timeout:
        return error "query_wait_timeout"
```

Резервные коннекты помечаются и имеют укороченное время жизни.
Когда total_held падает ниже total_max, резервные коннекты закрываются первыми:

```
fn on_return(user_U, connection):
    // Закрываем резервные коннекты когда давление спало
    if connection.is_reserve AND total_held > total_max:
        close(connection)
        return

    // ... обычное планирование (Алгоритм 3 + Алгоритм 5)
```

Гарантии:
- При обычной нагрузке: используется только total_max коннектов
- При давлении: до total_max + reserve_pool_size временно
- Резервные коннекты автоматически убираются когда давление спадает

---

## Анализ конвергенции

### Модель

- Пользователь A держит H коннектов, квота Q (H > Q, превышение H-Q)
- Средняя длительность транзакции: T секунд
- Каждый коннект A завершается и возвращается со средней частотой 1/T
- Суммарная частота возврата всех коннектов A: H/T в секунду

### Механизм конвергенции

Когда коннект возвращается пользователем A (выше квоты), планировщик отдаёт его
пользователю B (ниже квоты) вместо переиспользования для A. held_A уменьшается на 1 за возврат.

Избыток A = H - Q коннектов должны «не быть переиспользованы».
Первый коннект возвращается через ~T/H секунд (любой из H может быть первым).
Все избыточные коннекты возвращаются в пределах ~T секунд (один полный цикл транзакций).

### Время конвергенции по длительности транзакций

| Средняя длительность транзакции | Время конвергенции | Сценарий |
|:-------------------------------:|:------------------:|----------|
| 1 мс | < 50 мс | Простые key-value запросы |
| 10 мс | < 100 мс | Типичный OLTP |
| 100 мс | < 500 мс | OLTP со сложными JOIN |
| 1 с | ~1–2 с | Отчёты, агрегации |
| 10 с | ~10–15 с | Тяжёлые аналитические запросы |
| 30 с | ~30 с | Длительные batch-запросы |

Для цели в 30 секунд: система конвергирует за 30 секунд при средней длительности
транзакции ≤ 30 секунд. Для OLTP (1–100 мс) конвергенция почти мгновенна.

### Транзакции длиннее 30 секунд

Если все коннекты пользователя A выполняют 60-секундные запросы, планировщик не может
ребалансировать до их завершения. Активные запросы не отменяются.
Конвергенция происходит за max(30 секунд, самая длинная активная транзакция).

Для нагрузок с длинными запросами оператору следует установить соответствующий `max_pool_size`,
чтобы один пользователь не занимал весь пул на продолжительное время.

---

## Пошаговый пример: три пользователя

### Настройка

```
P = 50 (общий бюджет пула)

service_api:  weight=100, min=5, max=40
batch_worker: weight=30,  min=2, max=20
analytics:    weight=10,  min=0, max=10

Средняя длительность транзакции: 10мс (OLTP)
```

### Фаза 1: Активен только service_api

```
Квоты: service_api = min(5 + 45*100/100, 40) = 40
Состояние: service_api: held=40, idle=10
```

service_api использует 40 коннектов. 10 простаивают (service_api на максимуме).

### Фаза 2: Подключается batch_worker (t=0)

10 клиентов подключаются через auth_query как batch_worker.

```
Пересчёт квот (оба активны):
  reserved = 5 + 2 = 7, distributable = 43
  service_api: 5 + 43*(100/130) = 5 + 33 = 38
  batch_worker: 2 + 43*(30/130) = 2 + 10 = 12

Состояние: service_api: held=40, quota=38 (превышение на 2)
           batch_worker: held=0, quota=12 (дефицит 12)
           idle=10
```

**t=0 мс**: batch_worker запрашивает 12 коннектов.
- 10 idle-коннектов доступно. batch_worker.held < quota. Выдаём 10 сразу.
- batch_worker: held=10, waiting=2. idle=0.

**t≈10 мс**: service_api возвращает коннект (транзакция завершена).
- service_api: held=39 (выше квоты 38)
- Приоритетный ожидающий: batch_worker (held=10, quota=12, TIER_1)
- **Выдаём batch_worker**, не возвращаем service_api.
- service_api: held=39, batch_worker: held=11

**t≈20 мс**: service_api возвращает ещё один коннект.
- service_api: held=38 (теперь НА квоте)
- Приоритетный ожидающий: batch_worker (held=11, quota=12, TIER_1)
- **Выдаём batch_worker.**
- service_api: held=38, batch_worker: held=12

**t≈20 мс и далее**: Стационарное состояние.
```
service_api: held=38, quota=38  ✓
batch_worker: held=12, quota=12 ✓
Итого: 50/50
```

**Время конвергенции: ~20 мс** (2 завершения транзакций).

### Фаза 3: Подключается analytics (t=1с)

3 клиента подключаются как analytics.

```
Пересчёт квот (все три активны):
  reserved = 5 + 2 + 0 = 7, distributable = 43
  service_api: 5 + 43*(100/140) = 5 + 30.7 = 36
  batch_worker: 2 + 43*(30/140) = 2 + 9.2 = 11
  analytics: 0 + 43*(10/140) = 3.1 = 3

Состояние: service_api: held=38, quota=36 (превышение на 2)
           batch_worker: held=12, quota=11 (превышение на 1)
           analytics: held=0, quota=3 (дефицит 3)
           idle=0
```

**t=1.010 с**: batch_worker возвращает коннект.
- batch_worker: held=11 (на квоте)
- Приоритетный ожидающий: analytics (TIER_1, deficit=3/3=1.0, score=10)
- **Выдаём analytics.** analytics: held=1.

**t=1.020 с**: service_api возвращает коннект.
- service_api: held=37 (выше квоты 36)
- Приоритетный ожидающий: analytics (held=1, quota=3, TIER_1)
- **Выдаём analytics.** analytics: held=2.

**t=1.030 с**: service_api возвращает ещё один.
- service_api: held=36 (теперь НА квоте)
- Приоритетный ожидающий: analytics (held=2, quota=3, TIER_1)
- **Выдаём analytics.** analytics: held=3.

**t=1.030 с и далее**: Стационарное состояние.
```
service_api:  held=36, quota=36 ✓
batch_worker: held=11, quota=11 ✓
analytics:    held=3,  quota=3  ✓
Итого: 50/50
```

**Время конвергенции: ~30 мс.**

### Фаза 4: analytics отключается (t=2с)

Все клиенты analytics отключаются. demand analytics падает до 0.

```
Пересчёт квот (service_api + batch_worker):
  reserved = 5 + 2 = 7, distributable = 43
  service_api: 38, batch_worker: 12

Состояние: analytics: held=3, demand=0 (коннекты ещё держатся, завершаются)
```

По мере завершения 3 транзакций analytics коннекты возвращаются. demand analytics = 0,
никто для analytics не встаёт в очередь. Возвращённые коннекты уходят в idle-пул
(или service_api/batch_worker, если у них есть ожидающие запросы).

В пределах ~10 мс все 3 коннекта analytics возвращаются:
```
service_api:  held=38, quota=38 ✓
batch_worker: held=12, quota=12 ✓
analytics:    held=0            ✓
Итого: 50/50
```

### Фаза 5: Всплеск нагрузки service_api (t=3с)

service_api получает пик трафика: 100 клиентов отправляют запросы одновременно.
demand service_api скачет до 100, но max=40, quota=38.

Планировщик выдаёт service_api коннекты до квоты (38). Остальные 62 запроса
стоят в очереди и ждут возврата коннектов (транзакция завершена → мгновенная повторная выдача service_api).
С 38 коннектами, циклирующими со средним временем 10 мс, service_api обрабатывает ~3 800 транзакций/сек.

batch_worker не затронут — сохраняет свои 12 коннектов и продолжает работу.

---

## Dedicated vs Passthrough

### Dedicated Mode (server_user)

Все коннекты аутентифицированы как один и тот же PostgreSQL-пользователь. Коннекты **взаимозаменяемы**.

Когда планировщик передаёт возвращённый коннект User A пользователю User B:
- Коннект передаётся напрямую (RESET ROLE уже выполнен при checkin)
- Стоимость: 0 мс. Без close, open, fork().

### Passthrough Mode (каждый аутентифицируется как он сам)

Коннекты **не взаимозаменяемы** — PG-коннект User A аутентифицирован как User A
и не может использоваться User B.

Когда User A выше квоты, а User B ниже:
1. Возвращённый коннект User A закрывается вместо переиспользования
2. User B создаёт новый PG-коннект, используя освободившийся глобальный слот
3. Стоимость: ~100 мс за перебалансированный коннект (один close + один open)

По одному коннекту за раз, растянуто на период конвергенции. Без fork storm.

| Свойство | Dedicated | Passthrough |
|----------|-----------|-------------|
| Стоимость ребалансировки per-connection | 0 мс | ~100 мс |
| Вызовов fork() при ребалансировке | 0 | 1 на мигрированный коннект |
| Сохранение тёплого кэша коннекта | Да | Нет (новый коннект, холодный кэш) |

---

## Конфигурация

```toml
[pools.mydb.auth_query]
# Глобальный бюджет коннектов для этого database pool
total_max_connections = 50

# Резервный пул (по аналогии с PgBouncer): доп. коннекты при давлении
reserve_pool_size = 5               # default: 10% от total_max
reserve_pool_timeout = 5000         # мс до использования резерва (default: 5с)

# Защита от флапа: минимальное время жизни коннекта у пользователя
min_connection_lifetime = 30000     # мс (default: 30с)

# Значения по умолчанию для всех auth_query пользователей
default_weight = 100
default_min_pool_size = 0
default_max_pool_size = 10

# Переопределения для конкретных пользователей (по username из auth_query)
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

**Валидация при загрузке конфига:**
1. `sum(min_pool_size для всех сконфигурированных пользователей) <= total_max_connections`
2. `для каждого пользователя: min_pool_size <= max_pool_size`
3. `для каждого пользователя: max_pool_size <= total_max_connections`
4. `total_max_connections > 0`
5. `reserve_pool_size >= 0`
6. `reserve_pool_timeout < query_wait_timeout`
7. `min_connection_lifetime > 0`

Пользователи, не перечисленные в `user_overrides`, получают значения `default_*`.

---

## Открытые вопросы

1. **Динамические пользователи без переопределений.** Когда ранее неизвестный пользователь
   аутентифицируется через auth_query, он получает значения default. Если таких пользователей
   появляется много, сумма их defaults и настроенных overrides должна укладываться в total_max_connections.
   Вариант: ограничить `default_max_pool_size` так, чтобы worst-case количество пользователей укладывалось.
   Вариант: отслеживать количество активных пользователей и корректировать defaults динамически.

2. **Пересчёт fair share только по активным пользователям?**
   Текущий алгоритм: да — в расчёте квот участвуют только пользователи с demand > 0.
   Один активный пользователь может использовать до своего max (не total_max).
   Когда появляется второй, квоты сдвигаются и система ребалансируется.

3. **Метрики и наблюдаемость.** Нужно экспортировать per-user: held, quota, min, max, waiting,
   rate выдачи, wait time p50/p99. Глобально: total_held, idle_count, события ребалансировки.

4. **Интеграция с существующей архитектурой пула.** Глобальный бюджет находится над отдельными
   user-пулами. В dedicated mode оборачивает общий пул. В passthrough mode координирует
   per-user пулы. Семафор в PoolInner должен управляться глобальным бюджетом,
   а не действовать независимо.
