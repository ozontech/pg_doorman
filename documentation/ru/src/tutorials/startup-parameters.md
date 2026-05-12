# Параметры запуска PostgreSQL

PgDoorman может задавать параметры PostgreSQL при открытии серверного
соединения, не меняя `postgresql.conf`, `ALTER ROLE` или
`ALTER DATABASE`. Это полезно, например, в таких случаях:

- В горячем OLTP-пуле план переключается на generic после решения
  эвристики `plan_cache_mode = auto` и обратно уже не возвращается.
  `ALTER ROLE SET plan_cache_mode = force_custom_plan` затронет любую
  другую нагрузку под этой ролью, а изменить нужно только один пул.
- Приложение не задаёт `statement_timeout` или
  `idle_in_transaction_session_timeout`, а быстро доработать его нельзя.
  Администратору БД нужно сессионное значение по умолчанию, которое
  сохранится после клиентского `RESET ALL`.
- Одно приложение должно стабильно показывать конкретный
  `application_name`, независимо от значения, которое передаст драйвер,
  чтобы `pg_stat_activity` и аудит оставались читаемыми.

Для этого используется `startup_parameters`: набор GUC PostgreSQL,
который pg_doorman добавляет в `StartupMessage` новых серверных
соединений пула.

## Конфигурация

Значения применяются в три слоя. Более узкий слой переопределяет ключ
из предыдущего.

```toml
[general.startup_parameters]
statement_timeout = "5s"

[pools.checkout.startup_parameters]
plan_cache_mode = "force_custom_plan"
work_mem        = "64MB"
```

После `SIGHUP` или `RELOAD` через консоль администратора каждое новое
серверное соединение пула `checkout` открывается со значениями
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
именно как `text` и пишет предупреждение для полученной строки, если
тип не совпал.

Dedicated-режим `auth_query`, когда `server_user` задан, игнорирует эту
колонку и один раз пишет предупреждение на пару `(пул, пользователь)`.
В этом режиме один серверный пул обслуживает разных пользователей,
поэтому per-user значения применить нельзя.

## Что pg_doorman делает со значениями

pg_doorman добавляет итоговый набор параметров в `StartupMessage`
каждого нового бэкенда. PostgreSQL сохраняет эти значения как
сессионные значения по умолчанию (`pg_settings.reset_val` и
`pg_settings.source = 'client'`). Поэтому клиентские `RESET ALL` и
`DISCARD ALL` возвращают операторские значения, а не исходные
значения PostgreSQL.

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
- Каждый уровень (`general` или `pool`) должен помещаться в лимит для
  операторских параметров: `MAX_STARTUP_PACKET_LENGTH` (10000 байт)
  минус 512 байт, зарезервированных под служебные ключи pg_doorman.

Перед запуском каждого бэкенда pg_doorman заново проверяет объединённый
набор параметров по тому же лимиту. Два уровня, которые помещались по
отдельности, могут вместе выйти за лимит, особенно когда `auth_query`
добавляет третий слой. Если лимит превышает только слой `auth_query`,
pg_doorman отбрасывает этот слой и сохраняет baseline из `general` и
пула. Если не помещается сам baseline или полный startup-пакет,
pg_doorman пропускает все операторские параметры для этого
запуска и пишет размеры в лог.

## Что происходит, если PG отвергает параметр

Если PostgreSQL отвергает заданный оператором параметр при запуске
бэкенда, pg_doorman возвращает клиенту `ErrorResponse` PostgreSQL без
изменений. Клиент видит тот же sqlstate (`22023`, `42704`, `42501`,
`55P02` или любой другой код из стартового семейства) и то же сообщение,
что увидел бы при прямом подключении к PostgreSQL.

pg_doorman не пробует повторить подключение без отклонённого параметра
и не отключает этот ключ автоматически для пула. Следующее подключение
клиента отправит тот же `StartupMessage` и получит ту же ошибку, пока
оператор не исправит конфигурацию.

## Наблюдаемость

Итоговые параметры по каждому пулу видны через административную
SQL-консоль:

```text
admin> SHOW STARTUP_PARAMETERS;
 user | database | parameter         | value             | source  | state
------+----------+-------------------+-------------------+---------+--------
 shop | checkout | plan_cache_mode   | force_custom_plan | pool    | applied
 shop | reports  | statement_timeout | 10s               | general | applied
```

Веб-интерфейс показывает эти же строки на странице пула в секции
«Startup parameters (operator-injected)».

В Prometheus:

- `pg_doorman_backend_startup_parameter_errors_total{pool, sqlstate}`
  считает попытки запуска бэкенда, которые PostgreSQL отклонил из-за
  параметра, заданного оператором. Имя параметра и пользователя
  пишутся в строку лога уровня `warn`; в лейблы они не включены, чтобы
  динамические `auth_query`-пулы не раздували количество серий.

Разумная отправная точка для алерта: если
`pg_doorman_backend_startup_parameter_errors_total` растёт по одному и
тому же пулу несколько минут подряд, новые подключения к этому пулу
падают на одном и том же GUC. Конфигурацию нужно исправить до возврата
трафика.

## Когда это не нужно

- Приложение само задаёт параметр на каждом подключении. Дублирование в
  `startup_parameters` добавляет ещё одну настройку без изменения
  поведения.
- Тюнинг на одну транзакцию (`SET LOCAL`). `startup_parameters` задают
  сессионные значения по умолчанию; параметры уровня транзакции должно
  выставлять приложение.
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
