# Настройки Prometheus

pg_doorman экспортирует метрики в формате Prometheus о производительности и состоянии пулов соединений.

## Включение метрик Prometheus

Чтобы включить экспортёр метрик Prometheus, добавьте в конфигурационный файл следующее:

```yaml
prometheus:
  enabled: true
  host: "0.0.0.0"  # Хост, на котором сервер метрик будет принимать соединения
  port: 9127       # Порт, на котором сервер метрик будет принимать соединения
```

### Опции конфигурации

| Опция | Описание | По умолчанию |
|-------|----------|--------------|
| `enabled` | Включить или отключить экспортёр метрик Prometheus. | `false` |
| `host` | Хост, на котором экспортёр метрик Prometheus будет принимать соединения. | `"0.0.0.0"` |
| `port` | Порт, на котором экспортёр метрик Prometheus будет принимать соединения. | `9127` |

## Настройка Prometheus

Добавьте следующий job в конфигурацию Prometheus, чтобы собирать метрики с pg_doorman:

```yaml
scrape_configs:
  - job_name: 'pg_doorman'
    static_configs:
      - targets: ['<pg_doorman_host>:9127']
```

Замените `<pg_doorman_host>` на имя хоста или IP-адрес вашего инстанса pg_doorman.

## Доступные метрики

pg_doorman экспортирует следующие метрики:

### Системные метрики

| Метрика | Описание |
|---------|----------|
| `pg_doorman_total_memory` | Общий объём памяти, выделенный процессу pg_doorman, в байтах. Позволяет отслеживать потребление памяти приложением. |

### Метрики соединений

| Метрика | Описание |
|---------|----------|
| `pg_doorman_connections_total` | Накопительный счётчик принятых клиентских соединений по типу: `plain` (без TLS), `tls`, `cancel` (запрос отмены), `total` (сумма). Для темпа подключений используйте `rate(pg_doorman_connections_total[5m])`. |
| `pg_doorman_connection_count` | Устаревшая gauge-версия `pg_doorman_connections_total`; будет удалена в 3.10. Новые правила и панели должны использовать `pg_doorman_connections_total`. |

### Метрики сокетов (только Linux)

| Метрика | Описание |
|---------|----------|
| `pg_doorman_sockets` | Счётчик сокетов, используемых pg_doorman, по типу сокета. Типы: 'tcp' (IPv4 TCP-сокеты), 'tcp6' (IPv6 TCP-сокеты), 'unix' (Unix domain sockets), 'unknown' (сокеты нераспознанного типа). Доступно только в Linux. |

### Метрики пула

| Метрика | Описание |
|---------|----------|
| `pg_doorman_pools_clients` | Число клиентов в пулах соединений по статусу, пользователю и базе. Значения статуса: `idle` (подключён, но не выполняет запросы), `waiting` (ждёт серверного соединения), `active` (выполняет запросы). |
| `pg_doorman_pools_servers` | Число серверов в пулах соединений по статусу, пользователю и базе. Значения статуса: `active` (обслуживает клиента) и `idle` (свободен для новых соединений). |
| `pg_doorman_pools_bytes_total` | Накопительный счётчик байт, переданных через пулы соединений, по направлению (`received`/`sent`), пользователю и базе. Для пропускной способности используйте `rate(pg_doorman_pools_bytes_total[5m])`. |
| `pg_doorman_pools_bytes` | Устаревшая gauge-версия `pg_doorman_pools_bytes_total`; будет удалена в 3.10. |
| `pg_doorman_pool_size` | Сконфигурированный максимальный размер пула на пользователя и базу. Полезен для расчёта оставшейся ёмкости пула вместе с pg_doorman_pools_servers. |
| `pg_doorman_backend_startup_parameter_errors_total` | Накопительный счётчик запусков бэкенда, которые PostgreSQL отклонил из-за `startup_parameters`. Лейблы: пул и SQLSTATE. Отклонённый параметр и имя пользователя пишутся в строку лога уровня `warn`, а не в лейблы метрики. |
| `pg_doorman_startup_parameters_dropped_total` | Накопительный счётчик событий, когда pg_doorman отбросил `startup_parameters` до отправки `StartupMessage`. Лейблы: пул и причина (`cascade_budget_exceeded`, `packet_cap_exceeded`, `auth_query_oversize`, `auth_query_overlay_oversize`, `auth_query_bad_type`, `auth_query_invalid_json`, `auth_query_invalid_shape`, `auth_query_invalid_entry`, `dedicated_mode`). |

### Метрики запросов и транзакций

| Метрика | Описание |
|---------|----------|
| `pg_doorman_pools_query_duration_seconds` | Гистограмма времени выполнения запросов на стороне PostgreSQL по пулу, в секундах. Квантили считайте через `histogram_quantile(q, sum by (le, user, database) (rate(pg_doorman_pools_query_duration_seconds_bucket[5m])))`; QPS — через `rate(..._count[5m])`. |
| `pg_doorman_pools_transaction_duration_seconds` | Гистограмма полного времени транзакций по пулу, в секундах. Агрегируется так же, как `pg_doorman_pools_query_duration_seconds`. |
| `pg_doorman_pools_wait_duration_seconds` | Гистограмма времени ожидания выдачи backend-соединения клиенту, в секундах. Для p99 используйте `histogram_quantile(0.99, ...)`. |
| `pg_doorman_pools_transactions_total` | Накопительный счётчик транзакций по пулу. Для TPS используйте `rate(pg_doorman_pools_transactions_total[5m])`. |
| `pg_doorman_pools_queries_percentile` | Устаревшая метрика; будет удалена в 3.10. Это заранее посчитанные перцентили, которые нельзя корректно суммировать между репликами. Используйте `pg_doorman_pools_query_duration_seconds_bucket` и `histogram_quantile()`. |
| `pg_doorman_pools_transactions_percentile` | Устаревшая метрика; будет удалена в 3.10. Используйте `pg_doorman_pools_transaction_duration_seconds`. |
| `pg_doorman_pools_transactions_count` | Устаревшая gauge-версия `pg_doorman_pools_transactions_total`; будет удалена в 3.10. |
| `pg_doorman_pools_transactions_total_time` | Сумма времени выполнения транзакций в пулах соединений, по пользователю и базе. В миллисекундах. |
| `pg_doorman_pools_queries_total` | Накопительный счётчик запросов по пулу. Для QPS используйте `rate(pg_doorman_pools_queries_total[5m])`. |
| `pg_doorman_pools_queries_count` | Устаревшая gauge-версия `pg_doorman_pools_queries_total`; будет удалена в 3.10. |
| `pg_doorman_pools_queries_total_time` | Сумма времени выполнения запросов в пулах соединений, по пользователю и базе. В миллисекундах. |
| `pg_doorman_pools_avg_wait_time` | Устаревшая метрика; будет удалена в 3.10. Это среднее значение, которое сглаживает пики хвостовой задержки. Используйте `pg_doorman_pools_wait_duration_seconds_bucket` и `histogram_quantile()`. |

### Метрики auth_query

Эти метрики доступны только когда `auth_query` сконфигурирован для одного или нескольких пулов.

| Метрика | Описание |
|---------|----------|
| `pg_doorman_auth_query_cache_total` | Накопительные события кеша auth_query по типу (`hits`/`misses`/`refetches`/`rate_limited`) и базе. Текущее число записей остаётся в `pg_doorman_auth_query_cache{type="entries"}`. |
| `pg_doorman_auth_query_auth_total` | Накопительный счётчик результатов auth_query-аутентификации по `result` (`success`/`failure`) и базе. |
| `pg_doorman_auth_query_executor_total` | Накопительный счётчик событий исполнителя auth_query по типу (`queries`/`errors`) и базе. |
| `pg_doorman_auth_query_dynamic_pools_total` | Накопительный счётчик событий жизненного цикла динамических пулов auth_query по типу (`created`/`destroyed`) и базе. Текущее число пулов остаётся в `pg_doorman_auth_query_dynamic_pools{type="current"}`. |
| `pg_doorman_auth_query_cache` | Текущее число закешированных учётных данных (`type="entries"`). Накопительные значения в этой метрике устарели; используйте `pg_doorman_auth_query_cache_total`. |
| `pg_doorman_auth_query_auth` | Устаревшая gauge-версия `pg_doorman_auth_query_auth_total`; будет удалена в 3.10. |
| `pg_doorman_auth_query_executor` | Устаревшая gauge-версия `pg_doorman_auth_query_executor_total`; будет удалена в 3.10. |
| `pg_doorman_auth_query_dynamic_pools` | Метрики жизненного цикла динамических пулов auth_query по типу и базе. Типы: `current` (сейчас активные динамические пулы), `created` (всего создано с момента старта), `destroyed` (всего удалено сборщиком или при RELOAD). Имеет смысл только в passthrough mode. |

### Метрики серверов

| Метрика | Описание |
|---------|----------|
| `pg_doorman_servers_prepared_hits` | Совокупное число попаданий в кеш prepared statements по всем бэкендам пула, с лейблами `user` и `database`. Используется вместе с `pg_doorman_servers_prepared_misses` для расчёта доли попаданий. |
| `pg_doorman_servers_prepared_misses` | Совокупное число промахов prepared statements по всем бэкендам пула, с лейблами `user` и `database`. Устойчивая ненулевая скорость означает, что запросы часто готовятся заново или кеш `server_prepared_statement_cache_size` слишком мал. |

### Метрики серверного TLS

Активны, если включён TLS к PostgreSQL (`server_tls_mode != "disable"`).

| Метрика | Тип | Описание |
|---------|-----|----------|
| `pg_doorman_server_tls_connections` | gauge по пулу | Число активных TLS-соединений к PostgreSQL. |
| `pg_doorman_server_tls_handshake_duration_seconds` | histogram по пулу | Распределение длительности TLS handshake. |
| `pg_doorman_server_tls_handshake_errors_total` | counter по пулу | Счётчик неуспешных TLS handshake. Алертить при ненулевой скорости. |

Подробнее — см. [Клиентский и серверный TLS](../guides/tls.md#Мониторинг).

## Дашборд Grafana

Базовый набор панелей для дашборда:

1. Число соединений по типу
2. Использование памяти во времени
3. Число клиентов и серверов по пулам
4. Перцентили запросов и транзакций
5. Сетевой трафик по пулам

## Примеры запросов

Несколько примеров запросов Prometheus, которые могут быть полезны:

### Темп подключений

```
rate(pg_doorman_connections_total{type="total"}[5m])
```

### Загрузка пула

```
sum by (database) (pg_doorman_pools_clients{status="active"}) / sum by (database) (pg_doorman_pools_servers{status="active"} + pg_doorman_pools_servers{status="idle"})
```

### Медленные запросы (p99)

```
histogram_quantile(0.99, sum by (le, user, database) (rate(pg_doorman_pools_query_duration_seconds_bucket[5m])))
```

### Время ожидания клиентов (p99)

```
histogram_quantile(0.99, sum by (le, user, database) (rate(pg_doorman_pools_wait_duration_seconds_bucket[5m])))
```

### Hit rate кеша auth_query

```
rate(pg_doorman_auth_query_cache_total{type="hits"}[5m]) / clamp_min(rate(pg_doorman_auth_query_cache_total{type="hits"}[5m]) + rate(pg_doorman_auth_query_cache_total{type="misses"}[5m]), 0.001)
```

### Темп ошибок auth_query

```
rate(pg_doorman_auth_query_auth_total{result="failure"}[5m])
```
