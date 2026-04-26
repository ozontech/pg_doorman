# PgDoorman vs PgBouncer vs Odyssey vs PgCat

Практическая матрица фич для выбора пулера соединений PostgreSQL. PgDoorman нацелен на нагрузки, где важны prepared statements в транзакционном режиме, многопоточная производительность и удобство эксплуатации.

Числа бенчмарков — см. [Benchmarks](benchmarks.md).

## Аутентификация

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| MD5 пароль | Да | Да | Да |
| SCRAM-SHA-256 (клиент) | Да | Да | Да |
| Passthrough SCRAM-SHA-256 (без пароля в открытом виде в конфиге) | Да | Нет | Да |
| Passthrough MD5 | Да | Да | Да |
| `auth_query` (динамические пользователи) | Да | Да | Да |
| Режим passthrough в auth_query (per-user идентичность бэкенда) | Да | Нет | Да |
| Формат `pg_hba.conf` | Да (файл или inline) | Нет | Начиная с 1.4 |
| PAM | Да (Linux) | Да (HBA) | Да |
| JWT (RSA-SHA256) | Да | Нет | Нет |
| Talos (кастомный JWT с извлечением роли) | Да | Нет | Нет |
| LDAP | Нет | Начиная с 1.25 | Да |
| SCRAM channel binding (`scram-sha-256-plus`) | Нет | Да | Да |
| Маппинг имён пользователей (cert/peer → DB user) | Нет | Начиная с 1.23 | Да |
| Настраиваемый `scram_iterations` | Нет | Начиная с 1.25 | Нет |

См. [Аутентификация](authentication/overview.md).

## TLS

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Client-side TLS (4 режима: disable, allow, require, verify-full) | Да | Да | Да |
| Server-side TLS к PostgreSQL (6 режимов, включая verify-ca, verify-full) | Да | Да | Нет |
| mTLS к PostgreSQL (клиентский сертификат) | Да | Да | Нет |
| Горячая перезагрузка TLS-сертификатов по `SIGHUP` | Да (server-side) | Нет | Нет |
| Минимум TLS 1.2 + список шифров Mozilla | Да | Да | Нет (разрешает TLS 1.0) |
| Direct TLS handshake (PG17, без `SSLRequest`) | Нет | Начиная с 1.25 | Нет |
| Управление шифрами TLS 1.3 | Нет | Начиная с 1.25 | Нет |

См. [TLS](guides/tls.md).

## Маршрутизация и высокая доступность

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Patroni-assisted fallback (встроенный поиск через `/cluster`) | Да | Нет | Нет |
| Встроенный TCP-прокси с маршрутизацией по ролям (`patroni_proxy`) | Да | Нет | Нет |
| Защита от отставания реплики | Да (`max_lag_in_bytes` в `patroni_proxy`) | Нет | Да (watchdog-запрос) |
| Round-robin / least-connections для нескольких хостов | Да (`patroni_proxy`) | Начиная с 1.24 | Да |
| `target_session_attrs` (read-write / read-only) | Да (через роли в `patroni_proxy`) | Нет | Да |
| Последовательная маршрутизация (правила по порядку) | Нет | Нет | Да |
| Маршрутизация по типу соединения (TCP vs UNIX) | Нет | Нет | Да |
| Выбор хоста с учётом availability zone | Нет | Нет | Да |

См. [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md), [`patroni_proxy`](tutorials/patroni-proxy.md).

## Пулинг

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Режимы пула (transaction, session) | Да | Да (+ statement) | Да |
| Pool Coordinator (cross-user `max_db_connections` с приоритетным вытеснением) | Да | Нет (без вытеснения) | Нет |
| Резервный пул с `min_guaranteed_pool_size` | Да | Только reserve | Нет |
| Опережающая замена при истечении `server_lifetime` | Да | Нет | Нет |
| Опережающее создание / burst scaling (`scaling_warm_pool_ratio`, быстрые retry) | Да | Нет | Нет |
| Direct-handoff (ожидающий получает возвращаемое соединение за микросекунды) | Да | Нет | Нет |
| `min_pool_size` (прогретые соединения) | Да | Нет | Да |
| Кэш prepared statements (двухуровневый, query interner, statement remap) | Да | Начиная с 1.21 | Начиная с 1.3 |
| Умный `DISCARD` при возврате | RESET ALL + сброс кэша | Нет | Да (авто) |
| Прикрепление LISTEN / NOTIFY в транзакционном режиме | Нет | Нет | Экспериментально |
| Cross-rule ограничение соединений (`shared_pool`) | Нет | Нет | Начиная с 1.5.1 |
| `PAUSE` / `RESUME` / `RECONNECT` | Да | Да | Да (1.4.1+) |

См. [Pool Coordinator](concepts/pool-coordinator.md), [Пул под нагрузкой](tutorials/pool-pressure.md).

## Лимиты и таймауты

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| `server_idle_check_timeout` (проверка перед выдачей) | Да | Нет | Нет |
| `idle_timeout` (серверное соединение) | Да | Да | Да |
| `server_lifetime` | Да | Да | Да |
| `query_wait_timeout` | Да | Да | Да |
| `client_idle_timeout` | Нет | Начиная с 1.24 | Нет |
| `transaction_timeout` | Нет | Начиная с 1.25 | Нет |
| `max_user_client_connections` | Нет | Начиная с 1.24 | Нет |
| Per-user `query_timeout` | Нет | Начиная с 1.24 | Нет |
| Per-user `reserve_pool_size` | Нет | Начиная с 1.24 | Нет |
| `query_wait_notify` (NOTICE при ожидании бэкенда) | Нет | Начиная с 1.25 | Да (`pool_notice_after_waiting_ms`) |

См. [Справочник по general settings](reference/general.md), [Справочник по pool settings](reference/pool.md).

## Observability

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Встроенный Prometheus-эндпоинт | Да | Внешний (`pgbouncer_exporter`) | Да |
| Перцентили задержки на пул (p50, p90, p95, p99) | Да (HDR Histogram) | Нет | Да (TDigest) |
| Счётчики prepared statements в статистике | Да | Начиная с 1.24 | Нет |
| JSON структурированное логирование | Да (`--log-format Structured`) | Нет | Да |
| Управление уровнем логирования в рантайме (`SET log_level`) | Да | Нет | Нет |
| Admin `SHOW POOL_COORDINATOR` / `SHOW POOL_SCALING` / `SHOW SOCKETS` | Да | Нет | Нет |
| Admin `SHOW PREPARED_STATEMENTS` | Да | Нет | Нет |
| Admin `SHOW HOSTS` (CPU/память хоста) | Нет | Нет | Да |
| Admin `SHOW RULES` (дамп маршрутизации) | Нет | Нет | Да |
| Метрики TLS-соединений (длительность handshake, ошибки, активные) | Да (server-side) | Нет | Нет |
| Метрики Patroni API | Да | Нет | Нет |
| Метрики fallback (флаг активности, текущий хост, попадания) | Да | Нет | Нет |

См. [Справочник по Prometheus-метрикам](reference/prometheus.md), [Admin-команды](observability/admin-commands.md).

## Эксплуатация

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Graceful binary upgrade (zero-downtime, in-flight клиенты сохраняются) | Да | Ограниченно (`SO_REUSEPORT`) | Нет |
| YAML конфиг | Да | Нет (INI) | Нет (свой формат) |
| TOML конфиг | Да (legacy) | Нет | Нет |
| Человекочитаемые длительности и размеры (`30s`, `1h`, `256MB`) | Да | Нет | Нет |
| Режим проверки конфига (`pg_doorman -t`) | Да | Нет | Нет |
| Авто-конфиг из PostgreSQL (`pg_doorman generate --host`) | Да | Нет | Нет |
| Перезагрузка по `SIGHUP` | Да (включая server TLS-сертификаты) | Да | Да |
| Интеграция с systemd `sd-notify` | Да (`Type=notify`) | Нет | Нет |
| Лимит памяти (`max_memory_usage`) | Да | Нет | Нет |

См. [Binary upgrade](tutorials/binary-upgrade.md), [Сигналы](operations/signals.md).

## Протокол

| Фича | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Simple query | Да | Да | Да |
| Extended query | Да | Да | Частично |
| Pipelined batches | Да | Да | Частично |
| Async Flush | Да | Да | Нет |
| Cancel-запросы поверх TLS | Да | Да | Да |
| `COPY IN` / `COPY OUT` | Да | Да | Да |
| Проброс replication-соединений (`replication=true` в startup) | Нет | Начиная с 1.23 | Нет |
| Поддержка версии протокола 3.2 | Нет | Начиная с 1.23 | Нет |
| `server_drop_on_cached_plan_error` | Нет | Нет | Начиная с 1.5.1 |

## Когда PgDoorman не подходит

- Нужна аутентификация LDAP. Используйте Odyssey или PgBouncer 1.25+.
- Нужен SCRAM channel binding (`scram-sha-256-plus`) end-to-end. Используйте PgBouncer или Odyssey.
- Нужна сквозная replication для инструментов логической репликации. Используйте PgBouncer 1.23+.
- Нужна маршрутизация с учётом availability zone или последовательные правила в стиле `pg_hba`. Используйте Odyssey.
- Нужно, чтобы `transaction_timeout` принудительно применялся пулером. Используйте PgBouncer 1.25+.

Если важны prepared statements в транзакционном режиме, Patroni HA без внешних прокси, многопоточный throughput и перезапуски без простоя — PgDoorman подходит ближе.
