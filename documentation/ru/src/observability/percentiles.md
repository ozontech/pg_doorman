# Перцентили задержек

pg_doorman измеряет задержки запросов и транзакций на пул, используя HDR Histogram. В Prometheus экспортируются четыре перцентиля: p50, p90, p95, p99.

Эта страница объясняет, откуда берутся числа и как их читать.

## Что измеряется

Три серии задержек на пару user×database:

| Серия | Что покрывает |
| --- | --- |
| `query_histogram` | Время от старта запроса до его завершения на бэкенде. Измеряет время выполнения PostgreSQL так, как его видит pg_doorman. |
| `xact_histogram` | Время от `BEGIN` (или первого оператора неявной транзакции) до `COMMIT` / `ROLLBACK`. |
| `wait_histogram` | Время, которое клиент провёл в ожидании, пока соединение с бэкендом не освободится. |

`wait_histogram` — собственный вклад пула в задержку. Если p99 у `wait_histogram` высокий, а p99 у `query_histogram` низкий, узкое место — получение соединения, а не PostgreSQL.

## Детали гистограммы

pg_doorman использует [HDR Histogram](https://github.com/HdrHistogram/HdrHistogram_rust) с параметрами:

- Максимальное значение: 10 минут (600 секунд).
- Значащих цифр: 2 (около 0,1% относительной погрешности).

Расход памяти: около 10 KB на гистограмму. Три гистограммы на пару user×database — это ~30 KB на пул, что комфортно для сотен пулов.

По умолчанию горизонт отчёта — время жизни процесса. Гистограммы сбрасываются на `SIGHUP` (перезагрузка конфига) и при явном `RECONNECT`.

Odyssey использует TDigest, PgBouncer перцентили не экспортирует. HDR предпочтителен, когда вы знаете верхнюю границу (10 минут — щедрый запас для пула соединений); TDigest работает с неограниченными потоками.

## Экспорт в Prometheus

```
# HELP pg_doorman_pools_queries_percentile Query latency percentiles in milliseconds
# TYPE pg_doorman_pools_queries_percentile gauge
pg_doorman_pools_queries_percentile{percentile="50",user="app",database="mydb"} 1.2
pg_doorman_pools_queries_percentile{percentile="90",user="app",database="mydb"} 4.7
pg_doorman_pools_queries_percentile{percentile="95",user="app",database="mydb"} 8.1
pg_doorman_pools_queries_percentile{percentile="99",user="app",database="mydb"} 24.5

# HELP pg_doorman_pools_transactions_percentile Transaction latency percentiles in milliseconds
# TYPE pg_doorman_pools_transactions_percentile gauge
pg_doorman_pools_transactions_percentile{percentile="50",user="app",database="mydb"} 3.8
# ... (90, 95, 99)

# HELP pg_doorman_pools_avg_wait_time Average client wait time in milliseconds
# TYPE pg_doorman_pools_avg_wait_time gauge
pg_doorman_pools_avg_wait_time{user="app",database="mydb"} 0.05
```

`avg_wait_time` — среднее, а не перцентиль (HDR для ожиданий тоже отслеживается, но сейчас экспортируется только среднее).

## Чтение метрик

### Здоровый пул

```
queries:    p50=1.2  p90=4.7   p95=8.1   p99=24.5
xacts:      p50=3.8  p90=11.2  p95=18.5  p99=42.7
wait avg:   0.05ms
```

p99 укладывается в 20× от p50 — типично для OLTP-нагрузок с редкими медленными запросами. Время ожидания — микросекунды, пул не является узким местом.

### Пул под давлением

```
queries:    p50=1.5   p90=4.9   p95=8.5   p99=25.0
xacts:      p50=215   p90=1850  p95=2400  p99=4900
wait avg:   180ms
```

С задержкой запросов всё в порядке — PostgreSQL здоров. Но транзакции медленные, а время ожидания 180 мс. Клиенты выстраиваются в очередь за бэкендами. Проверьте `SHOW POOLS` на `cl_waiting > 0` и `SHOW POOL_COORDINATOR` на вытеснения или исчерпания. Вероятное лечение: поднять `pool_size` или `max_db_connections`. См. [Pool Coordinator](../concepts/pool-coordinator.md).

### Один медленный пользователь

```
user "fast_app":   queries p99=12   xacts p99=35
user "report_job": queries p99=4500 xacts p99=8000
```

`report_job` тянет общую базу вниз. С включённым Pool Coordinator медленные транзакции `report_job` приводят к тому, что под давлением он первым отдаёт свои соединения (вытеснение смещено по p95-времени транзакций). Без Coordinator выделите `report_job` его собственный `min_guaranteed_pool_size`, чтобы он не голодил `fast_app`.

## Grafana

Пример запроса задержки запросов по перцентилю:

```promql
pg_doorman_pools_queries_percentile{database="mydb"}
```

Пример алерта: p99 запросов выше 100 мс в течение 5 минут:

```promql
pg_doorman_pools_queries_percentile{percentile="99"} > 100
```

Пример алерта на насыщение очереди:

```promql
pg_doorman_pools_avg_wait_time > 50
```

JSON дашборда лежит в директории `grafana/` проекта.

## Оговорки

- Перцентили считаются на пул, а не на запрос. pg_doorman не скажет вам, какой именно запрос медленный, — для этого используйте `pg_stat_statements` в PostgreSQL.
- HDR-гистограммы хранят значения, а не события. Один и тот же запрос, выполненный 100 тысяч раз, добавляет 100 тысяч сэмплов; частоту сэмплирования настроить нельзя.
- Экспорт всех четырёх перцентилей на серию сделан намеренно — экспортировать сырые корзины гистограммы в Prometheus было бы значительно тяжелее и редко полезно.

## Куда дальше

- [Admin Commands](admin-commands.md) — читать перцентили напрямую через `SHOW POOLS_EXTENDED`.
- [Prometheus reference](../reference/prometheus.md) — полный список метрик с метками.
- [Pool Pressure](../tutorials/pool-pressure.md) — диагностические рецепты, когда перцентили выглядят неправильно.
- [Benchmarks](../benchmarks.md) — эталонные распределения перцентилей под нагрузкой.
