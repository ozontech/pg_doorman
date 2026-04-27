---
title: Benchmarks
---

# Benchmarks

Three connection poolers — pg_doorman, pgbouncer, odyssey — driven
by `pgbench` against the same PostgreSQL backend on identical
hardware. Numbers below are relative throughput against each
competitor and absolute per-transaction latency.

_Last updated: 2026-04-27 12:00 UTC._

## TL;DR

- **vs pgbouncer** — pg_doorman peaks at **x12.0** TPS on prepared protocol, 120 clients.
- **vs odyssey** — pg_doorman wins by **+40%** at most (extended protocol, 120 clients).
- **Tail spread at 10 000 simple-protocol clients** (`p99/p50`, lower = more predictable) — pg_doorman 1.1× (59.9→64.5ms), pgbouncer 1.4× (276→387ms), odyssey 11× (17.9→204ms).

### Environment

- **Provider**: Ubicloud `standard-60` (eu-central-h1)
- **Resources**: 60 vCPU / 235.9 GB
- **Kernel**: `Linux 5.15.0-139-generic x86_64`
- **Versions**: PostgreSQL 14.22, pg_doorman 3.6.1, pgbouncer 1.25.1, odyssey 1.4.1
- **Workers**: pg_doorman: 30, odyssey: 30
- **Duration per pgbench run**: 60s
- **Started**: 2026-04-27 08:06 UTC
- **Finished**: 2026-04-27 11:03 UTC
- **Total wall-clock**: 2h 57m 08s
- **Commit**: [`c9dd765c`](https://github.com/ozontech/pg_doorman/commit/c9dd765c60605e453d203f93e2d5cfadf2734716)

### Methodology

Each scenario runs `pgbench -T <duration>` against a 40-connection
server-side pool (`pool_mode = transaction`). The workload is a single
`SELECT :aid` (`\set aid random(1, 100000)`) — pure pooler overhead, no
real working set. Three poolers, one PostgreSQL backend, identical
hardware.

- **Reconnect** rows use `pgbench --connect`: a fresh TCP+startup per
  transaction (worst case for login latency).
- **SSL** rows set `PGSSLMODE=require` and a self-signed cert.
- Latency is collected with `pgbench --log` (per-transaction file);
  percentiles come from those samples, not from `pgbench` summary stats.
- Scenarios run sequentially with the same data dir and warm OS caches.

Source: [`tests/bdd/features/bench.feature`](https://github.com/ozontech/pg_doorman/blob/master/tests/bdd/features/bench.feature),
driver: [`benches/setup-and-run-bench.sh`](https://github.com/ozontech/pg_doorman/blob/master/benches/setup-and-run-bench.sh).

### Reading the tables

**Throughput** — `pg_doorman_TPS / competitor_TPS`, rendered:

| Value | Meaning |
|-------|---------|
| +N% / -N% | Faster / slower by N percent |
| ≈0% | Within 3% — call it a tie |
| xN.N | N times faster (when ratio ≥ 1.5) |
| ∞ | Competitor returned 0 TPS |
| N/A | Competitor was not measured for this row |
| - | Not measured for either pooler |

**Latency** — per-transaction in ms. Each row shows `p50 / p99` for
every pooler plus the **spread** (`p99 / p50`): how far the slowest 1%
drifts from the median. `1.0×` means the tail equals the median;
`100×` means the worst 1% takes two orders of magnitude longer than a
typical request — the regime where fanout latency starts hitting users
([Dean & Barroso, 2013](https://www.barroso.org/publications/TheTailAtScale.pdf)).
Watch the spread column to see whether tail latency stays bounded as
the client count grows. Full p95 series ships in the raw
`pgbench --log` files in the artifact tarball.

---

## Simple protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | ≈0% |
| 40 clients | x2.9 | -46% |
| 120 clients | x9.9 | ≈0% |
| 500 clients | x6.6 | -32% |
| 10,000 clients | x4.7 | -34% |
| 1 client + Reconnect | -14% | x2.0 |
| 40 clients + Reconnect | x1.6 | N/A |
| 120 clients + Reconnect | x1.7 | N/A |
| 500 clients + Reconnect | x1.7 | N/A |
| 10,000 clients + Reconnect | +41% | N/A |
| 1 client + SSL | ≈0% | ≈0% |
| 40 clients + SSL | x3.1 | -38% |
| 120 clients + SSL | x8.5 | -5% |
| 500 clients + SSL | x10.6 | +18% |
| 10,000 clients + SSL | x7.1 | +12% |
| 1 client + SSL + Reconnect | -6% | x1.6 |
| 40 clients + SSL + Reconnect | ≈0% | -35% |
| 120 clients + SSL + Reconnect | +5% | -39% |
| 500 clients + SSL + Reconnect | +17% | -28% |
| 10,000 clients + SSL + Reconnect | -8% | -16% |

### Latency (ms; spread = p99 / p50)

| Test | pg_doorman p50/p99 | spread | pgbouncer p50/p99 | spread | odyssey p50/p99 | spread |
|------|-------------------:|-------:|------------------:|-------:|----------------:|-------:|
| 1 client | 0.08 / 0.10 | 1.4× | 0.07 / 0.10 | 1.4× | 0.07 / 0.12 | 1.7× |
| 40 clients | 0.27 / 0.50 | 1.8× | 0.74 / 1.90 | 2.6× | 0.12 / 0.30 | 2.5× |
| 120 clients | 0.29 / 0.91 | 3.2× | 2.86 / 6.77 | 2.4× | 0.24 / 2.07 | 8.8× |
| 500 clients | 2.30 / 4.38 | 1.9× | 12.6 / 27.6 | 2.2× | 0.82 / 7.87 | 9.5× |
| 10,000 clients | 59.9 / 64.5 | 1.1× | 276 / 387 | 1.4× | 17.9 / 204 | 11× |
| 1 client + Reconnect | 0.14 / 0.23 | 1.6× | 0.11 / 0.21 | 1.9× | 0.18 / 0.31 | 1.7× |
| 40 clients + Reconnect | 1.26 / 4.10 | 3.2× | 1.91 / 6.26 | 3.3× | 1.85 / 5.35 | 2.9× |
| 120 clients + Reconnect | 3.83 / 11.1 | 2.9× | 5.89 / 18.1 | 3.1× | 5.95 / 16.7 | 2.8× |
| 500 clients + Reconnect | 16.3 / 42.9 | 2.6× | 26.2 / 71.1 | 2.7× | 25.3 / 65.4 | 2.6× |
| 10,000 clients + Reconnect | 369 / 763 | 2.1× | 524 / 1106 | 2.1× | 744 / 1519 | 2.0× |
| 1 client + SSL | 0.08 / 0.11 | 1.4× | 0.08 / 0.11 | 1.4× | 0.08 / 0.12 | 1.6× |
| 40 clients + SSL | 0.27 / 0.50 | 1.8× | 0.87 / 2.16 | 2.5× | 0.15 / 0.28 | 1.9× |
| 120 clients + SSL | 0.42 / 1.16 | 2.7× | 3.71 / 8.61 | 2.3× | 0.30 / 2.07 | 7.0× |
| 500 clients + SSL | 1.09 / 2.54 | 2.3× | 17.0 / 34.7 | 2.0× | 1.04 / 5.42 | 5.2× |
| 10,000 clients + SSL | 26.9 / 64.0 | 2.4× | 369 / 511 | 1.4× | 29.7 / 82.4 | 2.8× |
| 1 client + SSL + Reconnect | 0.23 / 0.36 | 1.6× | 0.19 / 0.35 | 1.9× | 0.28 / 0.43 | 1.5× |
| 40 clients + SSL + Reconnect | 17.6 / 42.2 | 2.4× | 16.3 / 53.1 | 3.3× | 11.4 / 31.7 | 2.8× |
| 120 clients + SSL + Reconnect | 57.0 / 130 | 2.3× | 55.6 / 166 | 3.0× | 33.9 / 95.7 | 2.8× |
| 500 clients + SSL + Reconnect | 212 / 483 | 2.3× | 237 / 619 | 2.6× | 150 / 392 | 2.6× |
| 10,000 clients + SSL + Reconnect | 5033 / 10055 | 2.0× | 4052 / 10245 | 2.5× | 4361 / 8612 | 2.0× |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | x1.6 |
| 40 clients | x3.0 | -21% |
| 120 clients | x10.4 | +40% |
| 500 clients | x7.4 | +7% |
| 10,000 clients | x5.2 | ≈0% |
| 1 client + Reconnect | -16% | x1.9 |
| 40 clients + Reconnect | x1.7 | N/A |
| 120 clients + Reconnect | x1.6 | x1.5 |
| 500 clients + Reconnect | x1.7 | N/A |
| 10,000 clients + Reconnect | +41% | x2.2 |
| 1 client + SSL | ≈0% | +49% |
| 40 clients + SSL | x3.3 | -13% |
| 120 clients + SSL | x9.4 | +50% |
| 500 clients + SSL | x10.4 | x1.7 |
| 10,000 clients + SSL | x7.1 | +25% |

### Latency (ms; spread = p99 / p50)

| Test | pg_doorman p50/p99 | spread | pgbouncer p50/p99 | spread | odyssey p50/p99 | spread |
|------|-------------------:|-------:|------------------:|-------:|----------------:|-------:|
| 1 client | 0.07 / 0.11 | 1.4× | 0.07 / 0.10 | 1.4× | 0.11 / 0.18 | 1.6× |
| 40 clients | 0.27 / 0.48 | 1.8× | 0.76 / 1.90 | 2.5× | 0.18 / 0.48 | 2.7× |
| 120 clients | 0.28 / 0.89 | 3.2× | 3.06 / 6.87 | 2.2× | 0.35 / 3.52 | 10× |
| 500 clients | 2.08 / 3.98 | 1.9× | 12.8 / 27.7 | 2.2× | 1.55 / 13.2 | 8.5× |
| 10,000 clients | 55.4 / 60.3 | 1.1× | 288 / 387 | 1.3× | 52.2 / 326 | 6.2× |
| 1 client + Reconnect | 0.14 / 0.22 | 1.6× | 0.12 / 0.22 | 1.8× | 0.24 / 0.41 | 1.7× |
| 40 clients + Reconnect | 1.25 / 4.02 | 3.2× | 1.96 / 6.44 | 3.3× | 1.85 / 5.48 | 3.0× |
| 120 clients + Reconnect | 3.81 / 11.1 | 2.9× | 5.62 / 17.8 | 3.2× | 5.74 / 15.7 | 2.7× |
| 500 clients + Reconnect | 16.5 / 44.6 | 2.7× | 26.5 / 72.3 | 2.7× | 27.1 / 72.2 | 2.7× |
| 10,000 clients + Reconnect | 368 / 764 | 2.1× | 511 / 1139 | 2.2× | 816 / 1657 | 2.0× |
| 1 client + SSL | 0.09 / 0.11 | 1.2× | 0.08 / 0.12 | 1.5× | 0.12 / 0.20 | 1.7× |
| 40 clients + SSL | 0.27 / 0.49 | 1.8× | 0.91 / 2.20 | 2.4× | 0.23 / 0.37 | 1.6× |
| 120 clients + SSL | 0.38 / 1.05 | 2.8× | 3.68 / 8.69 | 2.4× | 0.57 / 2.62 | 4.6× |
| 500 clients + SSL | 1.01 / 2.65 | 2.6× | 17.6 / 36.1 | 2.1× | 2.29 / 7.34 | 3.2× |
| 10,000 clients + SSL | 27.7 / 65.9 | 2.4× | 384 / 521 | 1.4× | 49.2 / 152 | 3.1× |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | ≈0% |
| 40 clients | x3.4 | -49% |
| 120 clients | x12.0 | ≈0% |
| 500 clients | x8.5 | -29% |
| 10,000 clients | x6.2 | -32% |
| 1 client + Reconnect | -9% | x2.1 |
| 40 clients + Reconnect | x1.6 | +48% |
| 120 clients + Reconnect | x1.7 | N/A |
| 500 clients + Reconnect | x1.8 | N/A |
| 10,000 clients + Reconnect | +40% | x2.1 |
| 1 client + SSL | +3% | ≈0% |
| 40 clients + SSL | x3.8 | -36% |
| 120 clients + SSL | x10.0 | ≈0% |
| 500 clients + SSL | x12.7 | +19% |
| 10,000 clients + SSL | x8.6 | +12% |

### Latency (ms; spread = p99 / p50)

| Test | pg_doorman p50/p99 | spread | pgbouncer p50/p99 | spread | odyssey p50/p99 | spread |
|------|-------------------:|-------:|------------------:|-------:|----------------:|-------:|
| 1 client | 0.07 / 0.10 | 1.4× | 0.07 / 0.10 | 1.4× | 0.07 / 0.11 | 1.6× |
| 40 clients | 0.27 / 0.49 | 1.8× | 0.90 / 2.23 | 2.5× | 0.12 / 0.26 | 2.2× |
| 120 clients | 0.30 / 0.94 | 3.2× | 3.82 / 8.20 | 2.1× | 0.23 / 1.48 | 6.3× |
| 500 clients | 2.21 / 4.23 | 1.9× | 16.5 / 33.0 | 2.0× | 0.81 / 6.93 | 8.6× |
| 10,000 clients | 57.5 / 62.9 | 1.1× | 353 / 465 | 1.3× | 17.5 / 205 | 12× |
| 1 client + Reconnect | 0.20 / 0.33 | 1.6× | 0.20 / 0.33 | 1.7× | 0.37 / 0.61 | 1.6× |
| 40 clients + Reconnect | 1.84 / 5.20 | 2.8× | 2.67 / 8.54 | 3.2× | 2.73 / 7.35 | 2.7× |
| 120 clients + Reconnect | 5.25 / 14.6 | 2.8× | 8.11 / 25.0 | 3.1× | 7.72 / 21.6 | 2.8× |
| 500 clients + Reconnect | 21.6 / 58.4 | 2.7× | 38.0 / 101 | 2.7× | 33.3 / 82.8 | 2.5× |
| 10,000 clients + Reconnect | 485 / 1031 | 2.1× | 672 / 1421 | 2.1× | 1026 / 2214 | 2.2× |
| 1 client + SSL | 0.08 / 0.12 | 1.4× | 0.08 / 0.12 | 1.5× | 0.08 / 0.13 | 1.7× |
| 40 clients + SSL | 0.26 / 0.48 | 1.8× | 1.02 / 2.59 | 2.5× | 0.15 / 0.27 | 1.8× |
| 120 clients + SSL | 0.42 / 1.16 | 2.8× | 4.44 / 9.74 | 2.2× | 0.28 / 1.15 | 4.0× |
| 500 clients + SSL | 1.08 / 2.60 | 2.4× | 21.5 / 41.0 | 1.9× | 1.06 / 5.35 | 5.0× |
| 10,000 clients + SSL | 27.1 / 58.3 | 2.1× | 463 / 609 | 1.3× | 29.8 / 86.2 | 2.9× |

---

### Caveats

- 30 s per run is short by `pgbench` standards (the docs recommend
  minutes); expect ±5% variance between runs. Re-run for production
  decisions.
- Single PostgreSQL backend, no replicas, no real working set — these
  numbers measure pooler overhead, not full-system throughput.
- All three poolers use vendor defaults plus `pool_size = 40`.
  Tuning specific knobs (`pgbouncer so_reuseport`, `odyssey workers`)
  will move the curves.
- `Reconnect` is the worst-case login-latency scenario; the headline
  numbers in production rarely look like the Reconnect rows.
- Workload is a 1-row `SELECT`. Read-heavy OLTP, OLAP, or `LISTEN`/
  `NOTIFY` paths are not represented.
