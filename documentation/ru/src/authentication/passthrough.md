# Passthrough-аутентификация (по умолчанию)

pg_doorman переиспользует криптографическое доказательство клиента — хеш MD5 или SCRAM `ClientKey` — чтобы аутентифицироваться на PostgreSQL. Открытый пароль никогда не покидает клиента и никогда не хранится в конфигурации пула.

Это рекомендуемая настройка, когда имя пользователя пула совпадает с пользователем PostgreSQL.

## Как это работает

### MD5

Протокол MD5-пароля PostgreSQL хранит на сервере `md5(password + username)`. Клиент хеширует пароль тем же способом и присылает `md5(stored_hash + salt)`. pg_doorman:

1. Получает хешированный ответ клиента.
2. Ищет сохранённый хеш MD5 в своём конфиге (или через `auth_query`).
3. Проверяет, что ответ клиента совпадает.
4. Передаёт **сохранённый хеш** в PostgreSQL как пароль во время аутентификации бэкенда. PostgreSQL принимает его, потому что именно этот хеш и хранится в `pg_authid`.

Поле `password` в конфиге пула содержит сохранённый хеш в формате `md5XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX` (32-символьный MD5 от `password + username` с буквальным префиксом `md5`).

### SCRAM-SHA-256

SCRAM проверяет клиента, не пересылая ничего эквивалентного паролю. pg_doorman:

1. Выполняет SCRAM handshake с клиентом, проверяя `ClientProof`.
2. Извлекает `ClientKey` из успешного обмена.
3. Выполняет SCRAM handshake с PostgreSQL, переиспользуя тот же `ClientKey` для вычисления свежего `ClientProof` под nonce бэкенда.

Поле `password` в конфиге пула содержит SCRAM-верификатор из `pg_authid.rolpassword` в формате `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>`.

pg_doorman не поддерживает SCRAM channel binding (`scram-sha-256-plus`).

## Конфигурация

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "app"
        password: "md5d41d8cd98f00b204e9800998ecf8427e"
        pool_size: 40
```

Обратите внимание, чего здесь **нет**: ни `server_username`, ни `server_password`. pg_doorman распознаёт passthrough-режим по отсутствию этих полей.

Для SCRAM поле password выглядит так:

```yaml
password: "SCRAM-SHA-256$4096:random_salt$stored_key:server_key"
```

## Получение хеша

Подключитесь как суперпользователь к PostgreSQL и прочитайте `pg_shadow` (или `pg_authid`):

```sql
SELECT usename, passwd FROM pg_shadow WHERE usename = 'app';
```

Колонка `passwd` содержит либо хеш MD5 (`md5...`), либо SCRAM-верификатор (`SCRAM-SHA-256$...`) — в зависимости от значения `password_encryption` в момент установки пароля.

Чтобы принудительно сохранить MD5: `SET password_encryption = 'md5'; ALTER ROLE app PASSWORD 'plaintext';`
Чтобы принудительно SCRAM: `SET password_encryption = 'scram-sha-256'; ALTER ROLE app PASSWORD 'plaintext';`

## Когда passthrough-режима недостаточно

Задавайте `server_username` и `server_password` явно, когда:

- Пользователь пула отличается от пользователя бэкенда (переименование).
- Клиент аутентифицируется через [JWT](jwt.md) — у него нет ни хеша MD5, ни ключа SCRAM, чтобы пробросить.
- Клиент аутентифицируется через [Talos](talos.md), и вы хотите фиксированную идентичность бэкенда на роль.
- Вы используете [auth_query](auth-query.md) в выделенном режиме.

```yaml
users:
  - username: "external_app"
    password: "jwt-pkey-fpath:/etc/pg_doorman/jwt.pub"
    server_username: "app"
    server_password: "md5..."
    pool_size: 40
```

## Автоматически сгенерированный конфиг

`pg_doorman generate --host your-pg-host --user your-admin-user` интроспектирует PostgreSQL и собирает конфиг с автоматически подставленными хешами из `pg_shadow`. Используйте это для новых инсталляций, чтобы избежать ошибок копирования.

```bash
pg_doorman generate --host db.example.com --user postgres --output pg_doorman.yaml
```

Подробнее про команду `generate` смотрите в [Basic Usage](../tutorials/basic-usage.md).
