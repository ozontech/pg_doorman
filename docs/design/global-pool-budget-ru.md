# Глобальный бюджет пула: взвешенное распределение коннектов для auth_query

## Проблема

Каждый пользователь auth_query получает изолированный пул без глобального лимита на серверные коннекты.
Если в PostgreSQL `max_connections = 100`, а у 10 пользователей по `pool_size = 40`,
пулер может попытаться открыть 400 коннектов. Ни один из существующих PostgreSQL-пулеров
(PgBouncer, Odyssey, PgCat, Supavisor) не решает эту задачу с учётом весов.

## Параметры

```
Глобальные:
  P                  — max_db_connections (жёсткий лимит на серверные коннекты)
  min_lifetime       — min_connection_lifetime (default: 30с)
                       коннект не может быть вытеснен до достижения этого возраста

Per-user:
  guaranteed         — guaranteed_pool_size (всегда доступен, открывается сразу)
  weight             — вес при конкуренции за коннекты сверх гарантии
  max                — max_pool_size (жёсткий лимит на пользователя)

Инвариант: sum(guaranteed для всех сконфигурированных пользователей) <= P
```

## Состояние

```
Per-user U:
  held[U]            — серверные коннекты, назначенные пользователю U
  waiting[U]         — запросы U, ожидающие коннект

Per-connection C:
  C.user             — какому пользователю принадлежит коннект
  C.created_at       — когда PG-бэкенд был создан (время fork)

Вычисляемые:
  total_held         = sum(held[U] по всем пользователям)
  above_guarantee[U] = max(0, held[U] - guaranteed[U])
```

## Формулы

**Приоритет ожидающего** (кто получит следующий свободный коннект):

```
priority(U) = (is_guaranteed(U), weight[U], waiting[U])
              сравнивается лексикографически, по убыванию

где is_guaranteed(U) = (held[U] < guaranteed[U])
```

Гарантированные запросы побеждают всегда. Среди ожидающих сверх гарантии: побеждает наибольший вес.
При равном весе: побеждает тот, у кого больше запросов в очереди.

**Допустимость вытеснения** (можно ли вытеснить коннект C для запросившего R?):

```
evictable(C, R) =
    held[C.user] > guaranteed[C.user]          // C сверх гарантии
    AND now() - C.created_at >= min_lifetime   // C достаточно стар
    AND (is_guaranteed(R)                       // R — гарантированный (побеждает всех)
         OR weight[C.user] < weight[R])         // ИЛИ у R больше вес
```

**Порядок вытеснения** (какой коннект вытеснять первым):

```
eviction_score(C) = (weight[C.user] ASC, age(C) DESC)
```

Вытесняем у пользователя с наименьшим весом. При равном весе: старейший коннект.

---

## Алгоритм

Три события управляют системой:

### Событие 1: REQUEST — Пользователь U запрашивает коннект

```
                          REQUEST(U)
                              │
                 ┌────────────┴────────────┐
                 │ held[U] < guaranteed[U]? │
                 └────────────┬────────────┘
                      да      │      нет
                      ▼       │      ▼
               ┌──────────┐   │  ┌──────────────────┐
               │ НЕМЕДЛЕННО│   │  │ held[U] < max[U]? │
               │ (см. ниже)│   │  └────────┬─────────┘
               └──────────┘   │     да     │     нет
                              │     ▼      │     ▼
                              │ ENQUEUE(U) │  ОШИБКА
                              │ SCHEDULE() │  "user at max"
                              │            │
                              └────────────┘
```

**НЕМЕДЛЕННО (гарантированный запрос):**

```
┌──────────────────┐    да     ┌───────────────┐
│ есть idle?       ├──────────►│ GRANT(U, idle)│
└────────┬─────────┘           └───────────────┘
         │ нет
         ▼
┌──────────────────┐    да     ┌───────────────┐
│ total_held < P?  ├──────────►│ CREATE(U)     │
└────────┬─────────┘           └───────────────┘
         │ нет
         ▼
┌──────────────────┐  найден   ┌───────────────┐
│ FIND_EVICTABLE() ├──────────►│ EVICT(жертва) │
│ (weight = ∞)     │           │ CREATE(U)     │
└────────┬─────────┘           └───────────────┘
         │ не найден (все слишком молоды)
         ▼
┌──────────────────────────────┐
│ ENQUEUE(U) — ждём пока       │
│ какой-то коннект достигнет   │
│ min_lifetime, затем повтор   │
└──────────────────────────────┘
```

### Событие 2: RETURN — Пользователь U завершил транзакцию

```
                     RETURN(U, connection)
                              │
                              ▼
                      held[U] -= 1
                              │
                              ▼
                         SCHEDULE()
```

Возвращённый коннект уходит в idle-пул. SCHEDULE() решает, кому его отдать.

### Событие 3: SCHEDULE — Назначение свободных коннектов ожидающим

```
                         SCHEDULE()
                              │
                 ┌────────────┴────────────┐
                 │ есть ожидающие?          │
                 └────────────┬────────────┘
                      нет     │     да
                      ▼       │      ▼
                   (готово)   │  best = SELECT_BEST_WAITER()
                              │      │
                              │      ▼
                 ┌─────────────────────────────┐
                 │ idle доступен               │
                 │ ИЛИ total_held < P?         │
                 └────────────┬───────────────┘
                         да   │         нет (пул полон)
                              ▼                ▼
                 ┌─────────────────┐  ┌─────────────────┐
                 │ GRANT(best)     │  │ FIND_EVICTABLE   │
                 │ или CREATE(best)│  │ (weight = best)  │
                 └─────────────────┘  └────────┬────────┘
                                        найден │   не найден
                                               ▼        ▼
                                     ┌──────────┐  (best остаётся
                                     │EVICT →   │   в очереди,
                                     │CREATE    │   повтор при
                                     │(best)    │   следующем
                                     └──────────┘   RETURN)
```

### Хелпер: SELECT_BEST_WAITER

```
fn select_best_waiter():
    // Гарантированные первыми, затем по весу, затем по числу ожидающих
    return waiters.max_by(|W|
        (held[W] < guaranteed[W],     // true > false (гарантированные первыми)
         weight[W],                    // больший вес побеждает
         waiting[W])                   // больше запросов побеждает (тай-брейк)
    )
```

### Хелпер: FIND_EVICTABLE

```
fn find_evictable(requester_weight):
    candidates = []
    for each connection C назначенный любому пользователю:
        if held[C.user] <= guaranteed[C.user]:   continue  // в гарантии: священный
        if age(C) < min_lifetime:                 continue  // слишком молод: защищён
        if requester_weight != ∞                            // не гарантированный запрос
           AND weight[C.user] >= requester_weight: continue // тот же или выше вес: безопасен
        candidates.push(C)

    if candidates.is_empty(): return None

    // Жертва: наименьший вес первым, старейший коннект первым
    return candidates.min_by(|C| (weight[C.user], -(age(C))))
```

---

## Диаграммы поведения

### Настройка

```
P = 20 (max_db_connections)
min_lifetime = 30с

service_api:  guaranteed=5, weight=100, max=15
batch_worker: guaranteed=3, weight=50,  max=10
analytics:    guaranteed=0, weight=10,  max=5
```

### Сценарий 1: Нормальный запуск

```
t=0с    Все пользователи стартуют. Пул пуст.

        service_api запрашивает 8 коннектов:
          5 в гарантии → CREATE немедленно (held=5)
          3 сверх гарантии → ENQUEUE, SCHEDULE:
            нет конкурентов → CREATE немедленно (held=8)
        total_held = 8

        batch_worker запрашивает 5 коннектов:
          3 в гарантии → CREATE немедленно (held=3)
          2 сверх гарантии → ENQUEUE, SCHEDULE:
            нет конкурентов → CREATE немедленно (held=5)
        total_held = 13

        analytics запрашивает 3 коннекта:
          0 в гарантии (guaranteed=0)
          3 сверх гарантии → ENQUEUE, SCHEDULE:
            нет конкурентов → CREATE немедленно (held=3)
        total_held = 16

        Итоговое состояние:
        ┌──────────────┬──────┬────────────┬──────────────┐
        │ Пользователь │ held │ guaranteed │ сверх гарант. │
        ├──────────────┼──────┼────────────┼──────────────┤
        │ service_api  │    8 │          5 │            3 │
        │ batch_worker │    5 │          3 │            2 │
        │ analytics    │    3 │          0 │            3 │
        ├──────────────┼──────┼────────────┼──────────────┤
        │ итого        │   16 │          8 │            8 │
        └──────────────┴──────┴────────────┴──────────────┘
        Пул: 16/20. 4 слота свободно.
```

### Сценарий 2: Пул заполнен, конкуренция по весу

```
t=1с    service_api запрашивает ещё 4 коннекта (хочет 12 всего).
        Сверх гарантии. ENQUEUE, SCHEDULE:
          total_held=16, P=20 → есть место → CREATE 4.
          service_api: held=12. total_held=20. ПУЛ ПОЛОН.

t=1с    analytics запрашивает ещё 2 коннекта (хочет 5 всего).
        Сверх гарантии. ENQUEUE, SCHEDULE:
          total_held=20 = P → пул полон.
          FIND_EVICTABLE(weight=10):
            service_api: 7 сверх гарантии, weight=100 > 10 → НЕ вытесняем
            batch_worker: 2 сверх гарантии, weight=50 > 10 → НЕ вытесняем
            analytics: 3 сверх гарантии, weight=10 = 10 → НЕ вытесняем (не <)
            Жертв нет.
          analytics остаётся в очереди. Ждёт естественных возвратов.
```

### Сценарий 3: Возврат транзакции, вес решает

```
t=1.01с batch_worker завершает транзакцию. RETURN(batch_worker, conn).
        batch_worker: held=4. total_held=19.
        SCHEDULE():
          Ожидающие: analytics (weight=10, waiting=2, сверх гарантии)
          idle=1, total_held=19 < P=20.
          → GRANT(analytics). analytics: held=4, waiting=1.

t=1.02с service_api завершает транзакцию. RETURN(service_api, conn).
        service_api: held=11. total_held=19.
        SCHEDULE():
          Ожидающие: analytics (weight=10, waiting=1)
          → GRANT(analytics). analytics: held=5=max. waiting=0.
```

### Сценарий 4: Пользователь с высоким весом вытесняет низкий вес

```
t=35с   (Все коннекты старше 30с — прошли min_lifetime)

        service_api запрашивает ещё 3 коннекта (хочет 15=max).
        Сверх гарантии. ENQUEUE, SCHEDULE:
          total_held=20 = P. Пул полон.
          FIND_EVICTABLE(weight=100):
            analytics: 5 сверх гарантии, weight=10 < 100, age=34с > 30с → МОЖНО
          EVICT(analytics). analytics: held=4. CREATE(service_api). held=12.
          Повторяем для оставшихся 2 запросов...

        Итог после вытеснений:
        ┌──────────────┬──────┬──────────────┬─────────────────────┐
        │ Пользователь │ held │ сверх гарант. │ примечание          │
        ├──────────────┼──────┼──────────────┼─────────────────────┤
        │ service_api  │   14 │            9 │                     │
        │ batch_worker │    4 │            1 │                     │
        │ analytics    │    2 │            2 │ 3 конн. вытеснены   │
        └──────────────┴──────┴──────────────┴─────────────────────┘

        analytics потерял 3 коннекта потому что:
        weight(analytics)=10 < weight(service_api)=100
        И все коннекты были старше min_lifetime=30с.
```

### Сценарий 5: Гарантированный запрос вытесняет любой вес

```
t=40с   Новый пользователь "admin": guaranteed=2, weight=1, max=2.
        admin запрашивает 2 коннекта. Оба в гарантии.

        НЕМЕДЛЕННО: total_held=20=P. Пул полон.
        FIND_EVICTABLE(weight=∞):  // гарантированный запрос побеждает любой вес
          analytics: 2 сверх гарантии, weight=10, age>30с → можно вытеснить
          EVICT(analytics). CREATE(admin).
          EVICT(analytics). CREATE(admin).

        admin: held=2. Несмотря на weight=1 (минимальный),
        гарантированные запросы вытесняют коннекты сверх гарантии вне зависимости от веса.
```

### Сценарий 6: Защита от флапа (min_lifetime предотвращает осцилляцию)

```
t=40с   analytics имеет 0 коннектов. Запрашивает 3.
        Сверх гарантии (guaranteed=0). ENQUEUE.
        SCHEDULE: total_held=20=P. Нет жертв. Ждём.

t=40.01с service_api завершает транзакцию. RETURN. held=13. total_held=19.
        SCHEDULE: analytics ждёт. total_held < P → CREATE(analytics). held=1.

t=40.05с Ещё два возврата service_api.
        analytics: held=3. Все три коннекта СВЕЖИЕ (возраст < 1с).

t=45с   service_api запрашивает 3 коннекта.
        FIND_EVICTABLE(weight=100):
          analytics: 3 сверх гарантии, weight=10 < 100
          НО возраст = 5с < min_lifetime = 30с → ЗАЩИЩЁН!
          Жертв нет.
        service_api ждёт.

        ╔═══════════════════════════════════════════════════════════╗
        ║ Защита от флапа: коннекты analytics слишком молоды для   ║
        ║ вытеснения. service_api ждёт естественных возвратов      ║
        ║ или пока коннекты analytics не достигнут возраста 30с.   ║
        ╚═══════════════════════════════════════════════════════════╝

t=70с   Коннекты analytics достигли 30с. min_lifetime пройден.
        Если service_api всё ещё ждёт:
          FIND_EVICTABLE(weight=100): analytics теперь можно вытеснить.
          Вытеснение проходит.
```

---

## Конфигурация

```toml
[pools.mydb.auth_query]
# Жёсткий лимит на серверные коннекты к PostgreSQL
max_db_connections = 50

# Защита от флапа: минимальный возраст коннекта для вытеснения
min_connection_lifetime = 30000   # мс, default 30с

# Значения по умолчанию для auth_query пользователей
default_guaranteed_pool_size = 0
default_weight = 100
default_max_pool_size = 5

# Переопределения для конкретных пользователей
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

**Валидация:**
1. `sum(guaranteed_pool_size для всех пользователей) <= max_db_connections`
2. `для каждого: guaranteed_pool_size <= max_pool_size`
3. `для каждого: max_pool_size <= max_db_connections`
4. `min_connection_lifetime > 0`

---

## Dedicated vs Passthrough

**Dedicated mode** (все пользователи используют один PG server_user): коннекты взаимозаменяемы.
EVICT = переназначение другому пользователю (RESET ROLE уже выполнен при checkin). Стоимость: 0 мс.

**Passthrough mode** (каждый аутентифицируется как он сам): коннекты не взаимозаменяемы.
EVICT = закрытие старого коннекта + открытие нового. Стоимость: ~100 мс (один fork в PostgreSQL).

Алгоритм идентичен в обоих режимах. Различается только стоимость вытеснения.
