# PgDoorman

Многопоточный пулер соединений для PostgreSQL, написанный на Rust. Drop-in замена для [PgBouncer](https://www.pgbouncer.org/) и [Odyssey](https://github.com/yandex/odyssey), альтернатива [PgCat](https://github.com/postgresml/pgcat). Три года в production у Ozon под нагрузками Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy) и Node.js.

[Скачать PgDoorman {{VERSION}}](tutorials/installation.md) · [Сравнение](comparison.md) · [Benchmarks](benchmarks.md)

## Ключевые возможности

```admonish success title="Встроенный диагностический дашборд"
Диагностическая консоль, встроенная в бинарь pg_doorman и обслуживаемая тем же портом, что и `/metrics`. Что показывает: тайлы насыщения пулов, sparkline p95/p99 латентности по пулам, ошибки в разбивке по SQLSTATE на каждый пул, top-N застрявших запросов, разбор памяти jemalloc по категориям (live allocations / фрагментация / внутренние кеши / code+libs / стеки / swap), значения из `/proc/self/status` с пояснениями рядом с цифрами, per-thread CPU tokio-worker'ов, атрибуцию prepared cache, содержимое query interner, живой хвост лога. Сортируемые и фильтруемые таблицы Pools / Clients / Apps / Caches; live qps и tx-per-second на каждое приложение и каждого клиента.

PgBouncer, PgCat, Odyssey, PgPool-II, RDS Proxy и Cloud SQL Auth Proxy отдают `/metrics` и admin-консоль через psql. Тот стек, который пришлось бы собирать поверх — Prometheus + Grafana + memory exporter + кастомный набор панелей, — у pg_doorman уже встроен.

Pause / Resume / Reconnect / Reload запускаются с той же страницы, per-pool или глобально. В остальном read-only. Консоль включается только при `[web].ui = true` и `admin_password`, отличном от пустой строки и от значения по умолчанию `admin`; с незаданным паролем pg_doorman остаётся в режиме «только `/metrics`» и пишет `WARN` в лог.

[Подробнее →](guides/web-ui.md)
```

```admonish success title="Pool Coordinator"
PgDoorman ограничивает суммарное число backend-соединений к одной базе. При достижении `max_db_connections` координатор вытесняет idle-соединение у пользователя с наибольшим запасом, ранжируя кандидатов по p95 времени транзакции — медленные пулы уступают первыми. Reserve pool поглощает короткие всплески; per-user `min_guaranteed_pool_size` исключает критичные нагрузки из списка вытеснения.

В PgBouncer `max_db_connections` есть, но без вытеснения и без честности распределения — при достижении лимита клиенты ждут, пока существующие соединения сами не закроются по idle timeout. В Odyssey аналога нет.

[Подробнее →](concepts/pool-coordinator.md)
```

```admonish success title="Patroni-assisted Fallback"
Когда PgDoorman работает рядом с PostgreSQL на одной машине и switchover Patroni убивает локальный backend, PgDoorman опрашивает Patroni REST API (`GET /cluster`), выбирает живого члена кластера (приоритет `sync_standby` → `replica`) и направляет новые соединения туда. Локальный backend уходит в cooldown; fallback-соединения наследуют короткий lifetime — пул возвращается на локальный узел сразу, как только тот восстанавливается.

Задайте `patroni_api_urls` и `fallback_cooldown` в `[general]`, и это применится ко всем пулам. Без HAProxy и `consul-template` перед пулером.

[Подробнее →](tutorials/patroni-assisted-fallback.md)
```

```admonish success title="Graceful Binary Upgrade"
Обновляйте PgDoorman в рабочее время, без maintenance window. Приложения не получают ошибок переподключения, PostgreSQL не накрывает лавиной `auth`/SCRAM handshake-ов от одновременных reconnect-ов, идущие транзакции не падают.

По `SIGUSR2` старый процесс передаёт TCP-сокет каждого idle-клиента новому через `SCM_RIGHTS` — тот же сокет, без переподключения — вместе с cancel keys и кешем prepared statements. Клиенты внутри транзакции дорабатывают её на старом процессе и мигрируют, как только становятся idle. Со сборкой `tls-migration` (Linux, отключено по умолчанию) переезжает и cipher state OpenSSL — TLS-сессии переживают upgrade без re-handshake.

Online restart в PgBouncer (`-R`, deprecated с 1.20; либо rolling restart через `so_reuseport`) и в Odyssey (`SIGUSR2` + `bindwith_reuseport`) устроены одинаково: новый процесс принимает новые соединения, старый дорабатывает до тех пор, пока его клиенты сами не отключатся. Сессии, prepared statements и TLS-состояние между процессами не переезжают.

[Подробнее →](tutorials/binary-upgrade.md)
```

```admonish success title="Кеш плана для анонимных prepared statements"
PostgreSQL не кеширует план анонимных prepared statements (`Parse` с пустым именем — типичная форма для разовых параметризованных запросов в большинстве драйверов): каждый `Bind` заново запускает планировщик. PgDoorman прозрачно переписывает пустое имя в служебное `DOORMAN_<N>` на бэкенде, и план попадает в реестр именованных statement бэкенда — переиспользуется между `Bind`'ами одного клиента и между клиентами одного пула.

PgBouncer (1.21+) и Odyssey поддерживают prepared statements в transaction mode, но только для **именованных** statement; анонимный `Parse` пробрасывается без изменений и каждый раз перепланируется. PgDoorman переписывает анонимный `Parse` сам.

Кеш ограничен и наблюдаем. Анонимные записи истекают по бездействию, именованные освобождаются, как только на них никто не ссылается, а `SHOW INTERNER` и метрики Prometheus показывают объём в реальном времени — поток сгенерированного SQL больше не удерживает память пулера до перезапуска.

[Подробнее →](tutorials/prepared-statements.md)
```

## Почему PgDoorman

- **Prepared statements без ручных имен.** PgDoorman может переиспользовать подготовленное состояние в пределах пула, включая безымянный `Parse`, который многие драйверы отправляют для коротких параметризованных запросов. Размер кеша, записи и вытеснения видны через `SHOW INTERNER` и метрики.
- **Один пул для всех рабочих потоков.** Рабочие потоки используют общий набор backend-соединений. При масштабировании PgBouncer несколькими процессами за `so_reuseport` каждый процесс держит свой пул, поэтому свободные соединения могут распределяться неравномерно.
- **Контроль всплесков при создании соединений.** Если много клиентов одновременно ждут несколько свободных backend-соединений, PgDoorman ограничивает параллельное создание новых соединений (`scaling_max_parallel_creates`) и передаёт вернувшийся backend самому старому ожидающему клиенту.
- **Предсказуемая задержка выдачи соединения.** Ожидающие клиенты обслуживаются по FIFO. PgDoorman заранее заменяет backend-соединения перед истечением `server_lifetime`, чтобы ротация поколения не превращалась во всплеск задержки при выдаче соединения.
- **Быстрое обнаружение обрыва backend.** Если backend пропадает во время транзакции, PgDoorman отслеживает это параллельно чтению клиента и возвращает SQLSTATE `08006`, не дожидаясь системного TCP keepalive.
- **Операционные инструменты в бинаре.** Конфиг пишется в YAML или TOML, длительности задаются как `30s` или `5m`, `pg_doorman generate --host ...` собирает стартовый конфиг из существующего PostgreSQL, `pg_doorman -t` проверяет конфиг без запуска сервера, а `/metrics` доступен без отдельного exporter.

## Сравнение

| Функция                                                  |       PgDoorman       |              PgBouncer              |          Odyssey           |
| -------------------------------------------------------- | :-------------------: | :---------------------------------: | :------------------------: |
| Общий пул соединений для всех рабочих потоков            |          Да           |        Нет, один рабочий поток       |  Рабочие процессы с отдельными пулами |
| Prepared statements в transaction pooling                |          Да           |          Да, с версии 1.21           | Да, через `pool_reserve_prepared_statement` |
| Общий лимит backend-соединений к базе                    | Да, с вытеснением idle-соединений |                Нет                  |            Нет             |
| Переключение на резервный узел через Patroni             |     Да, встроено       |                Нет                  |            Нет             |
| Опережающая замена стареющих backend-соединений          |          Да           |                Нет                  |            Нет             |
| Обрыв backend во время транзакции                        | Да, возвращает `08006` без ожидания TCP keepalive | Нет, ждёт TCP keepalive | Нет, ждёт TCP keepalive |
| Обновление бинаря с переносом idle-сессий                | Да, через `SCM_RIGHTS`; TLS-состояние при сборке `tls-migration` | Нет, старые сессии остаются в старом процессе | Нет, старые сессии остаются в старом процессе |
| TLS-соединение от пулера к PostgreSQL                    | Да, 5 режимов и reload по `SIGHUP` | Да, `server_tls_*` и reload по `RELOAD` |            Нет             |
| SCRAM passthrough без открытого пароля в конфиге         | Да, извлекает `ClientKey` из proof | Да, зашифрованный SCRAM secret через `auth_query`/`userlist.txt`, с 1.14 |            Да              |
| JWT-аутентификация (RSA-SHA256)                          |          Да           |                Нет                  |            Нет             |
| PAM, `pg_hba.conf` и `auth_query`                        |          Да           |                Да                   |            Да              |
| LDAP-аутентификация                                      |          Нет          |          Да, с версии 1.25           |            Да              |
| Формат конфигурации                                      |     YAML или TOML      |                INI                  |       Собственный формат    |
| Структурированные JSON-логи                              |          Да           |                Нет                  |   Да, `log_format "json"`  |
| Перцентили задержек p50/p90/p95/p99                      | Да, через встроенный `/metrics` |     Нет, только средние значения |  Да, через отдельный Go exporter |
| Диагностическая web-консоль                              |     Да, встроенная     | Нет, только admin console через psql | Нет, только admin console через psql |
| Проверка конфига без запуска сервера                     |      Да, `-t`          |                Нет                  |            Нет             |
| Генерация начального конфига из PostgreSQL               | Да, `generate --host`  |                Нет                  |            Нет             |
| Эндпоинт Prometheus                                      | Встроенный `/metrics` |       Отдельный exporter            |  Отдельный Go exporter     |

[Полная матрица фич →](comparison.md)

## Бенчмарки

AWS Fargate (16 vCPU), pool size 40, `pgbench` 30 с на тест:

| Сценарий                                | vs PgBouncer | vs Odyssey |
| --------------------------------------- | :----------: | :--------: |
| Extended protocol, 500 клиентов + SSL    |     ×3.5     |    +61%    |
| Prepared statements, 500 клиентов + SSL  |     ×4.0     |    +5%     |
| Simple protocol, 10 000 клиентов         |     ×2.8     |    +20%    |
| Extended + SSL + reconnect, 500 клиентов |     +96%     |    ~0%     |

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
        password: "md5..."   # хеш из pg_shadow / pg_authid
        pool_size: 40
```

`server_username` и `server_password` намеренно не указаны: PgDoorman использует MD5-хеш клиента или SCRAM `ClientKey` для аутентификации к PostgreSQL. В конфиге нет паролей в открытом виде.

[Руководство по установке →](tutorials/installation.md) · [Справочник по конфигурации →](reference/general.md)

## Куда дальше

- Впервые видите PgDoorman? Начните с [Обзора](tutorials/overview.md), затем [Установка](tutorials/installation.md) и [Базовое использование](tutorials/basic-usage.md).
- Мигрируете с PgBouncer или Odyssey? Прочитайте [Сравнение](comparison.md) и [Аутентификацию](authentication/overview.md).
- Используете Patroni? См. [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md) и [`patroni_proxy`](tutorials/patroni-proxy.md).
- Готовитесь к production? Прочитайте [Пул под нагрузкой](tutorials/pool-pressure.md) и [Pool Coordinator](concepts/pool-coordinator.md).
- Эксплуатируете PgDoorman? См. [Binary upgrade](tutorials/binary-upgrade.md), [Сигналы](operations/signals.md), [Troubleshooting](tutorials/troubleshooting.md).
