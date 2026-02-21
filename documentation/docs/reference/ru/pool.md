---
title: Настройки пула
---

## Настройки пула

Каждая запись в пуле — это имя виртуальной базы данных, к которой клиент pg-doorman может подключиться.

```toml
[pools.exampledb] # Declaring the 'exampledb' database
```

### server_host

Директория с unix-сокетами или IPv4-адрес сервера PostgreSQL для данного пула.

Пример: `"/var/run/postgresql"` или `"127.0.0.1"`.

Default: `"127.0.0.1"`.

### server_port

Порт, через который сервер PostgreSQL принимает входящие подключения.

Default: `5432`.

### server_database

Необязательный параметр, определяющий, к какой базе данных подключаться на сервере PostgreSQL.

### application_name

Параметр application_name, отправляемый серверу при открытии соединения с PostgreSQL. Полезен при настройке sync_server_parameters = false.

### connect_timeout

Максимальное время на установку нового серверного соединения для этого пула, в миллисекундах. Если не указано, используется глобальная настройка connect_timeout.

Default: `None (uses global setting)`.

### idle_timeout

Закрывать простаивающие соединения в этом пуле, открытые дольше указанного значения, в миллисекундах. Если не указано, используется глобальная настройка idle_timeout.

Default: `None (uses global setting)`.

### server_lifetime

Закрывать серверные соединения в этом пуле, открытые дольше указанного значения, в миллисекундах. Применяется только к простаивающим соединениям. Если не указано, используется глобальная настройка server_lifetime.

Default: `None (uses global setting)`.

### pool_mode

* `session` — Сервер возвращается в пул после отключения клиента.
* `transaction` — Сервер возвращается в пул после завершения транзакции.

Default: `"transaction"`.

### log_client_parameter_status_changes

Логировать информацию о любых SET-командах.

Default: `false`.

### cleanup_server_connections

При включении пул автоматически очищает серверные соединения, которые больше не нужны. Это помогает эффективно управлять ресурсами, закрывая простаивающие соединения.

Default: `true`.

## Настройки пользователей пула

```toml
[pools.exampledb.users.0]
username = "exampledb-user-0" # A virtual user who can connect to this virtual database.
```
### username

Виртуальное имя пользователя, которое может подключаться к данной виртуальной базе данных (пулу).

### password

Пароль виртуального пользователя пула.
Пароль может быть указан в формате `MD5`, `SCRAM-SHA-256` или `JWT`.
Также можно создать зеркальный список пользователей из секретов PostgreSQL: `select usename, passwd from pg_shadow`.

### auth_pam_service

PAM-сервис, отвечающий за авторизацию клиента. В этом случае pg_doorman будет игнорировать значение `password`.

### server_username

Реальное имя пользователя PostgreSQL для подключения к серверу базы данных.

По умолчанию PgDoorman использует одно и то же `username` как для аутентификации клиента, так и для подключения к серверу. Однако если `password` клиента — хеш MD5 или SCRAM (что является типичной настройкой), PostgreSQL **отклонит подключение**, поскольку ожидает пароль в открытом виде, а не хеш.

Для исправления укажите `server_username` и `server_password` с реальными учётными данными PostgreSQL. Оба поля должны быть указаны вместе.

### server_password

Пароль в открытом виде для пользователя сервера PostgreSQL, указанного в `server_username`.

Это необходимо, потому что PgDoorman хранит пароли клиентов как хеши MD5/SCRAM для аутентификации клиента, но PostgreSQL требует пароль в открытом виде при серверной аутентификации.

### pool_size

Максимальное количество одновременных соединений с сервером PostgreSQL для данного пула и пользователя.

Default: `40`.

### min_pool_size

Минимальное количество соединений для поддержания в пуле для данного пользователя. Помогает с производительностью, поддерживая готовые соединения. Если указано, должно быть меньше или равно pool_size.

Default: `None`.

### server_lifetime

Закрывать серверные соединения для этого пользователя, открытые дольше указанного значения, в миллисекундах. Применяется только к простаивающим соединениям. Если не указано, используется настройка server_lifetime пула.

Default: `None (uses pool setting)`.

!!! warning "Типичная проблема настройки"
    Если вы видите ошибки аутентификации при подключении PgDoorman к PostgreSQL, наиболее вероятная причина — `server_username` и `server_password` не установлены. Без них PgDoorman пытается аутентифицироваться в PostgreSQL, используя хеш MD5/SCRAM из поля `password`, который PostgreSQL отклоняет.

    **Решение:** Установите оба поля `server_username` и `server_password` с реальными учётными данными PostgreSQL:

    ```yaml
    users:
      - username: "app_user"
        password: "md5..."                # hash for client authentication
        server_username: "app_user"       # real PostgreSQL username
        server_password: "plaintext_pwd"  # real PostgreSQL password
    ```

