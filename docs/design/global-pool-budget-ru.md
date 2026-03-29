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
При равном весе: побеждает тот, у кого больше запросов в очереди (выше давление).

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
         waiting[W])                   // больше запросов в очереди побеждает (выше давление)
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

**Рекомендация:** `sum(guaranteed) <= P * 0.8`. Оставлять минимум 20% бюджета
для конкуренции сверх гарантии. При `sum(guaranteed) = P` пользователи с `guaranteed=0`
не получат коннектов никогда.

---

## Граничные случаи (Edge Cases)

### Кто кого может вытеснить (только above-guarantee)

```
Запрашивающий →      service_api  batch_worker  analytics
Жертва ↓               (w=100)      (w=50)       (w=10)
──────────────────────────────────────────────────────────
service_api (w=100)      —            ❌           ❌
batch_worker (w=50)      ✅            —           ❌
analytics (w=10)         ✅            ✅            —
──────────────────────────────────────────────────────────
✅ = может вытеснить (weight жертвы < weight запрашивающего AND age >= min_lifetime)
❌ = не может (weight жертвы >= weight запрашивающего)

Гарантированные запросы (held < guaranteed) вытесняют ЛЮБОЙ above-guarantee
коннект вне зависимости от веса (трактуются как weight = ∞).
```

### EC-1: Новый пользователь с guaranteed=0, пул полон, равный вес

```
Состояние: total_held=20=P. Все коннекты принадлежат пользователям с weight=100.

new_app (guaranteed=0, weight=100, max=5) запрашивает коннект.
  FIND_EVICTABLE(weight=100):
    Все above-guarantee коннекты имеют weight=100. 100 < 100? НЕТ.
    Жертв нет.

  new_app встаёт в очередь. Получит коннект при следующем RETURN.
  SELECT_BEST_WAITER: new_app (weight=100, waiting=1) конкурирует
  с возвращающим пользователем (если у того тоже есть ожидающие запросы).
  Тай-брейк: количество ожидающих запросов.
```

### EC-2: Новый пользователь с guaranteed=0, пул полон, вес ниже всех

```
new_app (guaranteed=0, weight=5) запрашивает коннект.
  Пул полон. FIND_EVICTABLE(weight=5): ни у кого weight < 5.
  new_app встаёт в очередь.

  При RETURN от любого пользователя:
    SELECT_BEST_WAITER среди всех ожидающих.
    Если service_api (weight=100) тоже ждёт → service_api побеждает.
    new_app получит коннект только когда НИ ОДИН пользователь с бо́льшим весом не ждёт.
```

### EC-3: Новый пользователь с guaranteed=2, пул полон

```
Состояние: total_held=20=P.
  service_api: held=12 (7 сверх гарантии), weight=100
  batch_worker: held=5 (2 сверх гарантии), weight=50
  analytics: held=3 (3 сверх гарантии), weight=10

new_service (guaranteed=2, weight=80) запрашивает первый коннект.
  held=0 < guaranteed=2 → НЕМЕДЛЕННО.
  Пул полон → FIND_EVICTABLE(weight=∞):
    Все 12 above-guarantee коннектов — кандидаты.
    Наименьший вес первым: analytics (weight=10).
    age >= min_lifetime? Если ДА → EVICT(analytics). CREATE(new_service).
    Если НЕТ (все коннекты < 30с) → new_service ждёт.

  После второго EVICT: new_service held=2 = guaranteed. Гарантия выполнена.
```

### EC-4: Все коннекты в гарантии, нет above-guarantee для вытеснения

```
P=8. service_api(guaranteed=5, held=5). batch_worker(guaranteed=3, held=3).
total_held=8=P. Все в гарантии.

analytics(guaranteed=0, weight=10) запрашивает коннект.
  Пул полон. FIND_EVICTABLE: above-guarantee коннектов нет.
  analytics встаёт в очередь.

  При RETURN(service_api): service_api held=4 < guaranteed=5.
    SELECT_BEST_WAITER:
      service_api: is_guaranteed=true (held=4 < guaranteed=5)
      analytics: is_guaranteed=false (held=0, но guaranteed=0)
    service_api побеждает (гарантированный > сверх гарантии).
    Коннект возвращается service_api.

  ⚠ analytics НИКОГДА не получит коннект в этой конфигурации.
  Корректное поведение: sum(guaranteed)=8=P, места нет.
```

### EC-5: Много динамических пользователей с guaranteed=0

```
P=20, 50 пользователей через auth_query, все: guaranteed=0, weight=100, max=5.

Первые 4 получают по 5 коннектов = 20. ПУЛ ПОЛОН.
Пользователи 5-50 встают в очередь.

При каждом RETURN: SELECT_BEST_WAITER среди 46 ожидающих.
  Все weight=100, guaranteed=false.
  Тай-брейк: количество ожидающих запросов.

При avg transaction=10мс и 20 коннектах:
  ~2000 возвратов/с → ~2000 выдач/с ожидающим.
  Все 50 пользователей делят 20 коннектов в round-robin.
  Эффективно: 0.4 коннекта на пользователя в среднем.
```

### EC-6: Динамические пользователи переполняют бюджет гарантий

```
default_guaranteed_pool_size = 1, P = 20
Статические: сумма guaranteed = 8
Динамические: 15 подключаются → 15 × 1 = 15
Итого guaranteed: 8 + 15 = 23 > P = 20. ИНВАРИАНТ НАРУШЕН.

Решение: runtime-проверка при подключении каждого динамического пользователя:

  fn can_grant_guarantee(new_user):
      current = sum(guaranteed[U] for U in active_users)
      return current + new_user.default_guaranteed <= P

  Если false: пользователь получает guaranteed=0 (без гарантии, конкурирует по весу).
```

### EC-7: min_lifetime=0 (защита отключена)

```
t=0.0с  analytics получает 5 коннектов
t=0.1с  service_api запрашивает → вытесняет analytics (weight 100 > 10)
t=0.2с  service_api снижает нагрузку → analytics получает коннекты обратно
t=0.3с  service_api снова запрашивает → вытесняет
...
Каждый цикл: ~100мс, один fork() в PostgreSQL.
10 циклов/с × fork() = деградация postmaster.

⚠ min_lifetime=0 вызывает флап коннектов. Не рекомендуется.
  Минимум: 5с. По умолчанию: 30с.
```

### EC-8: Гарантированный пользователь вытесняет above-guarantee

```
service_api: guaranteed=5, held=12 (7 сверх гарантии).
Все 7 above-guarantee коннектов старше 45с (прошли min_lifetime).

batch_worker запрашивает 5 коннектов (в гарантии: held=0 < guaranteed=3).
  НЕМЕДЛЕННО: FIND_EVICTABLE(weight=∞):
    service_api: 7 above-guarantee, age=45с > 30с → все evictable
    EVICT 3 (для guaranteed batch_worker). Потом ещё 2 above-guarantee.
    service_api: held=7 (2 сверх гарантии).
    batch_worker: held=5 (2 сверх гарантии).

  Гарантированные 5 коннектов service_api нетронуты.
  Вытеснены только above-guarantee.
```

### Сводная таблица

| # | Ситуация | Результат | Примечание |
|---|----------|-----------|------------|
| EC-1 | guaranteed=0, пул полон, равный вес | Ждёт RETURN | Тай-брейк по кол-ву ожидающих |
| EC-2 | guaranteed=0, пул полон, наименьший вес | Ждёт бесконечно | Получит, когда нет ожидающих с бо́льшим весом |
| EC-3 | guaranteed>0, пул полон | Вытесняет lowest-weight above-g | weight=∞ для guaranteed |
| EC-4 | sum(guaranteed)=P, нет above-g | Не получит коннект | Настроить sum(g) ≤ 80% P |
| EC-5 | 50 динамических, guaranteed=0 | Round-robin на P коннектах | Ожидаемое поведение |
| EC-6 | Динамические переполняют бюджет | Runtime-проверка, деградация до g=0 | Предотвращает нарушение инварианта |
| EC-7 | min_lifetime=0 | Флап/fork storm | Не рекомендуется, минимум 5с |
| EC-8 | Guaranteed вытесняет above-g | Только above-g затронуты | Guaranteed коннекты священны |

---

## Dedicated vs Passthrough

**Dedicated mode** (все пользователи используют один PG server_user): коннекты взаимозаменяемы.
EVICT = переназначение другому пользователю (RESET ROLE уже выполнен при checkin). Стоимость: 0 мс.

**Passthrough mode** (каждый аутентифицируется как он сам): коннекты не взаимозаменяемы.
EVICT = закрытие старого коннекта + открытие нового. Стоимость: ~100 мс (один fork в PostgreSQL).

Алгоритм идентичен в обоих режимах. Различается только стоимость вытеснения.

---

## Контракты интеграции

Требования для подключения BudgetController к pool layer.
Нарушение любого контракта приводит к дрифту held-счётчиков и потере бюджетной ёмкости.

### Контракт 1: Обнаружение мёртвых коннектов (CRITICAL)

При гибели серверного коннекта по ЛЮБОЙ причине ОБЯЗАН вызываться `release()`:

```
Причина                                Как pooler обнаруживает
───────────────────────────────────────────────────────────────
DBA выполнил pg_terminate_backend()     Проверка при recycle (следующий checkout)
PG убил по idle_in_transaction_         TCP read возвращает ошибку
  session_timeout / statement_timeout
TCP keepalive timeout                   ОС сообщает connection reset
PG restart / Patroni failover           Все коннекты падают одновременно
OOM-killed pod приложения               TCP FIN или RST (с задержкой)
```

Без вызова `release()` held[U] остаётся завышенным.
Фантомные слоты накапливаются до рестарта pooler'а.

**Реализация**: в каждом месте `pool/inner.rs`, где `slots.size` уменьшается
(failed recycle, retain removal, connection close), вызывать `budget.release()`.

### Контракт 2: Восстановление после failover (CRITICAL)

После failover PostgreSQL (Patroni promote, PG crash+restart) все серверные
коннекты мертвы одновременно. Budget controller нуждается в массовом сбросе:

```
fn reset_all(&self, now: Instant)
    // held=0 для всех пулов, очистить connection_ages, total_held=0
    // Прогнать drain для waiters (schedule сколько позволяет бюджет)
```

Вызывается pool layer'ом при обнаружении смены адреса сервера
или при провале всех health check'ов для server target.

Без этого: `total_held = P` после failover, никто не может подключиться.

### Контракт 3: Откат при неудаче CREATE (CRITICAL)

`try_acquire()` инкрементирует `held[U]` немедленно. Если реальное создание
PG-коннекта провалилось (max_connections, auth failure, network error),
вызывающий ОБЯЗАН вызвать `release()` для отката accounting'а.

Рекомендуемый паттерн — RAII guard:

```rust
let guard = budget.try_acquire(pool, now)?;  // инкрементирует held
match server_pool.create().await {
    Ok(conn) => {
        guard.confirm();  // held остаётся инкрементированным
        Ok(conn)
    }
    Err(e) => {
        drop(guard);      // held декрементируется автоматически
        Err(e)
    }
}
```

### Контракт 4: Периодическая сверка

Даже при соблюдении контрактов 1-3 дрифт счётчиков возможен
(race conditions, баги, edge cases). Фоновая задача сверяет
`held[U]` с реальным количеством живых коннектов:

```
каждые 60 секунд:
    для каждого пула U:
        actual = pool.slots.size  // реальные коннекты в пуле
        budget_held = budget.held(U)
        если budget_held != actual:
            log_warn("budget drift: pool={U} budget={budget_held} actual={actual}")
            budget.reconcile(U, actual)
```

`reconcile()` корректирует `held[U]` и `total_held`, затем вызывает
`schedule()` для обслуживания waiters, которые теперь могут получить слоты.

---

## Режимы отказа

### FM-1: Failover PostgreSQL (Patroni/Stolon)

```
t=0     Primary падает. Все TCP-коннекты разорваны.
t=1-30с Реплика промоутится. Новый primary принимает подключения.

БЕЗ reset_all():
  Budget: total_held=P. Все запросы → WouldBlock.
  Pool layer обнаруживает мёртвые коннекты при recycle (idle_timeout).
  Время восстановления: до idle_timeout (минуты). ПОЛНЫЙ OUTAGE.

С reset_all():
  Pool layer обнаруживает failover (смена DNS, connection refused).
  Вызывает budget.reset_all(). total_held=0.
  Все пулы переподключаются. Budget выдаёт до P коннектов.
  Время восстановления: ~1-5 секунд (fork time × параллельные создания).
```

### FM-2: Connection storm после рестарта

```
pg_doorman перезапускается (crash, rolling update). Budget state потерян.
total_held=0. 200 клиентов переподключаются одновременно.
Все try_acquire → Granted (до P).
P одновременных fork() к PG.

Защита:
  Существующий max_concurrent_creates (default 4 per pool) ограничивает параллельность.
  С budget controller'ом должен стать ГЛОБАЛЬНЫМ (не per-pool):
  max 10-20 параллельных создания суммарно по всем пулам.
```

### FM-3: Network partition

```
pg_doorman ↔ PG сеть разорвана. Budget позволяет новые acquire (total_held < P).
Pool layer пытается CREATE → TCP timeout (30с). Клиент ждёт 30с → timeout.
Budget held[U] инкрементирован при acquire.
CREATE проваливается → вызывается release() (Контракт 3) → held[U] декрементирован.

С RAII guard: безопасно. Без: held-счётчик завышен навсегда.
```

### FM-4: Long-running транзакции блокируют waiters

```
batch_worker запускает 20-минутный pg_dump. Держит 3 guaranteed коннекта.
Ни одного RETURN за 20 минут.
Waiters в очереди batch_worker зависают без timeout'а.

Защита: параметр max_wait_timeout (default: query_wait_timeout).
По истечении — ошибка вместо бесконечного ожидания.
```

---

## Multi-Instance деплой

Budget controller работает **per-instance** (shared-nothing). Каждый инстанс
pg_doorman имеет независимый бюджет. Координации между инстансами нет.

### Расчёт max_db_connections

```
P_per_instance = (PG_max_connections
                  - superuser_reserved_connections
                  - replication_slots
                  - monitoring_agents
                  - direct_dba_connections)
                 / количество_инстансов_pooler
```

Пример: PG max_connections=200, superuser_reserved=3, replication=2,
monitoring=2, DBA=3, 2 инстанса pg_doorman:

```
P = (200 - 3 - 2 - 2 - 3) / 2 = 95 на инстанс
```

### Отказ инстанса

При падении одного из N инстансов выжившие ограничены своим P,
а НЕ P × N. Доступная ёмкость PG недоиспользуется.

Обходной путь: задать P чуть выше расчётного и принять, что
при нормальной работе N × P может превысить PG capacity.
PG отклонит лишние коннекты (`FATAL: too many connections`).
Pool layer обработает через CREATE failure (Контракт 3).

Рекомендация: `P = расчётное_значение × 1.2` (20% запас для failover).

### Валидация при старте

При первом подключении к PG pooler должен проверить:

```sql
SELECT current_setting('max_connections')::int
     - current_setting('superuser_reserved_connections')::int AS available
```

Если `P > available`, логировать предупреждение.

---

## Применимость

### Где алгоритм подходит

- **Transaction pooling** с пользователями разного приоритета (API + batch + analytics)
- **P = 20-200**, 5-30 пулов (статические или динамические через auth_query)
- **Oversubscribed среды** где sum(desired) > max_connections
- **Multi-tenant SaaS** с per-tenant auth_query пулами

### Где алгоритм НЕ подходит

| Сценарий | Почему | Альтернатива |
|----------|--------|--------------|
| Session-mode pooling | Eviction убивает сессию клиента | Per-user pool_size |
| Все пользователи равны | Weight не добавляет ценности | Глобальный семафор |
| P < 5 | Overhead конфигурации | Фиксированные per-user лимиты |
| P > 500 | Риск Mutex contention | Партиционирование по сервису/БД |
| Один сервис, один пользователь | Нет контенции | Простой pool_size |

### Риск: budget controller ухудшает ситуацию

Контроллер может деградировать производительность по сравнению с независимыми пулами:

1. **min_lifetime deadlock**: пул полон, все коннекты молодые, guaranteed пользователь
   заблокирован до min_lifetime. Без контроллера PG принял бы коннект напрямую.

2. **Eviction cascades**: high-weight пользователь вызывает 10 вытеснений подряд.
   В passthrough — 10 close+open, деградация postmaster.

3. **Priority inversion**: high-weight пользователь заполнил пул, возвращает медленно.
   Low-weight с guaranteed=0 голодает. Без контроллера мог бы подключиться к PG напрямую.

**Митигация**: контроллер opt-in (выключен по умолчанию).

---

## Observability

### Обязательные Prometheus метрики

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

### Рекомендуемые алерты

| Алерт | Условие | Severity |
|-------|---------|----------|
| BudgetSaturated | total_held == max_connections более 2 мин | warning |
| BudgetWaitersStuck | waiting{pool} > 0 более 30с | warning |
| BudgetEvictionStorm | rate(evictions_total) > 5/с более 1 мин | warning |
| BudgetDrift | held{pool} != реальный размер пула более 60с | critical |

### Рекомендации по конфигурации

```
max_db_connections:
  Формула: (PG max_connections - reserves) / кол-во инстансов
  Типично: 50-100 на инстанс

min_connection_lifetime:
  Default: 30с
  Высокий churn: 10-15с
  Стабильная нагрузка: 60с
  Никогда: 0 (вызывает флап)

guaranteed_pool_size:
  Формула: ceil(p80_active_transactions × 0.8)
  Мониторинг: guaranteed = 2, max = 2
  Default для динамических: 0

weight:
  Critical API:     100
  Background jobs:   70
  Batch/ETL:         30
  Analytics:          10
  Monitoring:         50 (c guaranteed=2)
```
