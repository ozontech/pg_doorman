# Кеш плана для анонимных prepared statements

PostgreSQL не кеширует план анонимных prepared statements: каждый
`Bind` запускает планировщик с нуля. PgDoorman кеширует план сам,
прозрачно переписывая каждый анонимный `Parse` в служебное имя
`DOORMAN_<N>` на бекенде. План попадает в named-registry бекенда и
переиспользуется между `Bind`'ами одного клиента и между разными
клиентами одного пула.

Подмена прозрачна для драйвера: клиент шлёт и получает пустые имена
точно так же, как при работе с обычным PostgreSQL.

Это уникальная возможность PgDoorman. PgBouncer (1.21+) и Odyssey
поддерживают prepared statements в transaction mode, но только для
**именованных** statement; анонимный `Parse` пробрасывается без
изменений и перепланируется при каждом обращении.

## Базовая модель PostgreSQL

В сообщении `Parse` имя prepared statement задаёт его тип: пустое
имя соответствует **анонимному** statement, любое непустое —
**именованному**:

```text
                          Время жизни в PG        Кеширование плана
  ─────────────────────   ─────────────────       ──────────────────
  Анонимный (name="")     До следующего           Нет: планировщик
                          анонимного Parse        запускается на
                          или конца сессии        каждый Bind
  Именованный             До Close /              Generic вначале,
   (name="stmt_42")       DEALLOCATE /            переключается в
                          конца сессии            custom после 5
                                                  наблюдений
```

Большинство современных драйверов по умолчанию используют
**анонимные** prepared для разовых параметризованных запросов:
`lib/pq` (Go), `libpq` `PQexecParams` (C), часть режимов в pgjdbc и
psycopg. Прикладной код выглядит как обычный параметризованный
запрос, но в wire protocol уходит пустое имя.

## Почему это проблема в transaction-mode

В transaction pooling один backend по очереди обслуживает разных
клиентов. Если пулер пробрасывает пустой `Parse` как есть, каждый
`Bind` клиента приходит на backend, у которого плана для этого
запроса нет. Горячие OLTP-пути платят CPU планировщика на каждом
обращении.

Именованные prepared решают проблему кеширования плана, но
перекладывают учёт на пулер:

- Пулер обязан помнить именованные statement каждого клиента до его
  дисконнекта, даже если pool-level shared cache уже выселил запись.
- На каждом `Bind` пулер обязан проверить, знает ли текущий backend
  это имя, и при необходимости заново сделать `Parse`.
- При дисконнекте клиента пулер обязан отправить `Close` или
  `DEALLOCATE` на правильный backend.
- Драйверы, которые генерируют отдельное имя `stmt_<seq>` под каждый
  уникальный запрос, раздувают per-client cache: сотни записей на
  клиента, при тысячах подключений превращающиеся в миллионы записей
  в памяти.

Остаются два варианта: отказаться от кеширования плана для анонимных
или взять на себя полную стоимость учёта именованных. PgDoorman
выбирает третий путь.

## Что делает PgDoorman

На каждый анонимный `Parse` от клиента PgDoorman:

1. Считает хеш по тексту запроса и OID типов параметров.
2. Ищет хеш в **pool-level** кеше (общий между всеми клиентами
   пула). При miss выделяет новое имя `DOORMAN_<counter>` и
   регистрирует запись `Arc<Parse>`.
3. Записывает в per-client кеш ключ `Anonymous(hash)`, чтобы
   следующий `Bind` нашёл тот же `DOORMAN_<N>`.
4. Отправляет `Parse` на backend с переписанным именем.
5. На соответствующем `Bind` (с пустым именем) переписывает имя
   statement в `DOORMAN_<N>` и проверяет, что текущий backend уже
   держит запись; если нет, отправляет `Parse` повторно.

Клиент никогда не видит `DOORMAN_<N>`: имя живёт только на участке
между PgDoorman и backend. Когда нужный backend уже держит запись,
PgDoorman синтезирует `ParseComplete` сам и не делает round-trip.

### Пример wire-protocol

Go-приложение, выполняющее

```go
db.Query("SELECT * FROM t WHERE name = $1", "vasya")
```

через `lib/pq`, отправляет такой обмен:

```text
  Клиент                  PgDoorman                  Backend
  ──────                  ─────────                  ───────
  Parse("", q)        ───►│ hash, miss → DOORMAN_42
                           │ pool_cache[hash] = Arc<Parse>
                           │ client_cache[Anon(hash)] = ...
                           │            Parse("DOORMAN_42") ─────►
                           │                   ◄── ParseComplete
                      ◄────│ ParseComplete
  Bind("", "vasya")   ───►│ rewrite "" → "DOORMAN_42"
                           │            Bind("DOORMAN_42") ──────►
                           │            Execute, Sync ───────────►
                           │               ◄── BindComplete, ...
                           │               ◄── ReadyForQuery
                      ◄────│ BindComplete, ...
```

Второй клиент с тем же запросом в том же пуле попадает в pool cache
и не отправляет `Parse` на backend:

```text
  Клиент B           PgDoorman                       Backend (тот же)
  ────────           ─────────                       ────────────────
  Parse("", q)  ───►│ hash hit → DOORMAN_42
                     │ server_cache содержит "DOORMAN_42"
                ◄────│ синтетический ParseComplete   (на backend ничего)
  Bind("", v)   ───►│ rewrite "" → "DOORMAN_42"
                     │            Bind("DOORMAN_42") ────►
                     │            ...
```

## Слои кеша

PgDoorman держит состояние prepared statements на трёх уровнях:

```text
  Pool-level    DashMap<hash, CacheEntry>
                Один на пул. Хранит Arc<Parse> с именем DOORMAN_N.
                Размер:    prepared_statements_cache_size (default 8192).
                Выселение: approximate LRU.

  Client-level  Named:     AHashMap<String, CachedStatement>, без лимита.
                Anonymous: LruCache<u64, CachedStatement> ограничен
                           client_anonymous_prepared_cache_size (default 256),
                           или AHashMap при размере 0.
                Выселение Anonymous локальное: Arc<Parse> дропается,
                DOORMAN_<N> на бекенде остаётся.

  Server-level  LruCache<String, ()>, на backend-соединение.
                Запоминает, какие DOORMAN_N этот backend уже держит.
                True LRU; при выселении отправляет Close на backend.
```

При выселении записи из Anonymous LRU PgDoorman дропает локальную
ссылку и не отправляет `Close` на бекенд. Соответствующий
`DOORMAN_<N>` будет переиспользован server-level LRU или закроется по
`server_lifetime` (default 20 минут) — что наступит раньше.

Текст запроса интернируется через `Arc<str>`: десять клиентов с
одним и тем же анонимным запросом делят одну аллокацию в памяти.

## Когда подмена помогает

- **API-нагрузки с малым набором горячих запросов.** Десяток
  уникальных форм `SELECT` / `INSERT` на тысячи клиентов. Hit rate
  в pool cache близок к 100 %, планировщик работает один раз на
  backend на каждый запрос, нагрузка линейно масштабируется по
  параллелизму.
- **Драйверы, привязанные к анонимным prepared.** `lib/pq`, `libpq`
  `PQexecParams`, JDBC с `serverPreparedStatementType=NONE`. Без
  подмены они каждый раз перепланируют.
- **Смешанные пулы, где named и anonymous соседствуют.** Анонимные
  получают тот же выигрыш от plan cache, что и именованные, без
  раздувания per-client cache.

## Когда подмена не помогает

- **Ad-hoc / OLAP-трафик.** Каждый запрос уникален: pool cache
  постоянно вытесняет записи, на каждом insert идёт O(N) скан.
  Отключите через `prepared_statements_cache_size = 0`.
- **Скрипты с одним statement.** Паттерн connect → `Parse` →
  1 `Bind` → disconnect не накапливает достаточно hits, чтобы
  окупить учёт. Накладные расходы на `Parse` ~700 нс — небольшие, но
  измеримые.
- **Асинхронные драйверы в pipeline mode.** Каждая сессия получает
  уникальное имя `DOORMAN_async_<N>`, чтобы избежать коллизий между
  одновременными in-flight операциями. Server cache между сессиями
  не переиспользуется. Pool-level кеш по-прежнему делит текст
  запроса между сессиями; планировщик на backend срабатывает раз
  в сессию.

Эффективность измеряйте по Prometheus-счётчикам
`pg_doorman_servers_prepared_hits` и
`pg_doorman_servers_prepared_misses`. Устойчивый miss rate выше
30 % означает, что подмена расходует CPU и память, а
переиспользования плана не происходит. Тогда либо отключайте
подмену, либо увеличивайте `prepared_statements_cache_size`.

## Как это устроено у других пулеров

| Пулер           | Кеш плана для анонимного Parse                       |
| --------------- | :--------------------------------------------------- |
| **PgDoorman**   | Да: прозрачная подмена на `DOORMAN_<N>`              |
| PgBouncer 1.21+ | Нет: только named, анонимный пробрасывается as-is    |
| Odyssey         | Нет: только named, `pool_reserve_prepared_statement` |
| PgCat           | Нет: только named                                    |

В PgBouncer поддержка prepared statements появилась в 1.21, но
ограничена **именованными**: анонимный `Parse` пробрасывается без
изменений, и каждый `Bind` запускает планировщик. Флаг
`pool_reserve_prepared_statement` в Odyssey требует именованных
statement; на анонимный трафик он не влияет. PgCat ведёт себя
так же.

Кешировать план анонимных prepared сегодня умеет только PgDoorman.

## Конфигурация

| Параметр                                 | Default | Эффект                                                                  |
| ---------------------------------------- | :-----: | ----------------------------------------------------------------------- |
| `prepared_statements_cache_size`         | 8192    | Размер pool-level кеша в записях. 0 отключает подмену.                  |
| `client_anonymous_prepared_cache_size`   | 256     | Размер per-client Anonymous LRU. 0 = unlimited. Named всегда без лимита.|

Named-часть per-client кеша всегда без лимита и не зависит от
`client_anonymous_prepared_cache_size`.

Полностью отключить подмену анонимных (редко, для OLAP-only):

```yaml
general:
  prepared_statements_cache_size: 0
```

## Отличия от семантики PostgreSQL

Подмена меняет несколько протокольных деталей, на которые могут
полагаться строгие приложения:

- Один и тот же анонимный `Parse`, отправленный дважды, **не**
  стирает предыдущий. Каждая пара `(query, param_types)` живёт
  независимо в pool cache под своим `DOORMAN_<N>`.
- `Close` с пустым именем — no-op для кешей PgDoorman.
  Соответствующий `DOORMAN_<N>` живёт до выселения pool-level LRU
  или до закрытия пула.
- План остаётся generic дольше. В чистом PostgreSQL именованный
  statement переключается с generic на custom после пяти
  наблюдений; если разные клиенты делят один `DOORMAN_<N>` и каждый
  даёт по одному-двум `Bind`'ам, порог достигается быстрее. Но
  получившийся общий план может оказаться неудачным для клиента
  с перекошенными данными.

Приложения, которые опираются на PG-семантику "анонимный Parse
стирает предыдущий", должны переключиться на именованные statement
с явным `Close`.

## Тюнинг

### Размер кеша

Кеш prepared statements в PgDoorman состоит из трёх слоёв, и
управляют ими два связанных параметра:

- `prepared_statements_cache_size` (по умолчанию `8192`) задаёт
  размер общего pool-level кеша — одна карта на пул, ключом служит
  хеш запроса. Это верхняя граница на число различных query shape,
  которые пул помнит сразу для всех клиентов. Приближённый LRU:
  выселение проходит за O(N) по всей карте и не отправляет `Close`
  на бекенд (другие клиенты могут ещё держать `Arc`).
- `server_prepared_statements_cache_size` (по умолчанию наследует
  `prepared_statements_cache_size`) задаёт размер per-backend
  кеша — отдельный LRU на каждое серверное соединение, ключом
  служит имя `DOORMAN_<N>`. Это верхняя граница на число prepared
  statements, которое PgDoorman позволит держать одному бекенду
  PostgreSQL. Точный LRU за O(1); при выселении в очередь бекенда
  кладётся `Close`, который отправляется ближайшим Sync или Flush —
  представление `pg_prepared_statements` может временно показывать
  больше строк, чем потолок, пока не придёт следующий Sync.

Оба параметра принимают per-pool override:

```yaml
general:
  prepared_statements_cache_size: 8192
  server_prepared_statements_cache_size: 1024  # потолок per-backend жёстче

pools:
  oltp:
    # наследует оба значения из general
    pool_mode: "transaction"
  reporting:
    # у этого пула шире разнообразие запросов; пусть per-backend кеш
    # вмещает больше
    server_prepared_statements_cache_size: 4096
    pool_mode: "transaction"
```

`prepared_statements_cache_size: 0` отключает подмену целиком и
заодно обнуляет server-level LRU. Указать
`server_prepared_statements_cache_size: 0` при положительном
pool-size допустимо, но смысла мало: per-backend кеш превратится
в pass-through, и каждое попадание на чужой бекенд приведёт к
повторному `Parse`.

Когда уменьшать `server_prepared_statements_cache_size` ниже размера
pool-level кеша:

- На бекендах копится слишком много строк `DOORMAN_<N>`
  (`pg_prepared_statements` упирается в потолок, plan memory
  растёт).
- Хочется ускорить вытеснение через `Close`, не урезая попадания
  в pool-level кеш.

Когда оставить значения равными (поведение по умолчанию):

- Нет измеренной проблемы с памятью на бекендах. Достаточно
  наследования.

### Дефолт `client_anonymous_prepared_cache_size = 256`

Лимит в 256 записей на клиента подобран под типичную OLTP-нагрузку:
небольшой набор горячих анонимных запросов делится между тысячами
клиентов. Каждая запись хранит лёгкую структуру
`(hash, async_name?, Arc<Parse>)` — сам `Arc<Parse>` шарится с
pool-level кешем, поэтому накладные расходы per-client ≈ 80 байт
на запись. На 10 000 подключённых клиентах × 256 записей × ~80 байт
получаем около 200 МБ предсказуемого потолка на пулере.

Поднимайте лимит, когда:

- ORM или генератор SQL выдаёт `stmt_<seq>` под каждый запрос и
  Anonymous LRU постоянно вытесняет записи (видно по устойчиво
  ненулевой скорости `pg_doorman_clients_prepared_anonymous_evictions_total`).
- Приложение заведомо имеет широкое рабочее множество в одной
  сессии (больше 256 разных анонимных запросов), и скорость
  выселений соответствует этой нагрузке.

Уменьшайте лимит или поднимайте `max_memory_usage` при очень больших
числах подключений (50 000+ клиентов): на таком масштабе даже
256 × clients × 80 байт пересекает 1 ГБ учётной памяти на пулере, и
урезание лимита уполовинивает её.

### Named всегда без лимита

Named-часть per-client кеша не ограничена. PgDoorman держит
`Arc<Parse>` для каждого именованного statement, который создал
клиент, до его дисконнекта или явного `DEALLOCATE` /
`DEALLOCATE ALL`. Это согласуется с собственным контрактом
PostgreSQL — именованные prepared живут до конца сессии — и
исключает сценарий, при котором выселение Named-записи под нагрузкой
ломает следующий `Bind` ошибкой `prepared statement does not exist`.

Обратная сторона: драйверы, генерирующие отдельное имя под каждый
запрос (часть режимов pgjdbc и Hibernate, отдельные конфигурации
.NET Npgsql), могут раздуть Named-часть без потолка. PgDoorman не
может ограничить её безопасно; ответственность за переиспользование
имён или явный `DEALLOCATE` лежит на приложении.

Сигнал давления есть только для Anonymous LRU — счётчик выселений
`pg_doorman_clients_prepared_anonymous_evictions_total`. Для Named
такого сигнала нет: следите за колонкой `client_named_count` в
`SHOW POOLS_MEMORY` и метрикой `pg_doorman_clients_prepared_named_entries`
на предмет неожиданного роста.

### Окно утечки памяти на бекенде

При выселении записи из Anonymous LRU на стороне клиента PgDoorman
дропает только локальный `Arc<Parse>`. Соответствующий
`DOORMAN_<N>` остаётся живым на каждом backend, который когда-либо
его обслуживал. Очищают его две силы:

- **Server-level LRU.** На каждом backend ведётся свой
  `LruCache<String, ()>` имён `DOORMAN_<N>`, ограниченный
  `prepared_statements_cache_size` (default 8192). При достижении
  лимита backend отправляет `Close` на наименее давно использованное
  имя и освобождает план.
- **Ротация backend.** Backend достигает `server_lifetime` (default
  20 мин), и pg_doorman закрывает его; новый backend стартует с
  пустым plan cache.

Худший случай по памяти на одном backend — это
`prepared_statements_cache_size × ~100 КБ` плана ≈ 800 МБ на стороне
PostgreSQL. Чтобы сжать окно:

- Снизьте `prepared_statements_cache_size`, чтобы server-level LRU
  быстрее выселяла планы.
- Снизьте `server_lifetime`, чтобы backend ротировались чаще.

Системное представление `pg_prepared_statements` в PostgreSQL
показывает имена, которые держит текущий backend. Подсчёт строк там
показывает, насколько близко backend подошёл к лимиту.

## Observability

Admin-команды:

- `SHOW PREPARED_STATEMENTS` — pool, hash, name, query, `count_used`,
  `kind`. Топ записей по `count_used` показывает горячие запросы,
  на которых кеш окупается. Колонка `kind` — последняя в наборе и
  принимает значения `named`, `anonymous` или `mixed` в зависимости
  от того, как клиенты использовали запись за её жизнь.

  Пример:

  ```text
   pool         | hash               | name        | query             | count_used | kind
  --------------+--------------------+-------------+-------------------+------------+-----------
   sharded.user | 1234567890123456   | DOORMAN_1   | SELECT * FROM t1  |     150234 | anonymous
   sharded.user | 2345678901234567   | DOORMAN_2   | INSERT INTO t2 .. |      87654 | named
   sharded.user | 3456789012345678   | DOORMAN_3   | SELECT * FROM t3  |      45678 | mixed
  ```

- `SHOW POOLS_MEMORY` — `pool_prepared_count`,
  `client_prepared_count`, `pool_prepared_bytes`,
  `client_prepared_bytes` плюс разбивка по kind:
  `client_named_count`, `client_anonymous_count`,
  `client_anonymous_evictions_total`. Суффикс `_total` отмечает
  последнюю колонку как счётчик (нарастающий с момента старта пула)
  в отличие от gauge-колонок слева.

Prometheus-метрики (полный список в [Prometheus](../reference/prometheus.md)):

- `pg_doorman_pool_prepared_cache_entries{user, database}`
- `pg_doorman_pool_prepared_cache_bytes`
- `pg_doorman_clients_prepared_cache_entries`
- `pg_doorman_clients_prepared_cache_bytes`
- `pg_doorman_clients_prepared_named_entries{user, database}`
- `pg_doorman_clients_prepared_anonymous_entries{user, database}`
- `pg_doorman_clients_prepared_anonymous_evictions_total{user, database}`
- `pg_doorman_servers_prepared_hits{user, database, backend_pid}`
- `pg_doorman_servers_prepared_misses`
- `pg_doorman_async_clients_count`

## Алертинг

### Скорость выселений в Anonymous LRU

Устойчиво ненулевая скорость на счётчике выселений означает, что
LRU вытесняет записи быстрее, чем приложение переиспользует их.
Шаблон алерта:

```text
rate(pg_doorman_clients_prepared_anonymous_evictions_total[5m]) > 10
  for 10m
```

Порог в 10 выселений/с на пул — отправная точка, реальное значение
зависит от формы трафика и числа подключений. Срабатывание алерта
читайте как "лимит слишком мал или рабочее множество приложения
шире, чем ожидалось"; решение — либо поднять
`client_anonymous_prepared_cache_size`, либо разобраться, не
генерирует ли приложение уникальные запросы на горячем пути.

### Интерпретация `kind = mixed`

Каждая запись pool-level кеша помнит, использовали ли её клиенты под
именованным statement, под анонимным, или и так и так. `kind = mixed`
означает, что одна и та же пара `(query, param_types)` была
обработана хотя бы одним клиентом как named и хотя бы одним другим
как anonymous за её текущую жизнь. Большинство нагрузок не видят
строк `mixed`; пул, в котором их большинство, говорит о
гетерогенной клиентской базе (разные драйверы или конфигурации
драйверов против одной БД), и эту разнородность стоит проверить —
иногда она задумана, иногда сигналит, что один из клиентов настроен
неверно.

### Число prepared statements на бекенде

PostgreSQL отдаёт `pg_prepared_statements` для текущего backend.
Если память пулера в норме, но RSS PostgreSQL backend растёт,
посчитайте строки на каждом backend:

```sql
SELECT count(*) FROM pg_prepared_statements;
```

Цифры около `prepared_statements_cache_size` (default 8192)
означают, что server-level LRU работает на потолке и единственный
способ освободить память планов — ротация. Если `server_lifetime`
велик, планы копятся долго. Снижение любого из этих параметров
ослабляет давление на память планов ценой более частых перепарсингов
на backend.

## Справочник

- [Режимы пула](../concepts/pool-modes.md) — transaction mode, где
  работает подмена prepared statements.
- [Общие настройки](../reference/general.md) —
  `prepared_statements_cache_size`,
  `client_anonymous_prepared_cache_size`.
- [Admin-команды](../observability/admin-commands.md) —
  `SHOW PREPARED_STATEMENTS`, `SHOW POOLS_MEMORY`.
- [Prometheus](../reference/prometheus.md) — полный список метрик.
