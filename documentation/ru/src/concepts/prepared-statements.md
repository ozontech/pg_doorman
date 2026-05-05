# Кеш плана для анонимных prepared statements

PostgreSQL не кеширует план анонимных prepared statements: каждый
`Bind` запускает планировщик с нуля. PgDoorman закрывает этот провал,
прозрачно переписывая каждый анонимный `Parse` в служебное имя
`DOORMAN_<N>` на бекенде. План попадает в named-registry бекенда и
переиспользуется. Переиспользование охватывает `Bind`'ы одного
клиента и `Bind`'ы разных клиентов одного пула.

Подмена прозрачна для драйвера: клиент шлёт и получает пустые имена
точно так же, как при работе с обычным PostgreSQL.

Это уникальная возможность PgDoorman. PgBouncer (1.21+) и Odyssey
поддерживают prepared statements в transaction mode, но только для
**именованных** statement; анонимный `Parse` пробрасывается без
изменений и перепланируется при каждом обращении.

## Базовая модель PostgreSQL

В сообщении `Parse` имя prepared statement может быть пустым или
непустым. Пустое имя означает **анонимный** statement, непустое —
**именованный**:

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

Transaction pooling крутит один backend между десятками клиентов.
Если пулер пробрасывает пустой `Parse` как есть, каждый `Bind`
клиента приходит на backend, который не держит план для этого
запроса. Горячие OLTP-пути платят CPU планировщика на каждом
обращении.

Именованные prepared решают проблему кеширования плана, но
перекладывают учёт на пулер:

- Пулер обязан помнить именованные statement каждого клиента до его
  дисконнекта, даже если pool-level shared cache уже выселил запись.
- На каждом `Bind` пулер обязан проверить, знает ли текущий backend
  это имя, и при необходимости заново сделать `Parse`.
- При дисконнекте клиента пулер обязан отправить `Close` или
  `DEALLOCATE` на правильный backend.
- Драйверы, которые штампуют per-query имена `stmt_<seq>`,
  раздувают per-client cache: сотни записей на клиента, помноженные
  на десятки тысяч клиентов.

Выбор стоит так: отказаться от кеширования плана для анонимных,
либо взять на себя полную стоимость учёта именованных. PgDoorman
выбирает третий путь.

## Что делает PgDoorman

На каждый анонимный `Parse` от клиента PgDoorman:

1. Считает хеш текста запроса плюс OID типов параметров.
2. Ищет хеш в **pool-level** кеше (общий между всеми клиентами
   пула). При miss выделяет новое имя `DOORMAN_<counter>` и
   регистрирует запись `Arc<Parse>`.
3. Записывает в per-client кеш ключ `Anonymous(hash)`, чтобы
   следующий `Bind` нашёл тот же `DOORMAN_<N>`.
4. Отправляет `Parse` на backend с переписанным именем.
5. На соответствующем `Bind` (с пустым именем) переписывает имя
   statement в `DOORMAN_<N>` и проверяет, что текущий backend уже
   держит запись; если нет, отправляет `Parse` повторно.

Клиент никогда не видит `DOORMAN_<N>`. PgDoorman снимает подмену в
ответах и синтезирует `ParseComplete`, когда экономит round-trip на
backend.

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

Второй клиент с тем же запросом в том же пуле получает hit в pool
cache и пропускает `Parse` к backend целиком:

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

  Client-level  AHashMap или LruCache, на клиента.
                Маппит Named(client_name) | Anonymous(hash) → CachedStatement.
                Размер:   client_prepared_statements_cache_size
                          (default 0 = unlimited).

  Server-level  LruCache<String, ()>, на backend-соединение.
                Запоминает, какие DOORMAN_N этот backend уже держит.
                True LRU; при выселении отправляет Close на backend.
```

Текст запроса интернируется через `Arc<str>`: десять клиентов с
одним и тем же анонимным запросом делят одну аллокацию в памяти.

## Когда подмена помогает

- **API-нагрузки с малым набором горячих запросов.** Десяток
  уникальных форм `SELECT` / `INSERT`, разделённых тысячами
  клиентов. Hit rate в pool cache близок к 100 %, планировщик
  работает один раз на backend на запрос, масштабируется линейно с
  параллелизмом.
- **Драйверы, привязанные к анонимным prepared.** `lib/pq`, `libpq`
  `PQexecParams`, JDBC с `serverPreparedStatementType=NONE`. Без
  подмены они каждый раз перепланируют.
- **Смешанные пулы, где named и anonymous соседствуют.** Анонимные
  получают тот же выигрыш от plan cache, что и именованные, без
  раздувания per-client cache.

## Когда подмена не помогает

- **Ad-hoc / OLAP-трафик.** Каждый запрос уникален: pool cache
  вытесняет постоянно, скан O(N) на каждом insert. Отключите через
  `prepared_statements_cache_size = 0`.
- **Скрипты с одним statement.** Паттерн connect → `Parse` →
  1 `Bind` → disconnect не накапливает достаточно hits, чтобы
  окупить учёт. Накладные расходы на `Parse` ~700 нс — небольшие, но
  измеримые.
- **Асинхронные драйверы в pipeline mode.** Каждая сессия получает
  уникальное имя `DOORMAN_async_<N>`, чтобы избежать коллизий
  in-flight. Cross-session переиспользование на server cache не
  работает. Pool-level шеринг текста запроса работает; планировщик
  на backend срабатывает раз в сессию.

Эффективность измеряйте по Prometheus-счётчикам
`pg_doorman_servers_prepared_hits` и
`pg_doorman_servers_prepared_misses`. Устойчивый miss rate выше
30 % означает, что подмена тратит CPU и память, не зарабатывая
переиспользования плана. Либо отключайте, либо увеличивайте
`prepared_statements_cache_size`.

## Как это устроено у других пулеров

| Пулер           | Кеш плана для анонимного Parse                       |
| --------------- | :--------------------------------------------------- |
| **PgDoorman**   | Да: прозрачная подмена на `DOORMAN_<N>`              |
| PgBouncer 1.21+ | Нет: только named, анонимный пробрасывается as-is    |
| Odyssey         | Нет: только named, `pool_reserve_prepared_statement` |
| PgCat           | Нет: только named                                    |

В PgBouncer поддержка prepared statements появилась в 1.21, но
ограничена **именованными**: анонимный `Parse` пробрасывается без
изменений, и каждый `Bind` запускает планировщик. Odyssey'евский
`pool_reserve_prepared_statement` требует именованных statement; на
анонимный трафик он не действует. PgCat ведёт себя так же.

Кеш плана для анонимных prepared — возможность, которую сегодня
предоставляет только PgDoorman.

## Конфигурация

| Параметр                                 | Default | Эффект                                                  |
| ---------------------------------------- | :-----: | ------------------------------------------------------- |
| `prepared_statements_cache_size`         | 8192    | Размер pool-level кеша в записях. 0 отключает подмену.  |
| `client_prepared_statements_cache_size`  | 0       | Размер per-client кеша. 0 = unlimited (LRU выключен).   |

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
  итоговый shared plan может оказаться неудачным для клиента с
  перекошенными данными.

Приложения, которые опираются на PG-семантику "анонимный Parse
стирает предыдущий", должны переключиться на именованные statement
с явным `Close`.

## Observability

Admin-команды:

- `SHOW PREPARED_STATEMENTS` — pool, hash, name, query, `count_used`.
  Топ записей по `count_used` показывает горячие запросы, на которых
  кеш окупается.
- `SHOW POOLS_MEMORY` — `pool_prepared_count`,
  `client_prepared_count`, `pool_prepared_bytes`,
  `client_prepared_bytes`.

Prometheus-метрики (полный список в [Prometheus](../reference/prometheus.md)):

- `pg_doorman_pool_prepared_cache_entries{user, database}`
- `pg_doorman_pool_prepared_cache_bytes`
- `pg_doorman_clients_prepared_cache_entries`
- `pg_doorman_clients_prepared_cache_bytes`
- `pg_doorman_servers_prepared_hits{user, database, backend_pid}`
- `pg_doorman_servers_prepared_misses`
- `pg_doorman_async_clients_count`

## Справочник

- [Режимы пула](pool-modes.md) — transaction mode, где работает
  подмена prepared statements.
- [Общие настройки](../reference/general.md) —
  `prepared_statements_cache_size`,
  `client_prepared_statements_cache_size`.
- [Admin-команды](../observability/admin-commands.md) —
  `SHOW PREPARED_STATEMENTS`, `SHOW POOLS_MEMORY`.
- [Prometheus](../reference/prometheus.md) — полный список метрик.
