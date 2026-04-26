# Аутентификация Talos

Talos — схема аутентификации на основе JWT, разработанная в Ozon. Токен несёт в claim `resource_access` назначение роли на каждую базу, а pg_doorman извлекает наивысшую роль, чтобы выбрать идентичность бэкенда. Несколько ключей подписи поддерживаются через заголовок `kid`.

Если вы работаете внутри стека идентификации Ozon Talos — это нужная вам интеграция. Снаружи предпочитайте обычный [JWT](jwt.md).

## Как это работает

1. Клиент подключается с именем пользователя `talos` и JWT в качестве пароля.
2. pg_doorman читает поле `kid` из заголовка JWT и ищет соответствующий публичный ключ в `general.talos.keys`.
3. Токен проверяется (RS256, `exp`, `nbf`).
4. pg_doorman обходит ключи `resource_access`, разбивает каждый по `:` и сверяет часть **после двоеточия** с `general.talos.databases`. То есть ключ вида `"postgres.stg:billing"` совпадает с базой `billing`. Роли из всех совпавших записей собираются вместе; побеждает наивысшая (`owner` > `read_write` > `read_only`).
5. Соединение аутентифицируется против пользователя пула, имя которого совпадает с ролью: `owner`, `read_write` или `read_only`. Этот пользователь должен существовать в пуле с заданными `server_username` и `server_password`.

Идентичность клиента (`clientId` из токена) сохраняется в `application_name` и audit logs.

## Конфигурация

```yaml
general:
  host: "0.0.0.0"
  port: 6432
  talos:
    keys:
      - "/etc/pg_doorman/talos/keys/abc123.pem"
      - "/etc/pg_doorman/talos/keys/def456.pem"
    databases:
      - "billing"
      - "inventory"

pools:
  billing:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "owner"
        server_username: "billing_owner"
        server_password: "md5..."
        pool_size: 20
      - username: "read_write"
        server_username: "billing_app"
        server_password: "md5..."
        pool_size: 40
      - username: "read_only"
        server_username: "billing_ro"
        server_password: "md5..."
        pool_size: 60
```

Имя файла каждого ключа без расширения (`abc123`, `def456`) — это `kid`, который сверяется с заголовком JWT.

`databases` — фильтр: только перечисленные базы допускаются для Talos. Токен без записи для запрошенной базы будет отвергнут.

## Структура токена

```json
{
  "kid": "abc123",
  "alg": "RS256"
}
.
{
  "exp": 1714500000,
  "nbf": 1714400000,
  "clientId": "billing-service",
  "resource_access": {
    "postgres.stg:billing": { "roles": ["read_write"] },
    "postgres.stg:inventory": { "roles": ["read_only", "read_write"] }
  }
}
```

Ключи `resource_access` обязаны содержать двоеточие. pg_doorman игнорирует всё до него и сверяет суффикс с `general.talos.databases`. Токен, собранный без префикса с двоеточием, не даст ни одной роли, и аутентификация провалится с сообщением «Token may not contain valid roles for the requested databases».

Клиент, подключающийся к `inventory` с этим токеном, попадает в пользователя `read_write` (максимум из двух перечисленных ролей).

## Порядок выбора метода

У Talos наивысший приоритет. Если клиент подключается с именем пользователя `talos` и `general.talos.keys` непуст, никакой другой метод аутентификации не пробуется.

Смотрите [Обзор](overview.md#порядок-выбора-метода).

## Оговорки

- Talos требует специального имени пользователя `talos`. Не-Talos клиенты используют другие методы аутентификации в обычном порядке.
- Сопоставление роли с пользователем фиксированное: `owner`, `read_write`, `read_only`. Кастомные имена ролей требуют изменений в коде.
- Несколько ролей в одной записи `resource_access` свёртываются в максимум. Семантики «deny» нет.
- Публичные ключи загружаются один раз при старте и перечитываются по `SIGHUP`.
