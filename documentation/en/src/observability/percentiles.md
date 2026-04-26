# Latency Percentiles

PgDoorman tracks query and transaction latency per pool using HDR Histograms. Four percentiles are exposed to Prometheus: p50, p90, p95, p99.

This page explains where the numbers come from and how to read them.

## What is measured

Three latency series per user×database:

| Series | What it covers |
| --- | --- |
| `query_histogram` | Time from query start to query completion on a backend. Measures PostgreSQL execution time as observed by PgDoorman. |
| `xact_histogram` | Time from `BEGIN` (or first statement of an implicit transaction) to `COMMIT` / `ROLLBACK`. |
| `wait_histogram` | Time a client spent waiting for a backend connection to become available. |

`wait_histogram` is the pool's own contribution to latency. If `wait_histogram` p99 is high but `query_histogram` p99 is low, the bottleneck is connection acquisition, not PostgreSQL.

## Histogram details

PgDoorman uses [HDR Histogram](https://github.com/HdrHistogram/HdrHistogram_rust) with:

- Maximum value: 10 minutes (600 seconds).
- Significant figures: 2 (about 0.1% relative error).

Memory cost: about 10 KB per histogram. Three histograms per user×database means ~30 KB per pool — comfortable for hundreds of pools.

The default reporting horizon is the lifetime of the process. Histograms reset on `SIGHUP` (config reload) and on explicit `RECONNECT`.

Odyssey uses TDigest, PgBouncer does not expose percentiles. HDR is preferred when you know the upper bound (10 minutes is generous for a connection pool); TDigest handles unbounded streams.

## Prometheus exposure

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

`avg_wait_time` is the mean rather than a percentile (HDR for waits is also tracked but only the mean is currently exported).

## Reading the numbers

### Healthy pool

```
queries:    p50=1.2  p90=4.7   p95=8.1   p99=24.5
xacts:      p50=3.8  p90=11.2  p95=18.5  p99=42.7
wait avg:   0.05ms
```

p99 is within 20× of p50 — typical for OLTP workloads with rare slow queries. Wait time is microseconds — pool is not the bottleneck.

### Pool under pressure

```
queries:    p50=1.5   p90=4.9   p95=8.5   p99=25.0
xacts:      p50=215   p90=1850  p95=2400  p99=4900
wait avg:   180ms
```

Query latency is fine — PostgreSQL is healthy. But transactions are slow and wait time is 180ms. Clients are queuing for backends. Check `SHOW POOLS` for `cl_waiting > 0` and `SHOW POOL_COORDINATOR` for evictions or exhaustions. Likely fix: raise `pool_size` or `max_db_connections`. See [Pool Coordinator](../concepts/pool-coordinator.md).

### One slow user

```
user "fast_app":   queries p99=12   xacts p99=35
user "report_job": queries p99=4500 xacts p99=8000
```

`report_job` is dragging down the shared database. With Pool Coordinator on, `report_job`'s slow transactions cause it to donate connections first under pressure (eviction is biased by p95 transaction time). Without Coordinator, isolate `report_job` to its own `min_guaranteed_pool_size` so it cannot starve `fast_app`.

## Grafana

Sample query for query latency by percentile:

```promql
pg_doorman_pools_queries_percentile{database="mydb"}
```

Sample alert: query p99 above 100ms for 5 minutes:

```promql
pg_doorman_pools_queries_percentile{percentile="99"} > 100
```

Sample queue saturation alert:

```promql
pg_doorman_pools_avg_wait_time > 50
```

A dashboard JSON is available in the project's `grafana/` directory.

## Caveats

- Percentiles are per pool, not per query. PgDoorman cannot tell you which query is slow — use `pg_stat_statements` on PostgreSQL for that.
- HDR histograms hold values, not events. The same query running 100k times contributes to 100k samples; sampling rate is not adjustable.
- Exporting all four percentiles per series is intentional — exporting raw histogram buckets to Prometheus would be much heavier and rarely useful.

## Where to next

- [Admin Commands](admin-commands.md) — read percentiles directly via `SHOW POOLS_EXTENDED`.
- [Prometheus reference](../reference/prometheus.md) — full metric list with labels.
- [Pool Pressure](../tutorials/pool-pressure.md) — diagnostic recipes when percentiles look wrong.
- [Benchmarks](../benchmarks.md) — reference percentile distributions under load.
