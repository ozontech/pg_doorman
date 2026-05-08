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
| `pg_doorman_connections_total` | Кумулятивный счётчик принятых клиентских соединений по типу. Типы: `plain` (без шифрования), `tls` (TLS), `cancel` (запросы отмены при стартапе), `total` (сумма). Counter; для скорости используйте `rate(pg_doorman_connections_total[5m])`. |
| `pg_doorman_connection_count` | DEPRECATED, удаляется в 3.10. Gauge-зеркало `pg_doorman_connections_total` оставлено на один минор. Новые правила и панели должны читать counter. |

### Метрики сокетов (только Linux)

| Метрика | Описание |
|---------|----------|
| `pg_doorman_sockets` | Счётчик сокетов, используемых pg_doorman, по типу сокета. Типы: 'tcp' (IPv4 TCP-сокеты), 'tcp6' (IPv6 TCP-сокеты), 'unix' (Unix domain sockets), 'unknown' (сокеты нераспознанного типа). Доступно только в Linux. |

### Метрики пула

| Метрика | Описание |
|---------|----------|
| `pg_doorman_pools_clients` | Число клиентов в пулах соединений по статусу, пользователю и базе. Значения статуса: `idle` (подключён, но не выполняет запросы), `waiting` (ждёт серверного соединения), `active` (выполняет запросы). |
| `pg_doorman_pools_servers` | Число серверов в пулах соединений по статусу, пользователю и базе. Значения статуса: `active` (обслуживает клиента) и `idle` (свободен для новых соединений). |
| `pg_doorman_pools_bytes_total` | Кумулятивные байты, переданные через пулы соединений, по направлению (`received`/`sent`), пользователю и базе. Counter; для пропускной способности — `rate(pg_doorman_pools_bytes_total[5m])`. |
| `pg_doorman_pools_bytes` | DEPRECATED, удаляется в 3.10. Gauge-зеркало `pg_doorman_pools_bytes_total`. |

| `pg_doorman_pool_size` | Сконфигурированный максимальный размер пула на пользователя и базу. Полезен для расчёта оставшейся ёмкости пула вместе с pg_doorman_pools_servers. |

### Метрики запросов и транзакций

| Метрика | Описание |
|---------|----------|
| `pg_doorman_pools_query_duration_seconds` | Гистограмма времени выполнения запросов на стороне PostgreSQL по пулу, в секундах. Квантили: `histogram_quantile(q, sum by (le, user, database) (rate(pg_doorman_pools_query_duration_seconds_bucket[5m])))`. QPS — `rate(_count[5m])`. |
| `pg_doorman_pools_transaction_duration_seconds` | Гистограмма полного времени транзакций по пулу, в секундах. Контракт композиции тот же. |
| `pg_doorman_pools_wait_duration_seconds` | Гистограмма времени ожидания клиентского checkout по пулу, в секундах. Для tail-wait — `histogram_quantile(0.99, ...)`. |
| `pg_doorman_pools_transactions_total` | Кумулятивный счётчик транзакций по пулу. Counter; TPS — `rate(pg_doorman_pools_transactions_total[5m])`. |
| `pg_doorman_pools_queries_percentile` | DEPRECATED, удаляется в 3.10. Pre-aggregated gauge перцентилей — не суммируется между репликами. Используйте `pg_doorman_pools_query_duration_seconds_bucket` с `histogram_quantile()`. |
| `pg_doorman_pools_transactions_percentile` | DEPRECATED, удаляется в 3.10. См. `pg_doorman_pools_transaction_duration_seconds`. |
| `pg_doorman_pools_transactions_count` | DEPRECATED, удаляется в 3.10. Gauge-зеркало `pg_doorman_pools_transactions_total`. |
| `pg_doorman_pools_transactions_total_time` | Сумма времени выполнения транзакций в пулах соединений, по пользователю и базе. В миллисекундах. |
| `pg_doorman_pools_queries_total` | Кумулятивный счётчик запросов по пулу. Counter; QPS — `rate(pg_doorman_pools_queries_total[5m])`. |
| `pg_doorman_pools_queries_count` | DEPRECATED, удаляется в 3.10. Gauge-зеркало `pg_doorman_pools_queries_total`. |
| `pg_doorman_pools_queries_total_time` | Сумма времени выполнения запросов в пулах соединений, по пользователю и базе. В миллисекундах. |
| `pg_doorman_pools_avg_wait_time` | DEPRECATED, удаляется в 3.10. Running mean, который замывает tail-spikes ожидания. Используйте `pg_doorman_pools_wait_duration_seconds_bucket` с `histogram_quantile()`. |

### Метрики auth_query

Эти метрики доступны только когда `auth_query` сконфигурирован для одного или нескольких пулов.

| Метрика | Описание |
|---------|----------|
| `pg_doorman_auth_query_cache_total` | Кумулятивные события кеша auth_query по типу (`hits`/`misses`/`refetches`/`rate_limited`) и базе. Counter; снапшот `entries` остаётся на `pg_doorman_auth_query_cache`. |
| `pg_doorman_auth_query_auth_total` | Кумулятивные результаты аутентификации auth_query по `result` (`success`/`failure`) и базе. Counter. |
| `pg_doorman_auth_query_executor_total` | Кумулятивные события executor auth_query по типу (`queries`/`errors`) и базе. Counter. |
| `pg_doorman_auth_query_dynamic_pools_total` | Кумулятивные события жизненного цикла динамических пулов auth_query по типу (`created`/`destroyed`) и базе. Counter; снапшот `current` остаётся на `pg_doorman_auth_query_dynamic_pools`. |
| `pg_doorman_auth_query_cache` | Снапшот `entries` (текущее число закешированных учётных данных). Кумулятивные члены deprecated в этой метрике — используйте `pg_doorman_auth_query_cache_total`. |
| `pg_doorman_auth_query_auth` | DEPRECATED, удаляется в 3.10. Gauge-зеркало `pg_doorman_auth_query_auth_total`. |
| `pg_doorman_auth_query_executor` | DEPRECATED, удаляется в 3.10. Gauge-зеркало `pg_doorman_auth_query_executor_total`. |
| `pg_doorman_auth_query_dynamic_pools` | Метрики жизненного цикла динамических пулов auth_query по типу и базе. Типы: `current` (сейчас активные динамические пулы), `created` (всего пулов создано с момента старта), `destroyed` (всего пулов, собранных garbage collection или удалённых на RELOAD). Имеет смысл только в passthrough mode. |

### Метрики серверов

| Метрика | Описание |
|---------|----------|
| `pg_doorman_servers_prepared_hits` | Совокупное число попаданий в кэш prepared statement по всем бэкендам пула, с лейблами user и database. Сравните с `pg_doorman_servers_prepared_misses` для расчёта hit-ratio. |
| `pg_doorman_servers_prepared_misses` | Совокупное число промахов prepared statement по всем бэкендам пула, с лейблами user и database. Устойчиво ненулевая скорость указывает на запросы, для которых стоит включить prepare, либо на нехватку `server_prepared_statement_cache_size`. |

### Метрики серверного TLS

Активны, если включён TLS к PostgreSQL (`server_tls_mode != "disable"`).

| Метрика | Тип | Описание |
|---------|-----|----------|
| `pg_doorman_server_tls_connections` | gauge per pool | Число активных TLS-соединений к PostgreSQL. |
| `pg_doorman_server_tls_handshake_duration_seconds` | histogram per pool | Распределение длительности TLS handshake. |
| `pg_doorman_server_tls_handshake_errors_total` | counter per pool | Счётчик неуспешных handshake. Алертить при ненулевой скорости. |

Подробнее — см. [Клиентский и серверный TLS](../guides/tls.md#Observability).

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
