# auth_query

Получайте учётные данные пользователей из самого PostgreSQL вместо того, чтобы перечислять каждого пользователя в конфиге пула. Полезно, когда пользователи заводятся динамически или часто ротируются.

## Два режима

pg_doorman поддерживает два режима; оба настраиваются в одном блоке `auth_query`. Выбор зависит от того, задан ли `server_user`:

- **Режим passthrough** (без `server_user`): каждый аутентифицированный пользователь получает собственный пул бэкенда, аутентифицированный под ним же. Сохраняет идентичность бэкенда на пользователя для `current_user`, row-level security и audit logs.
- **Выделенный режим** (с `server_user`): все динамические пользователи разделяют один пул бэкенда, аутентифицированный как `server_user`. Это размен идентичности бэкенда на более высокое переиспользование пула и меньшее число соединений.

auth_query в стиле PgBouncer — это выделенный режим. Odyssey поддерживает оба. В pg_doorman режим passthrough — по умолчанию.

## Режим passthrough

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    auth_query:
      query: "SELECT passwd FROM pg_shadow WHERE usename = $1"
      user: "postgres"
      password: "md5..."
      database: "postgres"
      cache_ttl: "1h"
      cache_failure_ttl: "30s"
```

Запрос должен возвращать колонку с именем `passwd` или `password`, содержащую хеш MD5 или SCRAM. Дополнительные колонки игнорируются.

`user` и `password` — это учётные данные, под которыми pg_doorman выполняет lookup-запрос. У них должно быть право читать колонку с учётными данными. Либо выдайте доступ к специально созданному представлению (рекомендуется), либо используйте пользователя из группы `pg_read_server_files`.

Когда клиент подключается как `alice`:

1. pg_doorman выполняет запрос с `$1 = 'alice'` и получает её хеш.
2. Кэширует хеш в памяти на `cache_ttl` секунд.
3. Выполняет passthrough-аутентификацию MD5 или SCRAM (смотрите [Passthrough](passthrough.md)).
4. Открывает соединение с бэкендом, аутентифицированное как `alice` с тем же хешем.

## Выделенный режим (dedicated)

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    auth_query:
      query: "SELECT passwd FROM pg_shadow WHERE usename = $1"
      user: "auth_lookup"
      password: "md5..."
      database: "postgres"
      server_user: "app"
      server_password: "md5..."
      pool_size: 40
      min_pool_size: 5
      cache_ttl: "1h"
```

Установка `server_user` переключает режим. Теперь:

1. Клиент аутентифицируется как `alice` против хеша, возвращённого запросом.
2. Пул бэкенда аутентифицирован как `app` (значение `server_user`) и общий для всех динамических пользователей.
3. `current_user` в PostgreSQL всегда будет `app`, независимо от того, какой клиент подключился.

Используйте этот режим, когда у вас много пользователей (тысячи) и пулы бэкенда на каждого исчерпали бы слоты соединений PostgreSQL.

## Рекомендуемая настройка PostgreSQL

Не используйте суперпользователя для lookup. Создайте отдельную функцию с `SECURITY DEFINER`:

```sql
CREATE OR REPLACE FUNCTION pg_doorman_lookup(uname text)
RETURNS TABLE(passwd text)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
  SELECT passwd FROM pg_shadow WHERE usename = uname;
$$;

REVOKE ALL ON FUNCTION pg_doorman_lookup(text) FROM public;
GRANT EXECUTE ON FUNCTION pg_doorman_lookup(text) TO auth_lookup;
```

Затем в конфиге пула:

```yaml
auth_query:
  query: "SELECT passwd FROM pg_doorman_lookup($1)"
  user: "auth_lookup"
  password: "md5..."
```

## Кэширование

| Параметр | По умолчанию | Назначение |
| --- | --- | --- |
| `cache_ttl` | `"1h"` | Сколько кэшируется успешный lookup. |
| `cache_failure_ttl` | `"30s"` | Сколько кэшируется неуспешный lookup. Защищает от усиления brute-force атаки. |
| `min_interval` | `"1s"` | Минимальный интервал между повторными lookup-запросами для одного пользователя. |

Длительности — строки в кавычках: `"1h"`, `"30m"`, `"300s"`. Голое целое число интерпретируется как миллисекунды — `cache_ttl: 3600` будет кэшировать на 3.6 секунды, не на час.

Кэш — пер-пуловый, в памяти, сбрасывается при `RELOAD`. После ротации пароля пользователя сделайте перезапуск или `RELOAD`.

## Observability

`SHOW AUTH_QUERY` показывает статистику по базам:

```
database | cache_entries | cache_hits | cache_misses | cache_refetches | rate_limited | auth_success | auth_failure | executor_queries | executor_errors
```

Метрики Prometheus: `pg_doorman_auth_query_cache`, `pg_doorman_auth_query_auth`, `pg_doorman_auth_query_executor`, `pg_doorman_auth_query_dynamic_pools`. Смотрите [Admin commands](../observability/admin-commands.md).
