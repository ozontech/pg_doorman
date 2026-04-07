# Давление на пул

Давление на пул (pool pressure) — это то, как pg_doorman обрабатывает
ситуацию, когда множество клиентов одновременно запрашивают серверное
соединение, а в idle-пуле пусто. Решение о том, кто получит соединение,
кто будет ждать, кто инициирует свежий backend-connect, а кому будет
отказано, принимают два механизма: пер-пуловые **anticipation + bounded
burst** внутри каждого пула `(database, user)` и кросс-пуловый
**coordinator**, ограничивающий общее число backend-соединений на базу
данных. Этот документ описывает оба механизма, порядок их срабатывания,
а также метрики и параметры для поддержания их в здоровом состоянии.

Аудитория: DBA или production-оператор, который уже знает PgBouncer и
хочет понять, чем pg_doorman отличается и за чем нужно следить.

## Зачем нужно давление на пул

Возьмём пул с `pool_size = 40` и нагрузкой в 200 коротких транзакций,
приходящих в одну и ту же миллисекунду. В пуле 4 idle-соединения. В
наивном пулере первые 4 клиента забирают idle-соединения, а оставшиеся
196 независимо вызывают `connect()` к PostgreSQL. PostgreSQL получает
196 одновременных TCP connect-попыток, на каждую из которых нужно
выполнить SCRAM-аутентификацию и согласование параметров, только чтобы
обнаружить, что пул разрешает ещё 36 соединений. Backend-обращения к
`pg_authid` взлетают всплеском, потолок `max_connections` пробивается,
очередь `accept()` ядра насыщается, а tail latency уже подключённых
клиентов растёт, потому что postmaster PostgreSQL занят порождением
backend'ов вместо выполнения запросов. Это проблема **thundering herd**.

```
Time:  ----------------------------------------->

Client_1   -[idle hit]--[query]-----[done]
Client_2   -[idle hit]--[query]-----[done]
Client_3   -[idle hit]--[query]-----[done]
Client_4   -[idle hit]--[query]-----[done]
Client_5   -[connect]-[auth]-[query]-[done]
Client_6   -[connect]-[auth]-[query]-[done]
   .             ^
   .             196 backend connect()s
   .             fired in the same instant
Client_200 -[connect]-[auth]-[query]-[done]

PostgreSQL: 196 spawning backends + 4 running queries
```

Давление на пул подавляет это поведение. pg_doorman заставляет
большинство из этих 196 вызовов переиспользовать соединение, которое
другой клиент вот-вот вернёт, либо подождать несколько миллисекунд за
небольшим числом in-flight backend-connect'ов. Частота `connect()` к
PostgreSQL остаётся ограниченной даже при всплесках клиентов.

## Plain pool mode

Этот режим работает, когда `max_db_connections` не задан. Пулы
независимы, кросс-пуловой координации нет, давление управляется внутри
каждого пула `(database, user)`. Это режим по умолчанию, и большинство
инсталляций находятся именно в нём.

### Прогрев пула с холодного старта

Пул с `pool_size = 40` и `min_pool_size = 0` стартует с нулём
соединений. Первый пришедший клиент не ждёт: pg_doorman сразу создаёт
backend-соединение. Второй делает то же самое, третий — то же самое,
пока пул не достигнет **порога прогрева** (warm threshold).

Порог прогрева равен `pool_size × scaling_warm_pool_ratio / 100`. При
дефолтном значении 20% и `pool_size = 40` порог равен 8 соединениям.
Ниже этого порога pg_doorman создаёт соединения без раздумий: пул
холодный, цена ожидания выше цены коннекта, и клиенты не могут
конкурировать за idle-соединения, которых не существует.

Выше порога активируется **anticipation zone**. Когда клиент не находит
соединения в idle-пуле, pg_doorman сначала пытается перехватить
соединение, которое другой клиент вот-вот вернёт.

Третья зона накладывается поверх обеих: при любом размере пула, если
`inflight_creates` достигает `scaling_max_parallel_creates` (по
умолчанию 2), пул входит в **burst-capped state** для новых создаваемых
соединений. Дополнительные вызывающие ждут слот вне зависимости от
того, сколько idle-соединений существует.

```
                        Three pressure zones
                        --------------------

Pool size:  0 ----------- 8 ---------------------------- 40
            ^             ^                              ^
            |             |                              |
            |  WARM ZONE  |  ANTICIPATION ZONE           |
            |             |                              |
            |  size <     |  size >= warm_threshold      |
            |  warm_thr   |                              |
            |             |                              |
            |  Skip       |  Phase 3: fast spin          |
            |  phases 3   |  Phase 4: Notify wait        |
            |  and 4.     |   (<= scaling_max_           |
            |  Go straight|      anticipation_wait_ms)   |
            |  to phase 5 |  Then phase 5                |
            |  (burst gate|                              |
            |  + connect) |                              |

                  Burst-capped state (orthogonal)
                  -------------------------------

inflight_creates: 0 ---- 1 ---- 2 (= scaling_max_parallel_creates)
                                ^
                                |  At cap: any caller reaching the
                                |  burst gate waits on a Notify
                                |  for either an idle return or
                                |  a peer create completion.
```

Зоны warm/anticipation отслеживают *текущий размер пула*. Burst-capped
state отслеживает *одновременные backend-creates*. Пул может находиться
в anticipation zone и в burst-capped state одновременно — это типичная
ситуация под нагрузкой. Пул ниже порога прогрева тоже может упереться в
burst cap, если во время холодного заполнения одновременно приходит
много клиентов.

### Получение соединения

Когда клиент запрашивает соединение через `pool.get()`, pg_doorman
проходит по следующим фазам. Каждая фаза либо возвращает соединение,
либо передаёт управление следующей.

**Фаза 1 — горячий путь recycle.** Снимаем элемент с головы idle-очереди.
Если соединение там есть и проходит recycle-проверку (rollback,
валидность, эпоха) — возвращаем его. Здоровый steady-state пул идёт
только этим путём. Стоимость: захват мьютекса и recycle-проверка.

**Фаза 2 — гейт warm zone.** Если размер пула ниже порога прогрева,
пропускаем anticipation и сразу переходим к созданию нового
backend-соединения. Холодные пулы заполняются быстро.

**Фаза 3 — anticipation spin.** Выше порога прогрева повторяем
recycle 10 раз в плотном цикле `yield_now` (контролируется параметром
`scaling_fast_retries`). Так перехватывается случай, когда другой
клиент завершил свой запрос в том же микросекундном диапазоне и
вот-вот вернёт соединение. Полная стоимость — порядка 10–50
микросекунд. Без sleep, без блокирующего I/O.

**Фаза 4 — anticipation wait.** Если spin не поймал возврат,
регистрируем `Notify` future, который просыпается, когда *любой*
клиент возвращает соединение. Ждём этот future с границей:

- `scaling_max_anticipation_wait_ms` (по умолчанию 100 ms), и
- половина оставшегося бюджета клиента `query_wait_timeout`.

Используется меньшее из двух значений, с минимальным полом в 1 ms,
чтобы wait успел зарегистрироваться. Если возврат происходит во время
ожидания, просыпается ровно **одна** задача, а не все сразу. Если
ожидание истекает без возврата, переходим к фазе 5.

**Фаза 5 — bounded burst gate.** Пытаемся забрать один из
`scaling_max_parallel_creates` слотов (по умолчанию 2) для in-flight
backend-connect'ов. Если слот свободен, забираем его и вызываем
`connect()` к PostgreSQL. Если все слоты заняты, ждём `Notify`, который
будит либо возврат idle-соединения, либо завершение чужого in-flight
create, а затем повторяем recycle и гейт. 5 ms backoff работает как
страховка на случай, если оба источника пробуждения пропущены.

**Фаза 6 — backend connect.** Запускаем `connect()`, аутентифицируемся,
отдаём соединение клиенту. Burst slot освобождается автоматически по
завершении этой фазы независимо от успеха или ошибки.

```
                  Plain mode acquisition flow
                  ---------------------------

   pool.get()
       |
       v
   +--------------+
   |  Phase 1:    |  --- HIT ----> return idle connection
   |  recycle pop |
   +------+-------+
          | MISS
          v
   +--------------+
   |  Phase 2:    |  --- below warm ---> jump to phase 5
   |  warm gate   |
   +------+-------+
          | above warm
          v
   +--------------+
   |  Phase 3:    |  --- HIT ----> return idle connection
   |  fast spin   |
   +------+-------+
          | MISS
          v
   +--------------+
   |  Phase 4:    |  --- HIT     ----> return idle connection
   |  anticipate  |  --- notify  ----> return idle connection
   |  Notify wait |  --- timeout ----> fall through
   +------+-------+
          |
          v
   +--------------+
   |  Phase 5:    |  --- slot taken --> proceed to phase 6
   |  burst gate  |  --- slot full  --> wait, retry recycle
   +------+-------+
          |
          v
   +--------------+
   |  Phase 6:    |
   |  connect()   | ----> return new connection
   +--------------+
```

### Подавление всплеска в действии

Тот же сценарий с 200 клиентами в формате thundering herd, но теперь
в plain mode и с `scaling_max_parallel_creates = 2`:

```
Time:   t=0ms     t=5ms    t=10ms   t=15ms   t=20ms   t=25ms

C_1     [idle]--[query]-[done]
C_2     [idle]--[query]-[done]
C_3     [idle]--[query]-[done]
C_4     [idle]--[query]-[done]
C_5     [spin/wait]------[recycled C_1]--[query]-[done]
C_6     [spin/wait]------[recycled C_2]--[query]-[done]
C_7     [gate=1]-[connect]----[auth]--[query]-[done]
C_8     [gate=2]-[connect]----[auth]--[query]-[done]
C_9     [gate full, wait]---[recycled C_3]--[query]
C_10    [gate full, wait]---[recycled C_4]--[query]
  .
  .     [...196 clients use a mix of recycle, anticipation, and at
  .      most 2 in-flight connects...]
  .
C_200   [gate=2]-[connect]--[auth]--[query]--[done]

PostgreSQL: at most 2 spawning backends at any moment
            + the 4 connections that were already there
```

Тот же пул обслуживает все 200 клиентов, но PostgreSQL никогда не видит
больше `scaling_max_parallel_creates` (по умолчанию 2) одновременных
backend-spawn'ов из этого пула. Большинство клиентов попадают на
переиспользованное соединение от соседа, который завершил работу
мгновением раньше, а не на свежий `connect()`.

### Неблокирующий чекаут

Когда клиент устанавливает `query_wait_timeout = 0`, он просит либо
немедленный idle hit, либо свежий connect, без ожиданий. Фаза
anticipation и ожидание burst gate пропускаются. pg_doorman выполняет
recycle горячего пути, один раз пробует burst gate, затем либо создаёт
соединение, либо возвращает ошибку wait timeout.

**Ограничение при включённом coordinator.** Non-blocking пропускает
только anticipation и burst-gate ожидания внутри пер-пулового пути.
Если `max_db_connections` сконфигурирован и фазы ожидания координатора
(B–D) занимают время, non-blocking-вызов всё равно блокируется внутри
`coordinator.acquire()` до `reserve_pool_timeout` (по умолчанию 3000
ms) перед возвратом. Для строгого zero-wait дедлайна на базах под
координатором установите `reserve_pool_timeout` достаточно низко, чтобы
он умещался в ваш бюджет.

### Фоновый replenish

Когда задан `min_pool_size`, фоновая задача периодически дополняет пул
до его минимума. Она использует тот же burst gate, что и клиентский
трафик. **Эта задача не встаёт в очередь** за занятым гейтом: при
попадании на занятый гейт она немедленно сдаётся и повторяет попытку
на следующем retain-цикле (по умолчанию каждые 30 секунд, контролируется
параметром `retain_connections_time`).

Логика такова: во время всплеска нагрузки клиенты уже насыщают гейт,
создавая соединения, которые им нужны *прямо сейчас*. Задача
replenish, борющаяся с ними за слоты, ничего не даёт; client-driven
creates всё равно поднимут пул выше `min_pool_size`. Счётчик
`replenish_deferred` инкрементируется при каждом таком отступлении
фоновой задачи.

Следствие: `min_pool_size` под нагрузкой — best-effort. Жёсткий пол
описан в разделе troubleshooting.

## Размер cap'а относительно PostgreSQL

Перед чтением про координатор проверьте, что worst-case число
backend-соединений умещается в PostgreSQL. Без `max_db_connections`
worst case для одной базы:

```
N pools (users) × pool_size  =  ceiling on backend connections
```

Пример с расчётом: три пула, у каждого `pool_size = 40`, без
`max_db_connections`. Worst case — **120 одновременных backend-соединений**
к этой базе, ограничиваемых только `scaling_max_parallel_creates` на
пул (по умолчанию 2 на каждый, то есть до 6 одновременных вызовов
`connect()` в полёте). Если PostgreSQL сконфигурирован с
`max_connections = 100`, база отказывает в новых соединениях во время
общего всплеска нагрузки и клиенты получают `FATAL: too many connections`.

Два решения:

- Понизить `pool_size` так, чтобы `N × pool_size` укладывалось ниже
  `max_connections` с запасом на `superuser_reserved_connections`,
  слоты репликации и любые прямые коннекторы, которые обходят
  pg_doorman.
- Установить `max_db_connections` для жёсткого cap'а (см. следующий
  раздел).

Эмпирическое правило: держите совокупную потребность pg_doorman не
выше 80% от `PostgreSQL max_connections - superuser_reserved_connections`.
Оставшиеся 20% — запас под admin-соединения, репликацию и всплески.

## Coordinator mode

Режим координатора активируется, когда у пула задан `max_db_connections`.
Он добавляет второй слой давления **поверх** пер-пулового: разделяемый
семафор, который ограничивает общее число backend-соединений к базе
суммарно по всем user-пулам, обслуживающим её. Без него потолок
`N × pool_size` из предыдущего раздела — единственное ограничение. С
`max_db_connections = 80` одновременно может существовать только 80
соединений независимо от конфигурации пулов, и координатор решает,
какие пулы могут расти.

При `max_db_connections = 0` (по умолчанию) координатора не существует.
Когда параметр задан, все механизмы plain mode, описанные выше,
по-прежнему работают; координатор добавляет один шаг получения permit
на пути новой связи. Переиспользование idle никогда не касается
координатора.

### Что добавляет координатор

Три вещи:

1. **Жёсткий cap** на общее число соединений на базу. Если 80 уже
   используются, 81-й запрос ждёт или падает независимо от того, какой
   пул его подаёт.

2. **Eviction.** Когда cap достигнут и новому пулу нужен слот,
   координатор может закрыть idle-соединение из пула другого
   пользователя, чтобы освободить слот. Выселяемый пул теряет
   соединение; запрашивающий пул получает его. Это честно: пользователи
   с наибольшим излишком над их **effective minimum (эффективный
   минимум)** теряют соединения первыми, причём только соединения
   старше `min_connection_lifetime` (по умолчанию 5000 ms) попадают в
   список кандидатов.

   **Эффективный минимум** для user-пула равен
   `max(user.min_pool_size, pool.min_guaranteed_pool_size)`. Оба
   параметра защищают соединения от eviction; побеждает больший. Если
   снизить любой из них, пол падает.

3. **Reserve pool.** Если cap достигнут, eviction ничего не дал, и
   ожидание возврата истекло, координатор может выдать permit из
   **резерва** — небольшого дополнительного пула поверх
   `max_db_connections`. Резерв ограничен `reserve_pool_size` (по
   умолчанию 0, что означает выключено) и приоритизирован: голодающие
   пользователи (те, кто ниже своего **эффективного минимума**) и
   пользователи с большим числом ожидающих клиентов обслуживаются
   первыми.

### Фазы получения permit'а в координаторе

Когда пер-пуловой путь доходит до шага создания нового соединения,
координатор проходит пять фаз. Первая фаза, выдавшая permit, завершает
последовательность.

**Фаза A — Try-acquire.** Неблокирующий захват семафора. Если cap не
достигнут, забираем слот и возвращаемся.

**Фаза B — Eviction.** Обходим все *остальные* user-пулы той же базы,
ищем тот, у кого наибольший излишек над его **эффективным минимумом**,
и закрываем одно из его idle-соединений старше
`min_connection_lifetime`. Permit выселяемого соединения освобождается
синхронно, освобождая слот. Повторяем захват семафора. Если два
вызова конкурируют, проигравший идёт к следующей фазе.

**Фаза C — Wait.** Регистрируем `Notify`, который просыпается, когда
любое используемое соединение возвращается в координатор. Ждём до
`reserve_pool_timeout` (по умолчанию 3000 ms) либо notify, либо
дедлайна. **Этот таймаут применяется даже при `reserve_pool_size = 0`**:
он задаёт бюджет фазы wait, а не только окно гейтинга для резерва. Если
ваш `query_wait_timeout` короче, чем `reserve_pool_timeout`, клиент
сдаётся первым, и вы видите ошибки `wait timeout` вместо более
диагностичной `all server connections to database 'X' are in use`. См.
troubleshooting для разбора симптома.

**Фаза D — Reserve.** Если ожидание истекло и `reserve_pool_size > 0`,
запрашиваем permit у reserve arbiter'а. Запросы оцениваются по
`(starving, queued_clients)`, чтобы пользователи, которым соединения
нужны больше всех, получали их первыми. Arbiter — это одна tokio-задача,
которая раздаёт reserve-permit'ы из приоритетной кучи.

**Фаза E — Error.** Если резерв исчерпан или не сконфигурирован,
клиент получает ошибку: `all server connections to database 'X' are in
use (max=N, ...)`.

### Почему координатор работает до burst gate

Внутри пер-пулового потока получения соединения coordinator-permit
захватывается **до** burst gate. Порядок выбран намеренно.

Координатор может ждать *секунды* (до `reserve_pool_timeout`, по
умолчанию 3000 ms). Burst gate просыпается за *миллисекунды*. Если бы
гейт шёл первым, два вызова в одном пуле могли бы захватить
единственные два слота, оба заблокировались бы на координаторе на
секунды в ожидании возврата от соседнего пула, а остальные клиенты в
их собственном пуле голодали бы в ожидании этих двух — при том что
самому пулу не нужно делать ничего, кроме `connect()`. Это
**head-of-line blocking внутри одного пула**.

С координатором первым гейт ограничивает **фактические вызовы
`connect()`**, а не *время ожидания на соседнем пуле*. Вызов,
заблокированный на coordinator wait, держит ноль burst-слотов. Гейт
видит максимум одного вызывающего на слот, причём каждый из них вот-вот
выпустит `connect()`.

```
        Coordinator + plain mode acquisition flow
        -----------------------------------------

   pool.get()
       |
       v
   Phase 1: hot path recycle   --- HIT ---> return
       | MISS
       v
   Phase 2: warm gate          --- below ---+
       | above warm                         |
       v                                    |
   Phase 3: fast spin          --- HIT ---> return
       | MISS                               |
       v                                    |
   Phase 4: anticipation wait  --- HIT ---> return
       | timeout                            |
       v                                    |
       | <----------------------------------+
       v
   +----------------------+
   |  Coordinator acquire |   <-- inserted between phase 4 and phase 5
   |   A: try_acquire     |       only when max_db_connections > 0
   |   B: evict from peer |
   |   C: wait for return |   up to reserve_pool_timeout
   |   D: reserve permit  |   scored priority
   |   E: error           |   client gets DB exhausted error
   +----------+-----------+
              | permit granted
              v
   Phase 5: bounded burst gate (scaling_max_parallel_creates)
              | slot acquired
              v
   Phase 6: server_pool.create()
              |
              v
              return new connection
```

Фазы пронумерованы так же, как в plain mode. Coordinator acquire — это
**не** нумерованная фаза: это отдельный гейт, вставленный между фазой 4
и фазой 5, когда `max_db_connections > 0`. В plain mode он не работает.

### Когда координатор сконфигурирован, но cap не достигнут

Если `max_db_connections = 80`, а текущее использование — 30, фаза A
координатора всегда успешна. Фазы B–E никогда не запускаются. Поведение
идентично plain mode плюс одна атомарная инкрементация семафора на
каждое новое соединение. Горячий путь (idle reuse) вообще не касается
координатора, поэтому там у него нет измеримой стоимости. Платят только
*новые* создания соединений, и платят ровно длительностью одной
атомарной операции.

По устройству координатор — это *cap*, а не *очередь*: он стоит вам
ресурсов только когда вы упираетесь в лимит.

### Фоновый replenish под координатором

`replenish` получает свой permit координатора через `try_acquire`
(неблокирующий). Если база в cap'е, replenish сдаётся и повторяет
попытку на следующем retain-цикле. Та же логика, что и backoff burst
gate: фоновая задача не должна бороться с клиентским трафиком за
скудные permit'ы.

## Параметры тюнинга

Все четыре scaling-параметра по умолчанию глобальные, с пер-пуловыми
оверрайдами для `scaling_warm_pool_ratio` и `scaling_fast_retries`.
Два параметра anticipation/burst — только глобальные; пер-пуловые
оверрайды не поддерживаются.

| Параметр | По умолчанию | Где | Что делает |
|---|---|---|---|
| `scaling_warm_pool_ratio` | `20` (процент) | `general`, per-pool | Порог, ниже которого соединения создаются без anticipation. Ниже `pool_size × ratio / 100` каждый запрос нового соединения идёт сразу к `connect()`. |
| `scaling_fast_retries` | `10` | `general`, per-pool | Число `yield_now`-spin retry в фазе anticipation перед переходом к event-driven ожиданию. |
| `scaling_max_anticipation_wait_ms` | `100` (ms) | `general` | Верхняя граница event-driven ожидания возврата idle-соединения перед переходом к backend-connect. Ограничивается половиной оставшегося `query_wait_timeout` клиента. |
| `scaling_max_parallel_creates` | `2` | `general` | Жёсткий cap на одновременные backend `connect()` на пул. Задачи сверх cap'а ждут возврата idle или завершения чужого create. Должен быть `>= 1`. |
| `max_db_connections` | не задан (выключено) | per-pool | Cap на общее число backend-соединений к базе суммарно по всем user-пулам. Когда не задан, координатор не существует. |
| `min_connection_lifetime` | `5000` (ms) | per-pool | Минимальный возраст idle-соединения, после которого координатор может выселить его в пользу другого пула. Нижняя граница на churn соединений. |
| `reserve_pool_size` | `0` (выключено) | per-pool | Дополнительные permit'ы координатора поверх `max_db_connections`, выдаваемые по приоритету при исчерпании основного пула. |
| `reserve_pool_timeout` | `3000` (ms) | per-pool | Максимальное время ожидания координатора перед переходом к reserve pool. |
| `min_guaranteed_pool_size` | `0` | per-pool | Пер-юзерный минимум, защищённый от eviction координатором. У пользователя с `current_size <= min_guaranteed_pool_size` соединения иммунны к eviction со стороны других пользователей. |

### Когда повышать `scaling_max_parallel_creates`

Повышайте, если:

- `burst_gate_waits` стабильно растёт между скрейпами и
  `replenish_deferred` тоже ненулевой — клиентский трафик и фоновая
  задача оба борются за слоты, которых нет;
- backend-`connect()` быстрый (< 50 ms) и у PostgreSQL есть запас по
  `max_connections`;
- скачки задержки соединения коррелируют с ростом частоты
  `burst_gate_waits`.

**Жёсткий потолок.** Никогда не поднимайте `scaling_max_parallel_creates`
выше любого из этих лимитов:

- `pool_size / 4` для самого маленького пула, использующего этот
  параметр. Выше — cap теряет смысл: половина пула может одновременно
  быть в полёте, что разрушает сглаживание.
- `(PostgreSQL max_connections - superuser_reserved_connections) / (10 × N pools)`,
  где `N pools` — все пулы, делящие этот инстанс PostgreSQL. Выше —
  совокупная частота одновременных коннектов превышает то, что бэкенд
  может поглотить без переполнения очереди `accept()`.

Понижайте, если:

- PostgreSQL `connect()` дорогой (> 200 ms — например, SSL с проверкой
  сертификата или медленный lookup `pg_authid`);
- в логах PostgreSQL появляется конкуренция за `pg_authid`;
- бэкенд показывает переполнение очереди `accept()`.

Симптом слишком низкого значения: частота `burst_gate_waits` растёт
быстрее, чем частота прихода клиентов. Симптом слишком высокого:
задержка PostgreSQL `connect()` растёт, а connection storm возвращается.

**Размер для множества пулов.** Совокупный потолок одновременных
коннектов — `N pools × scaling_max_parallel_creates`. Если у вас один
PostgreSQL за 10 пулами и вам нужно не более 8 одновременных
backend-коннектов суммарно по ним всем в любой момент, поставьте
`scaling_max_parallel_creates` в значение около `desired_aggregate / N pools`,
округляя вниз. Ниже 1 не допускается; если арифметика даёт <1,
уменьшайте `N pools`, консолидируя пользователей.

### Когда повышать `scaling_max_anticipation_wait_ms`

Повышайте, если:

- `anticipation_wakes_timeout` сильно больше, чем
  `anticipation_wakes_notify`, и пул *не* недоразмерен (большинство
  запросов завершаются быстрее 100 ms, но бюджет anticipation слишком
  короткий, чтобы их поймать);
- скачки latency p99 коррелируют с частотой `create_fallback`.

Понижайте, если:

- `query_wait_timeout` короткий (< 200 ms) и вы не можете позволить
  себе сжечь 100 ms на anticipation;
- `anticipation_wakes_notify` высокий (оптимистичный путь работает),
  но отдельные клиенты видят, как ожидание раздувает их tail latency.

Симптом слишком низкого значения: `create_fallback` и
`anticipation_wakes_timeout` оба растут быстрее, чем это могло бы
оправдать создание соединений. Симптом слишком высокого: tail latency
клиентов содержит постоянный вклад от ожидания anticipation, когда пул
действительно недоразмерен.

### Когда повышать `scaling_warm_pool_ratio`

Повышайте, если:

- пулы медленно прогреваются на старте, и `min_pool_size` не
  используется;
- клиенты ждут anticipation, когда пул в основном пуст (anticipation
  активируется только выше порога прогрева, поэтому такого быть не
  должно, но высокий ratio сужает окно, в котором это *может*
  произойти).

Понижайте, если:

- пулы переразмерены и вы хотите, чтобы anticipation подавляла
  создания раньше в диапазоне размеров.

Этот параметр редко требует вмешательства. Дефолт 20% работает для
большинства нагрузок.

### Когда задавать `max_db_connections`

Задавайте, если:

- один хост PostgreSQL обслуживает несколько пулов
  `(database, user)`, и сумма `pool_size` по всем пулам превышает
  `max_connections` базы;
- нужен жёсткий потолок, который выживет при неправильной конфигурации
  любого отдельного пула;
- нужна кросс-пуловая честность через eviction.

Оставляйте незаданным, если:

- один пул обслуживает одну базу, и `pool_size` — единственный
  параметр;
- вы не хотите никакой кросс-пуловой eviction (некоторые нагрузки
  предпочитают жёсткую пер-юзерную изоляцию).

### `reserve_pool_size` и `reserve_pool_timeout`

Резерв — это *временный клапан переполнения*, а не дополнительная
steady-state ёмкость. Он предотвращает видимые клиенту ошибки
исчерпания во время коротких всплесков. В нормальном режиме работы
`reserve_in_use` должен быть равен 0 большую часть времени.

Эмпирическое правило размера: `reserve_pool_size ≤ 0.25 × max_db_connections`.
Резерв впитывает всплеск; он не удваивает cap.

`reserve_pool_timeout` — это сколько клиент ждёт в фазе C координатора
перед обращением к резерву. Дефолт 3000 ms — консервативный. Понижайте
его, если ваш `query_wait_timeout` короткий и вы предпочитаете быстро
переходить к резерву, а не блокировать клиентов на coordinator wait.

**Пол.** Никогда не опускайте `reserve_pool_timeout` ниже, чем
`2 × ваш p99 query latency`. Ниже этого пола фаза wait всегда
истекает раньше, чем сосед возвращает соединение, и резерв превращается
в обязательный permit для каждого нового соединения, а не в клапан
переполнения. Reserve-permit'ы скудны по замыслу; использование их как
steady state перечёркивает их назначение.

**Ловушка: `query_wait_timeout < reserve_pool_timeout`.** Когда дедлайн
клиента короче фазы ожидания координатора, клиент сдаётся первым, и вы
видите ошибки `wait timeout` вместо более диагностичной
`all server connections to database 'X' are in use`. Фазы wait и
reserve координатора отрабатывают полностью, но не остаётся клиента,
который мог бы получить результат. Валидатор конфига pg_doorman выдаёт
предупреждение на старте; реагируйте на него.

## Observability

pg_doorman экспортирует состояние давления на пул через admin-консоль и
через Prometheus. Оба показывают одни и те же счётчики; выбирайте то,
что подходит вашему стеку мониторинга.

### Admin: `SHOW POOL_SCALING`

Пер-пуловые счётчики для пути anticipation + bounded burst.
Подключитесь к admin-базе `pgdoorman` и выполните:

```sql
pgdoorman=> SHOW POOL_SCALING;
```

| Колонка | Тип | Значение |
|---|---|---|
| `user` | text | Пользователь пула |
| `database` | text | База пула |
| `inflight` | gauge | Backend `connect()` вызовы, прямо сейчас идущие для этого пула. Ограничено `scaling_max_parallel_creates`. |
| `creates` | counter | Общее число backend-соединений, создание которых пул начинал с момента старта. Используется в паре с `gate_waits` для вычисления частоты попадания на гейт. |
| `gate_waits` | counter | Сколько раз вызывающий обнаружил burst gate в cap'е и был вынужден ждать на `Notify`. Высокие значения указывают, что `scaling_max_parallel_creates` слишком низкий. |
| `antic_notify` | counter | Anticipation-ожидания, проснувшиеся на реальном возврате idle. Оптимистичный путь окупился. |
| `antic_timeout` | counter | Anticipation-ожидания, перешедшие дальше по таймауту бюджета вместо ловли возврата. Соотношение к `antic_notify` показывает, хорошо ли откалиброван `scaling_max_anticipation_wait_ms`. |
| `create_fallback` | counter | Сколько раз anticipation завершилось, но `try_recycle` всё ещё нашёл пул пустым, вынудив свежий `connect()`. |
| `replenish_def` | counter | Запуски фонового replenish, упёршиеся в burst cap и отложенные до следующего retain-цикла. Стабильно ненулевые значения означают, что `min_pool_size` нельзя поддерживать при текущей нагрузке. |

Все счётчики монотонные с момента старта. Считайте дельты между
скрейпами; абсолютные значения полезны только для соотношений.

### Admin: `SHOW POOL_COORDINATOR`

Пер-базовое состояние координатора. Присутствует только для баз с
`max_db_connections > 0`.

```sql
pgdoorman=> SHOW POOL_COORDINATOR;
```

| Колонка | Тип | Значение |
|---|---|---|
| `database` | text | Имя базы |
| `max_db_conn` | gauge | Сконфигурированное `max_db_connections` |
| `current` | gauge | Общее число backend-соединений, удерживаемых под этим координатором (по всем user-пулам) |
| `reserve_size` | gauge | Сконфигурированное `reserve_pool_size` |
| `reserve_used` | gauge | Reserve-permit'ы, прямо сейчас в использовании |
| `evictions` | counter | Сколько раз координатор выселил idle-соединение из соседнего пула, чтобы освободить слот |
| `reserve_acq` | counter | Общее число reserve-permit'ов, выданных arbiter'ом |
| `exhaustions` | counter | Сколько раз координатор вернул клиенту ошибку исчерпания. **Это главный сигнал на пейджер.** |

### Метрики Prometheus

Два семейства метрик на пул и два на координатор. Все четыре используют
namespace'ы `pg_doorman_pool_scaling*` и `pg_doorman_pool_coordinator*`.

| Метрика | Тип | Лейблы | Источник |
|---|---|---|---|
| `pg_doorman_pool_scaling{type="inflight_creates"}` | gauge | `user`, `database` | `inflight` из `SHOW POOL_SCALING` |
| `pg_doorman_pool_scaling_total{type="creates_started"}` | counter | `user`, `database` | `creates` |
| `pg_doorman_pool_scaling_total{type="burst_gate_waits"}` | counter | `user`, `database` | `gate_waits` |
| `pg_doorman_pool_scaling_total{type="anticipation_wakes_notify"}` | counter | `user`, `database` | `antic_notify` |
| `pg_doorman_pool_scaling_total{type="anticipation_wakes_timeout"}` | counter | `user`, `database` | `antic_timeout` |
| `pg_doorman_pool_scaling_total{type="create_fallback"}` | counter | `user`, `database` | `create_fallback` |
| `pg_doorman_pool_scaling_total{type="replenish_deferred"}` | counter | `user`, `database` | `replenish_def` |
| `pg_doorman_pool_coordinator{type="connections"}` | gauge | `database` | `current` из `SHOW POOL_COORDINATOR` |
| `pg_doorman_pool_coordinator{type="reserve_in_use"}` | gauge | `database` | `reserve_used` |
| `pg_doorman_pool_coordinator{type="max_connections"}` | gauge | `database` | `max_db_conn` |
| `pg_doorman_pool_coordinator{type="reserve_pool_size"}` | gauge | `database` | `reserve_size` |
| `pg_doorman_pool_coordinator_total{type="evictions"}` | counter | `database` | `evictions` |
| `pg_doorman_pool_coordinator_total{type="reserve_acquisitions"}` | counter | `database` | `reserve_acq` |
| `pg_doorman_pool_coordinator_total{type="exhaustions"}` | counter | `database` | `exhaustions` |

### Алерты для настройки

Алерты ниже покрывают режимы отказа, заслуживающие пейджера или
варнинга. Они написаны на синтаксисе Prometheus; адаптируйте под свой
стек. Все используют окна устойчивого условия, чтобы короткие всплески
не будили on-call.

Если вы часто перезагружаете pg_doorman и пулы появляются и исчезают,
ограничьте алерты недавно активными пулами (например, добавьте
`pg_doorman_pool_scaling_total{type="creates_started"} > 0` как
гейтинг-фильтр).

**Coordinator exhaustion (page).** Клиент получил ошибку "database
exhausted". Жёсткий отказ.
**Runbook:** см. Troubleshooting → "`max_db_connections` exhausted".

```promql
rate(pg_doorman_pool_coordinator_total{type="exhaustions"}[5m]) > 0
```

**Burst gate saturated (warn).** Примерно половина попыток создания
новых соединений хотя бы раз вставала в очередь. Короткие всплески выше
0.5 во время failover или рестарта нормальны; устойчивые значения
означают, что `scaling_max_parallel_creates` слишком низкий для
предлагаемой нагрузки.

```promql
rate(pg_doorman_pool_scaling_total{type="burst_gate_waits"}[5m])
  > 0.5 * rate(pg_doorman_pool_scaling_total{type="creates_started"}[5m])
```

**Anticipation calibration drifting (warn).** Больше anticipation-ожиданий
завершается по таймауту, чем ловится реальным возвратом — намёк, что
`scaling_max_anticipation_wait_ms` ниже типичной задержки запроса.
**Действие:** поднимите `scaling_max_anticipation_wait_ms` примерно до
вашего p90 задержки запроса.

```promql
rate(pg_doorman_pool_scaling_total{type="anticipation_wakes_timeout"}[5m])
  > 2 * rate(pg_doorman_pool_scaling_total{type="anticipation_wakes_notify"}[5m])
```

**Replenish deferred persistently (warn).** Фоновая задача не может
поддерживать `min_pool_size`, потому что burst gate занят клиентским
трафиком. Условие удерживается в течение часа, не короткий всплеск.

```promql
increase(pg_doorman_pool_scaling_total{type="replenish_deferred"}[1h]) > 60
```

**Reserve pool continuously in use (warn).** Резерв создан для
коротких всплесков. Это правило срабатывает только когда резерв
**непрерывно** в использовании 15 минут, а не моментальное
использование.

```promql
min_over_time(pg_doorman_pool_coordinator{type="reserve_in_use"}[15m]) > 0
```

**Coordinator approaching cap (warn).** Запас времени до исчерпания.
Фильтр `> 0` предотвращает деление на ноль на базах, где координатор
выключен.

```promql
pg_doorman_pool_coordinator{type="max_connections"} > 0
  and
  pg_doorman_pool_coordinator{type="connections"}
    / pg_doorman_pool_coordinator{type="max_connections"} > 0.85
```

**Inflight stuck at cap (warn).** `inflight_creates`, сидящий на
сконфигурированном cap'е больше 5 минут, означает, что вызовы
`connect()` не завершаются. Проверьте здоровье бэкенда.

```promql
min_over_time(pg_doorman_pool_scaling{type="inflight_creates"}[5m])
  >= 2  # adjust to your scaling_max_parallel_creates value
```

**Coordinator thrashing (warn).** Cap полон *и* идут eviction'ы:
координатор постоянно закрывает соседние соединения, чтобы освободить
место. Пул недоразмерен для предлагаемой нагрузки, а не "иногда под
давлением".

```promql
pg_doorman_pool_coordinator{type="connections"}
    / pg_doorman_pool_coordinator{type="max_connections"} > 0.95
  and
  rate(pg_doorman_pool_coordinator_total{type="evictions"}[5m]) > 0
```

### Чтение admin-вывода во время инцидента

Admin-консоль принимает только `SHOW <subcommand>`, `SET`, `RELOAD`,
`SHUTDOWN`, `UPGRADE`, `PAUSE`, `RESUME` и `RECONNECT`. `SHOW` — не
виртуальная таблица, поэтому к admin-базе нельзя выполнить `SELECT`.
Чтобы запрашивать счётчики в shell-пайплайнах, запускайте `SHOW` из
`psql` и постобрабатывайте вывод.

Шаблоны ниже используют `psql` против admin-листенера (дефолтные
креды `admin/admin`):

```bash
# Highest burst-gate-wait ratio first (the hot pool).
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
     -c 'SHOW POOL_SCALING' --no-align --field-separator='|' \
  | awk -F'|' 'NR>1 && $4>0 { printf "%-20s %-20s %.3f  inflight=%d  defer=%d\n", $1, $2, $5/$4, $3, $9 }' \
  | sort -k3 -nr | head

# Highest anticipation miss ratio (timeout vs notify).
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
     -c 'SHOW POOL_SCALING' --no-align --field-separator='|' \
  | awk -F'|' 'NR>1 && ($6+$7)>0 { printf "%-20s %-20s %.3f  notify=%d  timeout=%d\n", $1, $2, $7/($6+$7), $6, $7 }' \
  | sort -k3 -nr | head

# Coordinator: closest databases to exhaustion.
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman \
     -c 'SHOW POOL_COORDINATOR' --no-align --field-separator='|' \
  | awk -F'|' 'NR>1 && $2>0 { printf "%-30s %.3f  used=%d/%d  reserve=%d  exhaustions=%d\n", $1, $3/$2, $3, $2, $5, $8 }' \
  | sort -k2 -nr
```

Позиции полей в `awk` соответствуют порядку колонок, описанному выше:
`POOL_SCALING` — это `user|database|inflight|creates|gate_waits|antic_notify|antic_timeout|create_fallback|replenish_def`,
`POOL_COORDINATOR` — это `database|max_db_conn|current|reserve_size|reserve_used|evictions|reserve_acq|exhaustions`.

## Сравнение с PgBouncer

PgBouncer и pg_doorman оба пулят, но обрабатывают давление по-разному.

| Аспект | PgBouncer | pg_doorman |
|---|---|---|
| Пер-пуловой cap размера | `pool_size` | `pool_size` |
| Кросс-пуловой cap на уровне БД | `max_db_connections` (жёсткий cap, без eviction; пер-базовые/пер-юзерные оверрайды `pool_size` для изоляции) | `max_db_connections` (жёсткий cap плюс кросс-пуловой eviction и reserve pool) |
| Reserve pool | `reserve_pool_size`, `reserve_pool_timeout` | `reserve_pool_size`, `reserve_pool_timeout` (плюс приоритизация в arbiter по starving/queued) |
| Eviction между пользователями | Не поддерживается. Пользователь, удерживающий idle-соединения, морит голодом соседа, которому они нужны. | Координатор выселяет idle-соединения у пользователя с наибольшим излишком над **эффективным минимумом** (`max(user.min_pool_size, min_guaranteed_pool_size)`). |
| Одновременные backend `connect()` на пул | Однопоточный, обрабатывает события последовательно на пул — вызовы `connect()` выпускаются по одному. | Ограничено `scaling_max_parallel_creates` (по умолчанию 2 на пул): до N одновременных backend-коннектов на пул, ограниченных против предлагаемой нагрузки. |
| Anticipation возвратов | Нет. Клиенты ждут на `wait_timeout` следующего доступного соединения в порядке прихода. | Event-driven anticipation: возвращающееся соединение будит ровно одного из ожидающих в очереди, часто до того, как выпускается какой-либо новый `connect()`. |
| Прогрев `min_pool_size` | Поддерживается на каждом такте event loop (без отдельной задачи replenish). | Периодический фоновый replenish (`retain_connections_time`, по умолчанию 30 s), который отступает, когда burst gate занят. |
| Backend login retry-after-failure | `server_login_retry` (по умолчанию 15 s) блокирует новые попытки логина после отказа бэкенда. | Аналога нет. Ошибки логина бэкенда пробрасываются клиенту на каждую попытку. |
| Lifetime jitter | Нет. `server_lifetime` точный. | ±20% jitter на `server_lifetime` и `idle_timeout`, чтобы избежать синхронного массового закрытия. |
| Ключ поиска пула | `(database, user, auth_type)` | `(database, user)` |
| Честность между пользователями на общем cap'е | First come first served на `max_db_connections`. | Reserve arbiter оценивает запросы по `(starving, queued_clients)`. |
| Observability давления на новые соединения | `SHOW POOLS`, `SHOW STATS`. Никакой видимости в in-flight коннекты или результаты anticipation. | `SHOW POOL_SCALING` и `SHOW POOL_COORDINATOR` показывают каждый счётчик, который использует новый кодовый путь. |

Два различия имеют наибольшее значение в production:

1. **Bounded burst gate.** Размер пула в PgBouncer ограничивает,
   сколько *соединений* у вас есть, но не ограничивает, сколько вызовов
   `connect()` выпускается одновременно при приходе многих клиентов в
   один момент. pg_doorman ограничивает частоту одновременных
   backend-`connect()` независимо от размера пула, поэтому внезапный
   всплеск трафика не превращается в connection storm против PostgreSQL.

2. **Cross-pool eviction.** `max_db_connections` в PgBouncer — это
   жёсткий потолок без способа перераспределения. Если пользователь A
   держит 80 idle-соединений, а пользователю B нужно одно, но cap
   достигнут, пользователь B ждёт или падает. Координатор pg_doorman
   может закрыть одно из соединений A (если оно старше
   `min_connection_lifetime`) и отдать слот B.

## Troubleshooting

### Несколько одновременных строк лога backend connect

**Симптом.** В логах сервера (или в debug-логах pg_doorman) видно 5
или больше backend `connect()` событий в одной миллисекунде, что
наводит на мысль, что burst gate не работает.

**Причина.** Либо `scaling_max_parallel_creates` установлен слишком
высоко (проверьте в `SHOW CONFIG` или вашем `pg_doorman.yaml`), либо
существует 5 или больше пулов, каждый из которых независимо выпускает
одновременные коннекты (гейт пер-пуловой, не глобальный).

**Исправление.** Понизьте `scaling_max_parallel_creates`. Дефолт 2
подходит большинству нагрузок. При множестве пулов *совокупная*
частота одновременных коннектов — это `pools × scaling_max_parallel_creates`,
что ожидаемо. Чтобы ограничить совокупность, задайте
`max_db_connections` на базу; координатор затем поставит в очередь
создания сверх cap'а.

### `min_pool_size` не поддерживается

**Симптом.** Пул с `min_pool_size = 10` показывает `sv_idle = 4` в
`SHOW POOLS` и держится так минутами.

**Причина.** Фоновый replenish откладывается, потому что burst gate
занят клиентским трафиком. Проверьте `replenish_def` в
`SHOW POOL_SCALING`. Если он продолжает расти, replenish пропускает
каждый retain-цикл.

**Исправление.** По замыслу под нагрузкой client-driven creates
владеют гейтом. Пул достигнет `min_pool_size`, когда клиентский трафик
ослабнет. Для жёсткого пола повышайте `scaling_max_parallel_creates`,
чтобы у replenish была свободная ёмкость, или сократите
`retain_connections_time`, чтобы replenish запускался чаще.

Для **transaction pooling** (`pool_mode = transaction`) задание
`min_pool_size` выше, чем `pool_size / 2`, обычно указывает на
недоразмеренный пул: большинство соединений должно быть доступно для
клиентских чекаутов, а не пришпилено к минимуму. Для **session pooling**
эвристика не применяется: `min_pool_size = pool_size` — легитимная
настройка для удержания всего session-scoped state в горячем виде.

### Latency p99 растёт без видимой причины

**Симптом.** p99 клиентской задержки растёт, p50 держится плоским.
Размер пула выглядит нормально, в логах нет ошибок.

**Причина.** Anticipation истекает по таймауту, и клиенты платят 100
ms ожидания поверх задержки своего запроса. Проверьте соотношение
`antic_timeout / antic_notify` в `SHOW POOL_SCALING`.

**Исправление.** Два случая.

- Если соотношение высокое (timeout > notify) и `create_fallback` тоже
  растёт: anticipation не справляется с ловлей возвратов. Либо
  поднимите `scaling_max_anticipation_wait_ms`, чтобы anticipation
  мог дольше ждать возврата, либо признайте, что пул недоразмерен, и
  поднимите `pool_size`.
- Если соотношение низкое (notify > timeout), но p99 всё равно высокий:
  пул в порядке, задержка где-то ещё (PostgreSQL, сеть, клиентская
  сторона). Проверьте `SHOW STATS avg_wait_time`, чтобы убедиться,
  что pg_doorman — не узкое место.

### `max_db_connections` исчерпан, клиенты получают ошибки

**Симптом.** Клиенты видят ошибки вроде `all server connections to
database 'X' are in use (max=80, ...)`.
`pg_doorman_pool_coordinator_total{type="exhaustions"}` растёт.

**Причина.** Все пять фаз координатора провалились: try-acquire
провалился, выселять нечего, ожидание истекло, а резерв либо исчерпан,
либо `reserve_pool_size = 0`.

**Исправление.** Пройдите по фазам по порядку.

1. Проверьте `current` против `max_db_conn` в `SHOW POOL_COORDINATOR`.
   Если `current` стабильно на cap'е, ваша предлагаемая нагрузка
   превышает cap. Либо поднимите `max_db_connections`, либо ищите
   разогнавшийся пул.
2. Проверьте частоту `evictions`. Если она нулевая или близка к нулю,
   eviction не помогает: idle-соединения каждого пула моложе
   `min_connection_lifetime` (по умолчанию 5000 ms), либо все
   остальные пулы находятся на своём `min_guaranteed_pool_size`.
   Понизьте `min_connection_lifetime`, если у вас очень короткие
   запросы, или увеличьте `max_db_connections`.
3. Проверьте `reserve_used` против `reserve_size`. Если резерв
   полностью занят, поднимите `reserve_pool_size`. Если он пустой, но
   `exhaustions` происходят, резерв не сконфигурирован
   (`reserve_pool_size = 0`). Задайте его, чтобы поглощать всплески.
4. Посмотрите `SHOW POOLS` для базы. Если у одного пользователя `sv_idle`
   намного больше, чем у других, этот пользователь копит соединения;
   рассмотрите `min_guaranteed_pool_size`, чтобы защитить меньших
   пользователей от того, чтобы их раздавили, либо понизьте `pool_size`
   накопителю.

### Фаза ожидания координатора — узкое место

**Симптом.** Клиенты в среднем платят 3 секунды задержки, ровно
совпадающие с `reserve_pool_timeout`.

**Причина.** Phase C wait стабильно истекает по таймауту. Либо база
действительно на cap'е и соединения не возвращаются, либо
`reserve_pool_size = 0`, поэтому wait отрабатывает до конца, прежде
чем клиент получит хоть какой-то ответ.

**Исправление.** Понизьте `reserve_pool_timeout` для быстрого отказа,
либо задайте `reserve_pool_size > 0`, чтобы фаза D обрабатывала
переполнение в рамках того же пути получения соединения.

### Burst gate — узкое место даже при низком трафике

**Симптом.** Частота `gate_waits` значительная, но частота `creates`
низкая, а `inflight_creates` непрерывно на cap'е.

**Причина.** Backend `connect()` медленный. Каждый create удерживает
слот секундами; даже с двумя слотами вы можете создать лишь около
`2 / connect_seconds` соединений в секунду.

**Исправление.** Разбирайтесь, почему `connect()` медленный со стороны
PostgreSQL (слишком много SCRAM-итераций, конкуренция за блокировки
`pg_authid`, медленный DNS, SSL handshake). Когда `connect()` станет
быстрым, гейт перестанет быть узким местом. Поднятие
`scaling_max_parallel_creates` маскирует проблему и переносит storm на
PostgreSQL. Сначала разбирайтесь, потом поднимайте cap.

### `is_starving`-пользователи постоянно получают reserve permits

**Симптом.** `reserve_acquisitions_total` продолжает расти. Один и тот
же небольшой пользователь получает большинство резервов.

**Причина.** Пользователь ниже своего **эффективного минимума**
(`max(user.min_pool_size, min_guaranteed_pool_size)`), и координатор
не может удовлетворить этот минимум без выселения от соседей. Каждый
клиентский запрос от этого пользователя попадает в фазу D координатора
и захватывает резерв. **Более глубокий вопрос — почему пользователь
постоянно нуждается в свежих соединениях**: либо его `pool_size`
слишком низкий, чтобы поглотить собственную нагрузку, либо его трафик
всплесковый, и резерв делает то, для чего резервы и нужны.

**Исправление.** Три варианта, выбор зависит от глубинной причины:

- Если `pool_size` пользователя действительно слишком мал для
  steady-state нагрузки, поднимите `pool_size` и (если нужно)
  `max_db_connections`, чтобы больший пул вместился.
- Если у пользователя высокий эффективный минимум, который
  координатор не может удовлетворить, понизьте **тот параметр,
  который реально задаёт пол** (проверьте оба:
  `user.min_pool_size` и `min_guaranteed_pool_size`).
- Если трафик действительно всплесковый, и резервы ловят всплески,
  оставьте как есть. Краткое использование резерва — это и есть замысел.

### Клиенты получают `wait timeout`, а не `database exhausted`

**Симптом.** Под давлением координатора клиенты видят
`PoolError::Timeout(Wait)`, но
`pg_doorman_pool_coordinator_total{type="exhaustions"}` остаётся на
нуле. Координатор так и не объявил исчерпание, но каждый клиент уходит
по таймауту.

**Причина.** `query_wait_timeout` короче, чем `reserve_pool_timeout`.
Клиент сдаётся раньше, чем фаза ожидания координатора завершается.
Счётчик `exhaustions` никогда не инкрементируется, потому что
координатор в итоге получает permit для запроса, у которого больше нет
ожидающего клиента.

**Исправление.** Либо поднимите `query_wait_timeout` выше
`reserve_pool_timeout` плюс типичное время `connect()`, либо понизьте
`reserve_pool_timeout` (в пределах пола, отмеченного в разделе
тюнинга). Валидатор конфига на старте выдаёт предупреждение для такой
конфигурации; реагируйте на него.

### PostgreSQL перезапустили, что дальше

**Симптом.** Мастер PostgreSQL перезапустился (failover, краш,
плановое). Видна толпа клиентов, бьющих в burst gate, `inflight_creates`
сидит на cap'е, частота `creates_started` резко растёт.

**Причина.** Когда pg_doorman обнаруживает непригодный бэкенд (через
`server_idle_check_timeout` или провалившийся запрос), он бампит
reconnect-эпоху пула и сразу сливает все idle-соединения. Каждый
клиент, пришедший после слива, проходит мимо горячего пути и идёт по
маршруту anticipation → burst gate → connect. С
`scaling_max_parallel_creates = 2` пул дополняется максимум 2
соединениями за раз на пул, ограниченных задержкой `connect()` к
PostgreSQL.

**Как выглядит здоровое восстановление.** `inflight_creates = 2`
непрерывно в первые несколько секунд, частота `creates_started` быстро
растёт, частота `burst_gate_waits` растёт в ногу,
`anticipation_wakes_notify` быстро обгоняет `anticipation_wakes_timeout`
по мере того, как новые соединения начинают циркулировать. В пределах
`pool_size / 2` × `connect()` секунд пул возвращается в норму.

**Исправление.** Обычно никакого. Bounded burst gate делает свою
работу, предотвращая connection storm против восстанавливающегося
primary. Если `connect()` действительно быстрый (< 50 ms), и у вашего
`max_connections` есть запас, поднимите `scaling_max_parallel_creates`
до 4 или 8, чтобы сократить восстановление, но оставайтесь в пределах
жёсткого потолка из раздела тюнинга.
