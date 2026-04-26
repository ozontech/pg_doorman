# Admin-команды

pg_doorman предоставляет admin-базу, совместимую с протоколом Postgres. Подключайтесь к тому же порту, что и обычные клиенты, но с `dbname=pgdoorman` и admin-учёткой из конфига:

```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman
```

Или через connection string `psql`:

```bash
psql "host=127.0.0.1 port=6432 user=admin dbname=pgdoorman"
```

Admin-команды читаются через `SHOW <subcommand>` или выполняются голыми глаголами (`PAUSE`, `RESUME`, `RECONNECT`, `RELOAD`, `SHUTDOWN`, `SET <param> = <value>`).

## Команды SHOW

| Команда | Назначение |
| --- | --- |
| `SHOW HELP` | Список доступных команд. |
| `SHOW CONFIG` | Текущая активная конфигурация. Только для чтения. |
| `SHOW DATABASES` | По одной строке на пул: host, port, database, размер пула, режим. |
| `SHOW POOLS` | Снимок утилизации пула на пару user×database: idle/active/waiting клиенты, idle/active серверы. |
| `SHOW POOLS_EXTENDED` | `SHOW POOLS` плюс полученные/отправленные байты и среднее время ожидания. |
| `SHOW POOLS_MEMORY` | Учёт памяти на пул для кэша prepared statements (клиентский и серверный). |
| `SHOW POOL_COORDINATOR` | Состояние Pool Coordinator на базу: текущие соединения, использование резерва, число вытеснений. См. [Pool Coordinator](../concepts/pool-coordinator.md). |
| `SHOW POOL_SCALING` | Метрики anticipation/burst: in-flight create-операции, ожидания на воротах, anticipation notifies/timeouts. |
| `SHOW PREPARED_STATEMENTS` | Закэшированные prepared statements на пул: hash, имя, текст запроса, число попаданий. |
| `SHOW CLIENTS` | Активные клиенты: ID, database, user, имя приложения, адрес, состояние TLS, счётчики transaction/query/error, возраст. |
| `SHOW SERVERS` | Активные соединения с бэкендом: ID сервера, PID бэкенда, database, user, TLS, состояние, счётчики transaction/query, попадания/промахи кэша prepare, байты. |
| `SHOW CONNECTIONS` | Число соединений по типу: total, errors, TLS, plain, cancel. |
| `SHOW STATS` | Агрегированная статистика на пару user×database: всего транзакций, запросов, времени, байт, средние. |
| `SHOW LISTS` | Счётчики по категориям (databases, users, pools, clients, servers). |
| `SHOW USERS` | Список пользователей и их режимы пула. |
| `SHOW AUTH_QUERY` | Кэш `auth_query`: попадания/промахи/перезапросы, успехи/отказы аутентификации, ошибки исполнителя, счётчики динамических пулов. |
| `SHOW SOCKETS` | Счётчики TCP- и Unix-сокетов по состоянию (только Linux — читает `/proc/net/`). |
| `SHOW LOG_LEVEL` | Текущий уровень логирования. |
| `SHOW VERSION` | Версия pg_doorman. |

`SHOW POOL_COORDINATOR` и `SHOW POOL_SCALING` не имеют аналогов в PgBouncer или Odyssey — они показывают внутренние механизмы pg_doorman.

## Управляющие команды

| Команда | Эффект |
| --- | --- |
| `PAUSE` | Прекратить принимать новые клиентские запросы. Существующие клиенты завершают свои транзакции. |
| `PAUSE <database>` | Поставить на паузу один пул. |
| `RESUME` / `RESUME <database>` | Возобновить после `PAUSE`. |
| `RECONNECT` / `RECONNECT <database>` | Принудительно пересоздать соединения с бэкендом (закрыть простаивающие, дренировать активные). Новые соединения берутся из PostgreSQL. |
| `RELOAD` | То же, что и `SIGHUP` — перезагрузить конфиг с диска. |
| `SHUTDOWN` | То же, что и `SIGTERM` — плавное завершение работы. |
| `KILL <database>` | Сбросить всех клиентов, подключённых к конкретному пулу. |
| `SET log_level = '<level>'` | Изменить уровень логирования в рантайме (`error`, `warn`, `info`, `debug`, `trace`). |

`PAUSE`/`RESUME` полезны при failover или окнах обслуживания. `RECONNECT` после ротации учётных данных в `pg_authid` гарантирует, что бэкенды используют новый пароль.

## Чтение типового вывода

### `SHOW POOLS`

```
database | user | cl_idle | cl_active | cl_waiting | sv_active | sv_idle | sv_used | maxwait
mydb     | app  | 12      | 4         | 0          | 4         | 36      | 0       | 0.0
```

- `cl_waiting > 0` означает, что клиенты застряли в ожидании бэкенда. Либо поднимите `pool_size`, либо проверьте медленные запросы.
- `sv_idle` соответствует свободным бэкендам; `sv_active` — занятым; `sv_used` — зарезервированным координатором (см. ниже).
- `maxwait` — самое долгое текущее ожидание в секундах. Если оно вырастает за `query_wait_timeout`, клиенты получают ошибки.

### `SHOW POOL_COORDINATOR`

```
database | max_db_conn | current | reserve_size | reserve_used | evictions | reserve_acq | exhaustions
mydb     | 80          | 78      | 16           | 2            | 142       | 18          | 0
```

- `evictions` быстро растут: какой-то пользователь голодает раз за разом. Задайте или поднимите `min_guaranteed_pool_size` для этого пользователя.
- `reserve_acq` высокий: всплески — норма, но возможно вы недооценили размер. Подумайте о повышении `max_db_connections`, а не о ставке на резерв.
- `exhaustions` ненулевые: даже резерв был полным. Клиенты упёрлись в `query_wait_timeout`. Поднимите потолок.

Тонкости настройки см. в [Pool Coordinator](../concepts/pool-coordinator.md).

### `SHOW POOL_SCALING`

```
user | database | inflight | creates | gate_waits | burst_gate_budget_ex | antic_notify | antic_timeout | create_fallback | replenish_def
app  | mydb     | 1        | 12345   | 87         | 3                    | 142          | 8             | 22              | 0
```

- `inflight` — текущие создания соединений с бэкендом в процессе.
- `gate_waits` растут: `scaling_max_parallel_creates` придушивает вас. Допустимо, если PostgreSQL под нагрузкой; поднимите, если PG может обработать больше параллельных вызовов `connect()`.
- Соотношение `antic_notify` и `antic_timeout`: высокий счётчик timeout означает, что упреждающее ожидание не успевает поймать возвращающееся соединение. Поднимите `scaling_warm_pool_ratio`, чтобы пул рос с опережением спроса.
- `create_fallback` растёт — срабатывает предзамена: соединения истекают раньше, чем естественным образом возвращаются.

См. [Пул под нагрузкой → Параметры тюнинга](../tutorials/pool-pressure.md#Параметры-тюнинга).

## Аутентификация

Admin-база использует учётку из `general.admin_username` и `general.admin_password`:

```yaml
general:
  admin_username: "admin"
  admin_password: "change_me"
```

Admin-соединения не проходят через правила `pg_hba.conf` — они идут напрямую в admin-обработчик. Ограничивайте admin-доступ на сетевом уровне (`listen_addresses`, фаервол) или используйте Unix-сокеты.

## Куда дальше

- [Prometheus reference](../reference/prometheus.md) — те же данные в машинно-читаемом виде.
- [Pool Coordinator](../concepts/pool-coordinator.md) — что говорит вам `SHOW POOL_COORDINATOR`.
- [Pool Pressure](../tutorials/pool-pressure.md) — что говорит вам `SHOW POOL_SCALING`.
- [Troubleshooting](../tutorials/troubleshooting.md) — типичные сбои и их вывод в `SHOW`.
