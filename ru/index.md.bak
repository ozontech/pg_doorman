# PgDoorman

Многопоточный пулер соединений для PostgreSQL, написанный на Rust. Drop-in замена для [PgBouncer](https://www.pgbouncer.org/) и [Odyssey](https://github.com/yandex/odyssey), альтернатива [PgCat](https://github.com/postgresml/pgcat). Три года в production у Ozon под нагрузками Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy) и Node.js.

[Скачать PgDoorman {{VERSION}}](tutorials/installation.md) · [Сравнение](comparison.md) · [Benchmarks](benchmarks.md)

## Ключевые возможности

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

По `SIGUSR2` старый процесс передаёт TCP-сокет каждого idle-клиента новому через `SCM_RIGHTS` — тот же сокет, без переподключения — вместе с cancel keys и кешем prepared statements. Клиенты внутри транзакции дорабатывают её на старом процессе и мигрируют, как только становятся idle. Со сборкой `tls-migration` (Linux, opt-in) переезжает и cipher state OpenSSL — TLS-сессии переживают upgrade без re-handshake.

Online restart в PgBouncer (`-R`, deprecated с 1.20; либо rolling restart через `so_reuseport`) и в Odyssey (`SIGUSR2` + `bindwith_reuseport`) устроены одинаково: новый процесс принимает новые соединения, старый дорабатывает до тех пор, пока его клиенты сами не отключатся. Сессии, prepared statements и TLS-состояние между процессами не переезжают.

[Подробнее →](tutorials/binary-upgrade.md)
```

## Почему PgDoorman

- **Prepared statements в transaction mode.** PgDoorman переименовывает клиентские statement names в `DOORMAN_N` и ведёт кеш на трёх уровнях — pool, client, backend. Драйверы видят свои имена, backend'ы — переименованные. Никаких прикладных `DEALLOCATE`, никаких `DISCARD ALL`.
- **Многопоточность с одним общим пулом.** Все рабочие потоки делят один пул. PgBouncer однопоточен; рекомендованный способ масштабирования — несколько инстансов за `so_reuseport` — даёт каждому инстансу свой отдельный пул, и idle-счётчики для одной и той же базы могут расходиться между процессами.
- **Подавление thundering herd.** Когда 200 клиентов борются за 4 idle-соединения, PgDoorman ограничивает число параллельно создаваемых backend-соединений (`scaling_max_parallel_creates`) и направляет возвращающиеся серверы напрямую самому давно ждущему клиенту через in-process oneshot-канал — без перекладывания через idle-очередь.
- **Ограниченная хвостовая задержка.** Очередь ожидающих обслуживается строго FIFO — никто не обгоняет того, кто пришёл раньше. Опережающая замена истекающих backend-соединений (на 95% от `server_lifetime`, до 3 параллельных) удерживает пул прогретым: при ротации поколения соединений нет всплеска checkout-латентности.
- **Обнаружение мёртвого backend внутри транзакции.** Если backend умирает посреди транзакции (failover, OOM, network partition), PgDoorman сразу возвращает SQLSTATE `08006`: чтение клиента состязается с readability backend через 100-мс тик. Без этой проверки клиент завис бы до срабатывания TCP keepalive — на дефолтных настройках Linux это около двух часов плюс 9×75 с probe.
- **Сделано для эксплуатации.** Конфиг в YAML или TOML с человекочитаемыми длительностями (`30s`, `5m`). `pg_doorman generate --host …` интроспектирует существующий PostgreSQL и собирает starter-конфиг. `pg_doorman -t` валидирует конфиг без запуска сервера. Prometheus-эндпоинт `/metrics` встроен.

## Сравнение

| Возможность                                              |       PgDoorman       |              PgBouncer              |          Odyssey           |
| -------------------------------------------------------- | :-------------------: | :---------------------------------: | :------------------------: |
| Многопоточность с общим пулом                            |          Да           |        Нет (однопоточный)           |  Workers, отдельные пулы   |
| Prepared statements в transaction mode                   |          Да           |          Да (с 1.21)                | Да (`pool_reserve_prepared_statement`) |
| Pool Coordinator (per-database cap, priority eviction)   |          Да           |                Нет                  |            Нет             |
| Patroni-assisted fallback (встроенный)                   |          Да           |                Нет                  |            Нет             |
| Опережающая замена при истечении `server_lifetime`       |          Да           |                Нет                  |            Нет             |
| Обнаружение мёртвого backend внутри транзакции           | Да (мгновенный `08006`) |   Нет (ждёт TCP keepalive)         | Нет (ждёт TCP keepalive)   |
| Binary upgrade с миграцией сессий                        | Да (`SCM_RIGHTS`, TLS state opt-in) | Нет (сессии остаются на старом процессе) | Нет (сессии остаются на старом процессе) |
| Backend TLS к PostgreSQL                                 |   Да (5 режимов, hot reload по `SIGHUP`) | Да (`server_tls_*`, hot reload по `RELOAD`) |            Нет             |
| Auth: SCRAM passthrough (без plaintext-пароля в конфиге) | Да (`ClientKey` извлекается из proof) | Да (encrypted SCRAM secret через `auth_query`/`userlist.txt`, с 1.14) |            Да              |
| Auth: JWT (RSA-SHA256)                                   |          Да           |                Нет                  |            Нет             |
| Auth: PAM / `pg_hba.conf` / `auth_query`                 |          Да           |                Да                   |            Да              |
| Auth: LDAP                                               |          Нет          |          Да (с 1.25)                |            Да              |
| Формат конфига                                           |     YAML / TOML       |                INI                  |       Свой формат          |
| JSON structured logging                                  |          Да           |                Нет                  |   Да (`log_format "json"`) |
| Latency percentiles (p50/p90/p95/p99)                    | Да (встроенный `/metrics`) |     Нет (только средние)        |  Да (через отдельный Go-exporter) |
| Режим проверки конфига (`-t`)                            |          Да           |                Нет                  |            Нет             |
| Авто-конфиг из PostgreSQL (`generate --host`)            |          Да           |                Нет                  |            Нет             |
| Prometheus-эндпоинт                                      | Встроенный `/metrics` |       Внешний exporter              |  Внешний exporter (Go sidecar) |

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
