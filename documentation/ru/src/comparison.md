# PgDoorman vs PgBouncer vs Odyssey

Сравнительная матрица фич для выбора пулера соединений PostgreSQL. Каждое утверждение про PgBouncer привязано к [config reference](https://www.pgbouncer.org/config.html) и [changelog](https://www.pgbouncer.org/changelog.html); каждое утверждение про Odyssey — к [docs](https://github.com/yandex/odyssey/tree/master/docs) проекта.

PgCat намеренно опущен: у него центр тяжести — шардинг и балансировка, а не drop-in замена PgBouncer, поэтому построчное сравнение вводит в заблуждение. Если нужен горизонтальный шардинг, см. [репозиторий PgCat](https://github.com/postgresml/pgcat).

Цифры из бенчмарков — [Бенчмарки](benchmarks.md).

## Аутентификация

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| MD5 password | Да | Да | Да |
| SCRAM-SHA-256 (клиент → пулер) | Да | Да | Да |
| Сквозной SCRAM-SHA-256 (без открытого пароля в конфиге) | Да (`ClientKey` извлекается из proof клиента) | Да (с 1.14, encrypted SCRAM secret в `auth_query` / `userlist.txt`) | Да |
| Сквозной MD5 | Да | Да | Да |
| `auth_query` (динамические пользователи) | Да | Да | Да |
| Сквозной режим `auth_query` (своя идентичность PostgreSQL для каждого пользователя) | Да | Нет (один `auth_user` на все lookup-запросы) | Да |
| Файл в формате `pg_hba.conf` | Да (файл или inline) | Да (`auth_hba_file`) | Да (с 1.4) |
| PAM | Да (Linux) | Да (`auth_type=pam` или через HBA) | Да |
| JWT (RSA-SHA256) | Да | Нет | Нет |
| Talos (custom JWT с извлечением роли) | Да (специфика Ozon) | Нет | Нет |
| LDAP | Нет | Да (с 1.25) | Да |
| SCRAM channel binding (`scram-sha-256-plus`) | Нет | Да | Да |
| User-name maps (cert/peer → DB user) | Нет | Да (с 1.23) | Да |
| Тонкая настройка `scram_iterations` | Нет | Да (с 1.25) | Нет |

См. [Аутентификация](authentication/overview.md).

## TLS

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Client-side TLS (режимы: `disable`, `allow`, `require`, `verify-full`) | Да | Да (`disable`, `allow`, `prefer`, `require`, `verify-ca`, `verify-full`) | Да |
| Server-side TLS к PostgreSQL (`disable`, `allow`, `require`, `verify-ca`, `verify-full`) | Да (5 режимов) | Да (`server_tls_*`, 6 режимов вкл. `prefer`) | Нет |
| mTLS к PostgreSQL (отправка клиентского сертификата на backend) | Да (`server_tls_certificate` + `server_tls_private_key`) | Да (`server_tls_key_file` + `server_tls_cert_file`) | Нет |
| Hot reload server-side TLS-сертификатов | Да (`SIGHUP`) | Да (через `RELOAD` / `SIGHUP`, "new file contents will be used for new connections") | Нет |
| Hot reload client-facing TLS-сертификатов | Нет (требуется restart или binary upgrade) | Да (через `RELOAD` / `SIGHUP`) | Нет |
| Минимальная версия TLS настраивается | Да (по умолчанию TLS 1.2) | Да (`tls_protocols`, default `tlsv1.2,tlsv1.3`) | Настраивается, дефолты другие |
| Direct TLS handshake (PostgreSQL 17, без `SSLRequest`) | Нет | Да (с 1.25) | Нет |
| Контроль TLS 1.3 cipher suites | Нет | Да (с 1.25, `client_tls13_ciphers`/`server_tls13_ciphers`) | Нет |
| Миграция TLS-сессии при binary upgrade | Да (сборка `tls-migration`, Linux, по запросу) | Нет (TLS-соединения отбрасываются при online restart) | Нет |

См. [TLS](guides/tls.md).

## Маршрутизация и высокая доступность

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Fallback через Patroni (встроенный lookup `/cluster`) | Да | Нет | Нет |
| Bundled TCP-прокси с маршрутизацией по ролям (`patroni_proxy`) | Да | Нет | Нет |
| Защита от лага реплик | Да (`max_lag_in_bytes` в `patroni_proxy`) | Нет | Да (`watchdog_lag_query` + `catchup_timeout`) |
| Несколько хостов PostgreSQL с балансировкой | Да (`patroni_proxy`) | Да (с 1.24, `load_balance_hosts`) | Да |
| `target_session_attrs` (read-write / read-only routing) | Да (через роли `patroni_proxy`) | Нет | Да |
| Sequential routing rules (правило-в-порядке-первое-совпадение) | Нет | Нет | Да |
| Маршрутизация по типу соединения (TCP vs UNIX) | Нет | Нет | Да |
| Выбор хоста с учётом availability zone | Нет | Нет | Да |

См. [Fallback через Patroni](tutorials/patroni-assisted-fallback.md), [`patroni_proxy`](tutorials/patroni-proxy.md).

## Пулинг

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Режимы пула | session, transaction | session, transaction, statement | session, transaction |
| Координатор пулов (лимит на базу с приоритетным вытеснением) | Да (`max_db_connections` + вытеснение по p95) | Нет (`max_db_connections` ставит клиентов в очередь, пока существующие соединения не закроются по idle timeout) | Нет |
| Резервный пул | Да (`reserve_pool_size`) | Да (`reserve_pool_size`) | Нет |
| Per-user `min_guaranteed_pool_size` | Да | Нет | Нет |
| Опережающая замена при истечении `server_lifetime` (warm-up до экспирации старого) | Да (порог 95%, до 3 параллельных) | Нет | Нет |
| Упреждающее ожидание и ограничение всплеска (`scaling_warm_pool_ratio`, быстрые повторы) | Да | Нет | Нет |
| Прямая передача (возвращающееся соединение уходит самому давно ждущему клиенту через in-process oneshot-канал) | Да | Нет | Нет |
| Строгий FIFO порядок ожидающих | Да | Нет (LIFO через `server_round_robin = 0`) | Нет |
| `min_pool_size` (warm connections) | Да | Нет | Да |
| Prepared statements в transaction mode | Да (именованные и анонимные, двухуровневый кеш, query interner) | Да (именованные, с 1.21, `max_prepared_statements`) | Да (именованные, `pool_reserve_prepared_statement`) |
| Кеш анонимного `Parse` для производительности | Да (`DOORMAN_N`, переиспользование между клиентами пула) | Нет (анонимный `Parse` проходит без изменений) | Нет (требуются именованные prepared statements) |
| Умная очистка при возврате соединения (пропустить `DEALLOCATE ALL`, если кеш не менялся) | Да (`RESET ALL` / `DEALLOCATE ALL` по факту мутаций) | Нет (всегда `DISCARD ALL`, если задан `server_reset_query`) | Да (auto) |
| LISTEN / NOTIFY pinning в transaction mode | Нет | Нет | Экспериментально |
| Cross-rule connection cap (`shared_pool`) | Нет | Нет | Да (с 1.5.1) |
| Команды администратора `PAUSE` / `RESUME` / `RECONNECT` | Да | Да | Да (с 1.4.1) |
| GUC PostgreSQL на уровне пула в backend `StartupMessage` | Да (`startup_parameters`: `general` → пул → passthrough `auth_query`; клиентские `RESET ALL` / `DISCARD ALL` возвращают эти значения; ошибки PG при запуске бэкенда доходят до клиента без переписывания) | Нет эквивалентных операторских значений по умолчанию; отдельные клиентские startup-параметры можно отслеживать или игнорировать | Нет (`maintain_params` сохраняет клиентские параметры при rebind; операторских GUC нет) |

См. [Координатор пулов](concepts/pool-coordinator.md), [Пул под нагрузкой](tutorials/pool-pressure.md).

## Лимиты и таймауты

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| `server_idle_check_timeout` (probe перед checkout) | Да | Нет | Нет |
| `idle_timeout` (server-side) | Да (`idle_timeout`) | Да (`server_idle_timeout`) | Да |
| `server_lifetime` | Да | Да | Да |
| `query_wait_timeout` | Да | Да | Да |
| `client_idle_timeout` | Нет | Да (с 1.24) | Нет |
| `transaction_timeout` (enforced пулером) | Нет | Да (с 1.25) | Нет |
| `max_user_client_connections` | Нет | Да (с 1.24) | Нет |
| `max_db_client_connections` | Нет | Да (с 1.24) | Нет |
| Per-user `query_timeout` | Нет | Да (с 1.24) | Нет |
| Per-user `reserve_pool_size` | Нет | Да (с 1.24) | Нет |
| Уведомление клиента, пока тот ждёт серверное соединение | Нет | Да (с 1.25, `query_wait_notify`) | Да (`pool_notice_after_waiting_ms`) |

См. [Справочник по general-настройкам](reference/general.md), [Справочник по pool-настройкам](reference/pool.md).

## Наблюдаемость

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Встроенная веб-консоль администратора | Да (HTML-консоль на том же порту, что и `/metrics`, включается через `[web].ui`) | Нет (только psql admin-консоль) | Нет (только psql admin-консоль) |
| Prometheus-эндпоинт | Встроенный `/metrics` | Внешний (`pgbouncer_exporter`) | Внешний (Go-exporter sidecar, опрашивает admin-консоль) |
| Перцентили задержки на пул (p50, p90, p95, p99) | Да (HDR Histogram) | Нет (только средние в `SHOW STATS`) | Да через exporter (TDigest, требует rule-опцию `quantiles`) |
| Счётчики prepared statements в `SHOW STATS` | Да | Да (с 1.24) | Нет |
| Структурированные JSON-логи | Да (`--log-format structured`) | Нет | Да (`log_format "json"`) |
| Управление уровнем логов в рантайме (`SET log_level`) | Да | Нет | Нет |
| `SHOW POOL_COORDINATOR` / `SHOW POOL_SCALING` / `SHOW SOCKETS` | Да | Нет | Нет |
| `SHOW PREPARED_STATEMENTS` | Да | Нет | Нет |
| `SHOW INTERNER` (записи / байты / предпросмотр по половинам) | Да | Нет | Нет |
| Ограниченный prepared-кеш (TTL у анонимных, клиентский LRU с разделением Named/Anonymous) | Да | Нет (только named, неограниченный `max_prepared_statements`) | Нет |
| `SHOW HOSTS` (CPU/память хоста) | Нет | Нет | Да |
| `SHOW RULES` (дамп активной маршрутизации) | Нет | Нет | Да |
| Метрики server-side TLS-соединений (длительность handshake, ошибки, активные) | Да | Нет | Нет |
| Метрики Patroni API | Да | Нет | Нет |
| Метрики fallback (active flag, текущий хост, hits) | Да | Нет | Нет |

См. [Справочник Prometheus-метрик](reference/prometheus.md), [Команды администратора](observability/admin-commands.md).

## Эксплуатация

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Обновление бинаря с миграцией сессий (TCP-сокет, cancel keys, prepared cache) | Да (`SCM_RIGHTS`, плюс TLS state со сборкой `tls-migration`) | Нет: `-R` deprecated с 1.20; rolling restart через `so_reuseport` оставляет старые сессии на старом процессе | Нет: `SIGUSR2` + `bindwith_reuseport` оставляет старые сессии на старом процессе |
| Формат конфига | YAML или TOML | INI | Свой формат (lex/yacc) |
| Человекочитаемые длительности и размеры (`30s`, `1h`, `256MB`) | Да | Нет (целые микросекунды / байты) | Нет |
| Режим проверки конфига (`pg_doorman -t`) | Да | Нет | Нет |
| Авто-конфиг из PostgreSQL (`pg_doorman generate --host`) | Да | Нет | Нет |
| Перезагрузка по `SIGHUP` | Да (серверные TLS-сертификаты включены; клиентский TLS требует рестарта) | Да (`auth_file`, `auth_hba_file`, server и client TLS certs) | Да |
| systemd `sd-notify` (`Type=notify`) | Да | Нет | Нет |
| Лимит памяти (`max_memory_usage`) | Да | Нет | Нет |

См. [Плавное обновление бинаря](tutorials/binary-upgrade.md), [Сигналы](operations/signals.md).

## Протокол

| Возможность | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Simple query | Да | Да | Да |
| Extended query | Да | Да | Частично |
| Pipelined batches | Да | Да | Частично |
| Async Flush | Да | Да | Нет |
| Cancel requests поверх TLS | Да | Да | Да |
| `COPY IN` / `COPY OUT` | Да | Да | Да |
| Replication passthrough (`replication=true` startup) | Нет | Да (с 1.23) | Нет |
| Согласование версии протокола (3.2) | Нет | Да (с 1.23) | Нет |
| `server_drop_on_cached_plan_error` | Нет | Нет | Да (с 1.5.1) |

## Когда PgDoorman не подойдёт

- **Нужна LDAP-аутентификация.** Используйте Odyssey или PgBouncer 1.25+.
- **Нужен replication passthrough для logical replication tools.** Используйте PgBouncer 1.23+.
- **Нужен `transaction_timeout`, который применяет сам пулер.** Используйте PgBouncer 1.25+.
- **Нужен горизонтальный шардинг внутри пулера.** Используйте PgCat.

Если нужны prepared statements в transaction mode, Patroni HA без внешних прокси, многопоточная пропускная способность с одним общим пулом и обновление бинаря с миграцией живых сессий — PgDoorman ближе по профилю.
