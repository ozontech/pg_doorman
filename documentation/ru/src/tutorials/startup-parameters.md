# Параметры запуска PostgreSQL

Иногда параметры PostgreSQL нужно задавать для каждого серверного
соединения, которое открывает pg_doorman, без правки `postgresql.conf`,
`ALTER ROLE` или `ALTER DATABASE`. Типичные случаи:

- В горячем OLTP-пуле план переключается на generic после решения
  эвристики `plan_cache_mode = auto` и обратно уже не возвращается.
  `ALTER ROLE SET plan_cache_mode = force_custom_plan` затронет любую
  другую нагрузку под этой ролью, а изменить нужно только один пул.
- Приложение не задаёт `statement_timeout` или
  `idle_in_transaction_session_timeout`, а быстро доработать его нельзя.
  Администратору БД нужен сессионный дефолт, который переживает
  клиентский `RESET ALL`.
- Одно приложение должно стабильно показывать конкретный
  `application_name`, независимо от значения, которое передаст драйвер,
  чтобы `pg_stat_activity` и аудит оставались читаемыми.

Для этого в pg_doorman есть `startup_parameters`: карта GUC PostgreSQL,
которую pg_doorman передаёт в каждое новое серверное соединение пула.

## Конфигурация

Каскад состоит из трёх уровней. Более узкий уровень выигрывает по ключу.

```toml
[general.startup_parameters]
statement_timeout = "5s"

[pools.checkout.startup_parameters]
plan_cache_mode = "force_custom_plan"
work_mem        = "64MB"
```

После `SIGHUP` или `RELOAD` через консоль администратора каждый новый
бэкенд пула `checkout` стартует со значениями
`statement_timeout = 5s`, `plan_cache_mode = force_custom_plan` и
`work_mem = 64MB`. В других пулах остаётся только
`statement_timeout = 5s` из `general`; остальные значения берутся из
настроек PostgreSQL по умолчанию. Уже открытые бэкенды не меняются:
новые значения вступают в силу по мере ротации соединений.

В passthrough-режиме `auth_query`, когда `server_user` не задан, запрос
аутентификации может вернуть необязательную колонку `startup_parameters`
типа `text` с JSON-объектом. Значения из этой колонки переопределяют
`general` и настройки пула только для конкретного пользователя.

```sql
SELECT
  rolpassword AS passwd,
  CASE rolname
    WHEN 'vip' THEN '{"work_mem":"256MB"}'::text
    ELSE NULL::text
  END AS startup_parameters
FROM pg_authid
WHERE rolname = $1;
```

Колонка должна возвращаться как `text`. Если SQL отдаёт `json` или
`jsonb`, добавьте явное приведение типа `::text`. pg_doorman читает её
именно как `text` и один раз пишет предупреждение для каждого
пользователя, у которого тип не совпал.

Dedicated-режим `auth_query`, когда `server_user` задан, игнорирует эту
колонку и один раз пишет предупреждение на пару `(пул, пользователь)`.
Один общий бэкенд не может одновременно иметь разные значения для
разных пользователей.

## Что pg_doorman делает со значениями

Слитая карта записывается в `StartupMessage` каждого бэкенда, который
открывает pg_doorman. PostgreSQL запоминает эти значения как сессионные
дефолты (`pg_settings.reset_val` и `pg_settings.source = 'client'`).
Поэтому клиентские `RESET ALL` и `DISCARD ALL` возвращают именно
значение, заданное оператором, а не исходное значение PostgreSQL.

Значение видно со стороны клиента:

```text
checkout=> SHOW plan_cache_mode;
 plan_cache_mode
-------------------
 force_custom_plan

checkout=> SET plan_cache_mode = 'auto'; RESET ALL; SHOW plan_cache_mode;
 plan_cache_mode
-------------------
 force_custom_plan
```

## Валидация

При загрузке конфигурации pg_doorman проверяет:

- Имена ключей должны соответствовать маске GUC PostgreSQL:
  `^[A-Za-z_][A-Za-z0-9_.]*$`. Составные имена вроде
  `auto_explain.log_min_duration` допустимы; произвольная пунктуация
  нет.
- Зарезервированные ключи (`user`, `database`, `replication`, `options`
  и всё, что начинается с `_pq_.`) отклоняются. pg_doorman управляет
  ими сам, либо PostgreSQL обрабатывает их в `StartupMessage` особым
  образом.
- Значения не должны содержать нулевой байт.
- Каждый уровень (`general` или `pool`) должен помещаться в
  операторский бюджет: `MAX_STARTUP_PACKET_LENGTH` (10000 байт) минус
  512 байт, зарезервированных под служебные ключи pg_doorman.

Перед запуском каждого бэкенда pg_doorman заново проверяет уже слитый
каскад против того же лимита. Два уровня, которые помещались по
отдельности, могут вместе выйти за бюджет, особенно когда `auth_query`
добавляет третий слой. В таком случае pg_doorman пропускает все GUC,
заданные оператором, для этого запуска, пишет размеры в лог и открывает
бэкенд с настройками PostgreSQL по умолчанию.

## Что происходит, если PG отвергает параметр

Если PostgreSQL отвергает заданный оператором параметр на старте
бэкенда, pg_doorman возвращает клиенту PG-родной `ErrorResponse` как
есть. Клиент видит тот же sqlstate (`22023`, `42704`, `42501`,
`55P02` или любой другой код из стартового семейства) и то же
сообщение, что увидел бы при прямом подключении к PG.

pg_doorman не пытается переподключиться без этого параметра, не
скрывает ключ и не ведёт per-pool карантин. Следующее подключение
клиента отправит тот же `StartupMessage` и упадёт так же, пока
оператор не исправит конфигурацию.

## Наблюдаемость

Эффективный каскад по каждому пулу виден через административную
SQL-консоль:

```text
admin> SHOW STARTUP_PARAMETERS;
 user  | database | parameter        | value             | source
-------+----------+------------------+-------------------+-----------
 shop  | checkout | plan_cache_mode  | force_custom_plan | pool
 shop  | reports  | statement_timeout| 10s               | general
```

Веб-интерфейс показывает тот же набор в секции «Startup parameters
(operator-injected)» на странице пула.

В Prometheus:

- `pg_doorman_backend_startup_parameter_errors_total{pool, sqlstate}`
  считает каждый запуск бэкенда, отвергнутый PostgreSQL из-за
  параметра, заданного оператором. Имя параметра и пользователя
  пишутся в warn-строке лога; в лейблы они не включены, чтобы
  динамические `auth_query`-пулы не раздували количество серий.

Разумная отправная точка для оповещения: ненулевая скорость роста
`pg_doorman_backend_startup_parameter_errors_total` для одного и того
же пула в течение нескольких минут означает, что каждое подключение
к пулу падает на одном и том же GUC и конфигурацию нужно править.

## Когда это не нужно

- Приложение само задаёт параметр на каждом подключении. Дублирование в
  `startup_parameters` добавляет ещё одну настройку без изменения
  поведения.
- Тюнинг на одну транзакцию (`SET LOCAL`). `startup_parameters` задают
  сессионные дефолты; параметры уровня транзакции должно выставлять
  приложение.
- Значения, которые зависят от текущего запроса. Параметры запуска
  действуют для всех транзакций бэкенда на протяжении его жизни;
  режима «на один statement» нет.

## Справочник

- [Общие настройки](../reference/general.md): `startup_parameters`.
- [Настройки пула](../reference/pool.md):
  `pools.<name>.startup_parameters`.
- [auth_query](../authentication/auth-query.md): passthrough- и
  dedicated-режимы, чтение колонки `startup_parameters`.
- [Команды администратора](../observability/admin-commands.md):
  `SHOW STARTUP_PARAMETERS`.
- [Метрики Prometheus](../reference/prometheus.md): полный список.
