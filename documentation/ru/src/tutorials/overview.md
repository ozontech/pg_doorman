# Обзор

## Что делает PgDoorman

PgDoorman сидит между приложениями и PostgreSQL. Для приложения он выглядит как сервер PostgreSQL (тот же wire-протокол, та же строка подключения для `psql`); внутри он мультиплексирует много клиентских сессий на гораздо меньший набор реальных backend-соединений.

```mermaid
graph LR
    App1[Приложение A] --> Pooler(PgDoorman)
    App2[Приложение B] --> Pooler
    App3[Приложение C] --> Pooler
    Pooler --> DB[(PostgreSQL)]
```

Изначально PgDoorman был форкнут из [PgCat](https://github.com/postgresml/pgcat), но с тех пор переписан вокруг других целей: prepared statements в transaction mode, многопоточные общие пулы, интеграция с Patroni и binary upgrade с миграцией живых сессий. Сейчас это самостоятельный кодовый код.

## Зачем вообще пулер

Каждое соединение к PostgreSQL стоит серверу около 10 МБ RAM, отдельный процесс и время на каждый handshake (auth, SCRAM, разрешение `search_path`). Без пулера приложение, открывающее N короткоживущих соединений в секунду, платит N×время-handshake. Пулер позволяет тем же N клиентам переиспользовать небольшой набор долгоживущих backend-соединений, и стоимость handshake оплачивается один раз на backend, а не один раз на клиента.

Конкретные эффекты:

- `pool_size` равный 40 обычно обслуживает несколько тысяч клиентских сессий для коротких OLTP-транзакций.
- PostgreSQL не платит per-process memory overhead за соединения, которые иначе пришлось бы держать открытыми.
- Failover, restart или rolling deploy не выливаются в thundering herd свежих handshake-ов.

## Pool modes

```admonish success title="Transaction (рекомендуется)"
Backend-соединение удерживается на время одной транзакции и возвращается в пул при `COMMIT` или `ROLLBACK`. Именно в этом режиме пулинг реально окупается.
```

```admonish info title="Session"
Backend-соединение удерживается на всё время клиентской сессии и возвращается только при отключении клиента. Используйте для клиентов, зависящих от состояния уровня сессии (`SET TIME ZONE` вне транзакции, advisory-блокировки между транзакциями, `WITH HOLD` курсоры).
```

PgDoorman не реализует statement mode. См. [Pool Modes](../concepts/pool-modes.md) — точный контракт каждого режима и что работает в transaction mode у нас, чего нет у других пулеров.

## Что есть для эксплуатации

- **Admin-консоль** — PostgreSQL-совместимый эндпоинт для `SHOW POOLS`, `SHOW CLIENTS`, `RELOAD`, `PAUSE`, `UPGRADE` и т.д.
- **Prometheus `/metrics`** — встроенный HTTP-эндпоинт с per-pool latency-перцентилями, счётчиками prepared statements, состоянием fallback и метриками TLS.
- **`pg_doorman -t`** — валидация конфига без запуска сервера.
- **`pg_doorman generate --host …`** — собрать starter-конфиг через интроспекцию существующего PostgreSQL.

См. [Admin-команды](../observability/admin-commands.md) и [Справочник Prometheus](../reference/prometheus.md).

## Куда дальше

- [Установка](installation.md) — установить pg_doorman из пакетов, исходников или Docker.
- [Базовое использование](basic-usage.md) — минимальный конфиг, первое подключение, типичные грабли.
- [Pool Coordinator](../concepts/pool-coordinator.md) — когда одна база делится между несколькими user-пулами.
- [Binary upgrade](binary-upgrade.md) — заменить бинарник в production без потери живых сессий.
