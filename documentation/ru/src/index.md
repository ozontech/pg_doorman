# PgDoorman

Многопоточный пулер соединений для PostgreSQL, написанный на Rust. Drop-in замена для [PgBouncer](https://www.pgbouncer.org/), [Odyssey](https://github.com/yandex/odyssey) и [PgCat](https://github.com/postgresml/pgcat). В production в Ozon уже больше трёх лет под нагрузками Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy) и Node.js.

[Скачать PgDoorman {{VERSION}}](tutorials/installation.md) · [Сравнение](comparison.md) · [Benchmarks](benchmarks.md)

## Что отличает PgDoorman

Три вещи, которых вы не найдёте в PgBouncer и Odyssey.

```admonish success title="Pool Coordinator"
Ограничение числа соединений на уровне базы с приоритетным вытеснением. `max_db_connections` задаёт суммарное число backend-соединений к одной базе; когда лимит исчерпан, idle-соединения вытесняются у пользователей с наибольшим избытком, ранжируя их по p95 времени транзакции — самые медленные пулы отдают соединения первыми. Резервный пул поглощает короткие всплески. Per-user `min_guaranteed_pool_size` защищает критичные нагрузки.

В PgBouncer `max_db_connections` есть, но без вытеснения и без честности распределения. В Odyssey аналога нет.

[Подробнее →](concepts/pool-coordinator.md)
```

```admonish success title="Patroni-assisted Fallback"
Когда PgDoorman работает рядом с PostgreSQL на одной машине и switchover Patroni убивает локальный бэкенд, PgDoorman опрашивает Patroni REST API эндпоинт `/cluster`, выбирает живого члена кластера (предпочтение отдаётся `sync_standby`) и направляет новые соединения туда за 1–2 TCP round trips. Локальный бэкенд остаётся в cooldown; fallback-соединения используют короткий lifetime, чтобы пул вернулся к локальному узлу после восстановления.

Одна строка в `[general]` включает функцию для всех пулов. Никакого внешнего HAProxy, никакого consul-template.

[Подробнее →](tutorials/patroni-assisted-fallback.md)
```

```admonish success title="Graceful Binary Upgrade"
Замените бинарник, не потеряв ни одного клиента. Новый процесс сразу принимает новые соединения, а существующие клиенты завершают свои транзакции на старом. TLS, состояние соединения и cancel keys переносятся корректно.

PgBouncer требует `SO_REUSEPORT` с отдельными процессами (что приводит к разбалансировке пулов). В Odyssey аналогичного механизма нет.

[Подробнее →](tutorials/binary-upgrade.md)
```

## Почему PgDoorman

- **Drop-in замена.** Прозрачно кэширует и переименовывает prepared statements в транзакционном режиме — никаких `DISCARD ALL`, `DEALLOCATE` или хаков в драйверах.
- **Многопоточность.** Один общий пул на все рабочие потоки. PgBouncer однопоточен; запуск нескольких инстансов через `SO_REUSEPORT` приводит к разбалансировке пулов.
- **Подавление thundering herd.** Когда 200 клиентов одновременно борются за 4 idle-соединения, PgDoorman ограничивает число параллельных создаваемых backend-соединений и направляет ожидающих на возвращаемые соединения через direct handoff — большинство получают соединение за микросекунды.
- **Ограниченная хвостовая задержка.** Строгий FIFO через каналы direct-handoff удерживает p99 в пределах 10% от p50 независимо от числа клиентов. Опережающая замена при истечении `server_lifetime` — никаких всплесков при ротации соединений.
- **Обнаружение разорванных бэкендов.** Когда клиент держит открытую транзакцию, а бэкенд умирает (failover, OOM kill), PgDoorman сразу возвращает ошибку. Другие пулеры ждут TCP keepalive и оставляют клиентов висеть на минуты.
- **Сделано для эксплуатации.** Конфиг в YAML или TOML с человекочитаемыми длительностями (`"30s"`, `"5m"`). `pg_doorman generate --host your-db` интроспектирует PostgreSQL и собирает конфиг. `pg_doorman -t` валидирует его перед деплоем. Prometheus-эндпоинт встроен.

## Сравнение

|                                                          |    PgDoorman    | PgBouncer | Odyssey |
| -------------------------------------------------------- | :-------------: | :-------: | :-----: |
| Многопоточность                                          |       Да        |    Нет    |   Да    |
| Prepared statements в транзакционном режиме              |       Да        | Начиная с 1.21 | Начиная с 1.3 |
| Полная поддержка extended query protocol                 |       Да        |    Да     | Частично |
| Pool Coordinator с приоритетным вытеснением              |       Да        |    Нет    |   Нет   |
| Patroni-assisted fallback (встроенный)                   |       Да        |    Нет    |   Нет   |
| Опережающая замена при истечении `server_lifetime`       |       Да        |    Нет    |   Нет   |
| Обнаружение застрявших бэкендов (idle-in-transaction)    |       Да        |    Нет    |   Нет   |
| Graceful binary upgrade                                  |       Да        | Ограниченно |   Нет   |
| Server-side TLS (mTLS, горячая перезагрузка)             |       Да        |    Нет    |   Нет   |
| Auth: passthrough SCRAM (без пароля в открытом виде в конфиге) | Да           |    Нет    |   Да    |
| Auth: JWT                                                |       Да        |    Нет    |   Нет   |
| Auth: PAM / `pg_hba.conf` / auth_query                   |       Да        |    Да     |   Да    |
| Auth: LDAP                                               |       Нет       | Начиная с 1.25 |   Да   |
| Конфиг YAML / TOML                                       |       Да        | Нет (INI) | Нет (свой формат) |
| JSON структурированное логирование                       |       Да        |    Нет    |   Да    |
| Перцентили задержки (p50/90/95/99)                       |       Да        |    Нет    |   Да    |
| Режим проверки конфига (`-t`)                            |       Да        |    Нет    |   Нет   |
| Авто-конфиг из PostgreSQL                                |       Да        |    Нет    |   Нет   |
| Встроенный Prometheus-эндпоинт                           |       Да        | Внешний   |   Да    |

[Полная матрица фич →](comparison.md)

## Бенчмарки

AWS Fargate (16 vCPU), pool size 40, `pgbench` 30 с на тест:

| Сценарий                                | vs PgBouncer | vs Odyssey |
| --------------------------------------- | :----------: | :--------: |
| Extended protocol, 500 клиентов + SSL   |     ×3.5     |    +61%    |
| Prepared statements, 500 клиентов + SSL    |  ×4.0    |    +5%     |
| Simple protocol, 10 000 клиентов        |     ×2.8     |    +20%    |
| Extended + SSL + Reconnect, 500 клиентов |    +96%     |    ~0%     |

[Полные результаты →](benchmarks.md)

## Быстрый старт

Запуск через Docker:

```bash
docker run -p 6432:6432 \
  -v $(pwd)/pg_doorman.yaml:/etc/pg_doorman/pg_doorman.yaml \
  ghcr.io/ozontech/pg_doorman
```

Минимальный конфиг (`pg_doorman.yaml`):

```yaml
general:
  host: "0.0.0.0"
  port: 6432
  admin_username: "admin"
  admin_password: "change_me"

pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "app"
        password: "md5..."   # хэш из pg_shadow / pg_authid
        pool_size: 40
```

`server_username` и `server_password` опущены намеренно — PgDoorman переиспользует MD5-хэш клиента или SCRAM ClientKey для аутентификации в PostgreSQL. Никаких паролей в открытом виде в конфиге.

[Руководство по установке →](tutorials/installation.md) · [Справочник по конфигурации →](reference/general.md)

## Куда дальше

- Впервые с PgDoorman? Начните с [Обзора](tutorials/overview.md), затем [Установка](tutorials/installation.md) и [Базовое использование](tutorials/basic-usage.md).
- Мигрируете с PgBouncer или Odyssey? Прочитайте [Сравнение](comparison.md) и [Аутентификация](authentication/overview.md).
- Используете Patroni? См. [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md) и [`patroni_proxy`](tutorials/patroni-proxy.md).
- Готовитесь к production? Прочитайте [Пул под нагрузкой](tutorials/pool-pressure.md) и [Pool Coordinator](concepts/pool-coordinator.md).
- Эксплуатация PgDoorman? См. [Binary upgrade](tutorials/binary-upgrade.md), [Сигналы](operations/signals.md), [Troubleshooting](tutorials/troubleshooting.md).
