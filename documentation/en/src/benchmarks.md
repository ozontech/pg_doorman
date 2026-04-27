---
title: Benchmarks
---

# Benchmarks

Three connection poolers — pg_doorman, pgbouncer, odyssey — driven
by `pgbench` against the same PostgreSQL backend on identical
hardware. Numbers below are relative throughput against each
competitor and absolute per-transaction latency.

_Last updated: 2026-04-27 07:07 UTC._

## TL;DR

- **vs pgbouncer** — pg_doorman peaks at **x10.9** TPS on prepared protocol, 120 clients.
- **vs odyssey** — pg_doorman peaks at **x1.8** TPS on extended protocol, 10,000 clients.
- **Tail latency at 10 000 simple-protocol clients** — pg_doorman **p99 44ms** (others: odyssey 286ms, pgbouncer 461ms).

### Environment

- **Provider**: Ubicloud `standard-60` (eu-central-h1)
- **Resources**: 60 vCPU / 235.9 GB
- **Kernel**: `Linux 5.15.0-139-generic x86_64`
- **Versions**: PostgreSQL 14.22, pg_doorman 3.6.1, pgbouncer 1.25.1, odyssey 1.4.1
- **Workers**: pg_doorman: 12, odyssey: 12
- **Duration per pgbench run**: 5s
- **Started**: 2026-04-27 07:02 UTC
- **Finished**: 2026-04-27 07:02 UTC
- **Total wall-clock**: 0s
- **Commit**: [`086778e5`](https://github.com/ozontech/pg_doorman/commit/086778e51c5c89e8e4e4ea6efb8906bb3a83045f)

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

**Latency** — per-transaction in ms, `p50 / p95 / p99` per cell. Lower is
better. Compare the same column across rows for one pooler, or across
columns at one row for head-to-head.

---

## Simple protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | -5% | ≈0% |
| 40 clients | x3.3 | -39% |
| 120 clients | x8.8 | ≈0% |
| 500 clients | x8.3 | ≈0% |
| 10,000 clients | x7.9 | +31% |
| 1 client + Reconnect | -24% | x1.6 |
| 40 clients + Reconnect | x1.7 | x1.7 |
| 120 clients + Reconnect | x1.5 | x1.6 |
| 500 clients + Reconnect | x1.7 | x1.6 |
| 10,000 clients + Reconnect | +45% | x2.5 |
| 1 client + SSL | -9% | ≈0% |
| 40 clients + SSL | x3.8 | ≈0% |
| 120 clients + SSL | x9.9 | ≈0% |
| 500 clients + SSL | x9.8 | ≈0% |
| 10,000 clients + SSL | - | - |
| 1 client + SSL + Reconnect | x1.9 | x1.8 |
| 40 clients + SSL + Reconnect | -5% | -20% |
| 120 clients + SSL + Reconnect | ≈0% | -31% |
| 500 clients + SSL + Reconnect | +13% | -7% |
| 10,000 clients + SSL + Reconnect | -19% | -7% |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.08 / 0.09 / 0.12 | 0.07 / 0.09 / 0.12 | 0.07 / 0.11 / 0.13 |
| 40 clients | 0.25 / 0.38 / 0.47 | 0.81 / 1.32 / 1.77 | 0.12 / 0.25 / 0.42 |
| 120 clients | 0.29 / 0.82 / 1.07 | 2.75 / 6.22 / 6.71 | 0.33 / 0.70 / 0.97 |
| 500 clients | 1.72 / 2.36 / 2.73 | 11.97 / 24.70 / 26.19 | 0.55 / 6.06 / 10.15 |
| 10,000 clients | 34.39 / 37.38 / 43.96 | 266.75 / 335.75 / 461.25 | 3.72 / 167.47 / 285.52 |
| 1 client + Reconnect | 0.17 / 0.21 / 0.25 | 0.11 / 0.15 / 0.19 | 0.16 / 0.23 / 0.27 |
| 40 clients + Reconnect | 1.28 / 2.79 / 4.00 | 2.11 / 5.23 / 6.53 | 2.07 / 4.92 / 6.43 |
| 120 clients + Reconnect | 3.84 / 8.51 / 11.38 | 5.50 / 13.94 / 17.97 | 6.24 / 14.34 / 18.60 |
| 500 clients + Reconnect | 16.93 / 36.62 / 46.22 | 26.40 / 61.90 / 74.98 | 27.06 / 59.03 / 74.45 |
| 10,000 clients + Reconnect | 357.16 / 647.26 / 783.49 | 500.35 / 1051.47 / 1234.50 | 839.81 / 1795.49 / 1992.58 |
| 1 client + SSL | 0.08 / 0.10 / 0.13 | 0.07 / 0.08 / 0.09 | 0.08 / 0.11 / 0.13 |
| 40 clients + SSL | 0.23 / 0.35 / 0.42 | 0.86 / 1.61 / 2.13 | 0.18 / 0.36 / 0.67 |
| 120 clients + SSL | 0.35 / 0.90 / 1.20 | 3.61 / 7.90 / 8.63 | 0.34 / 0.56 / 0.72 |
| 500 clients + SSL | 1.93 / 2.96 / 3.49 | 15.39 / 31.05 / 33.41 | 0.97 / 5.78 / 9.91 |
| 10,000 clients + SSL | - | - | - |
| 1 client + SSL + Reconnect | 0.16 / 0.29 / 0.33 | 0.32 / 0.37 / 0.42 | 0.29 / 0.38 / 0.43 |
| 40 clients + SSL + Reconnect | 18.39 / 37.81 / 41.91 | 15.59 / 42.33 / 57.12 | 14.30 / 31.65 / 38.41 |
| 120 clients + SSL + Reconnect | 61.02 / 121.70 / 137.25 | 59.44 / 135.81 / 161.10 | 41.65 / 92.30 / 112.88 |
| 500 clients + SSL + Reconnect | 214.90 / 433.34 / 486.40 | 226.69 / 526.96 / 633.07 | 194.41 / 402.90 / 535.25 |
| 10,000 clients + SSL + Reconnect | 2552.14 / 4745.67 / 4956.08 | 2516.67 / 4638.25 / 4906.60 | 2439.61 / 4666.80 / 4943.76 |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | -4% | x1.6 |
| 40 clients | x3.1 | -13% |
| 120 clients | x8.8 | x1.6 |
| 500 clients | x8.5 | x1.5 |
| 10,000 clients | x7.4 | x1.8 |
| 1 client + Reconnect | -18% | x1.7 |
| 40 clients + Reconnect | x1.7 | +48% |
| 120 clients + Reconnect | x1.7 | x1.7 |
| 500 clients + Reconnect | x1.8 | x2.2 |
| 10,000 clients + Reconnect | +47% | x2.6 |
| 1 client + SSL | +9% | x1.5 |
| 40 clients + SSL | x4.5 | +45% |
| 120 clients + SSL | x9.3 | +46% |
| 500 clients + SSL | x8.4 | +46% |
| 10,000 clients + SSL | - | - |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.08 / 0.09 / 0.12 | 0.07 / 0.09 / 0.11 | 0.12 / 0.17 / 0.19 |
| 40 clients | 0.24 / 0.37 / 0.46 | 0.73 / 1.11 / 1.68 | 0.19 / 0.33 / 0.49 |
| 120 clients | 0.28 / 0.89 / 1.26 | 2.84 / 6.13 / 6.54 | 0.50 / 1.43 / 2.11 |
| 500 clients | 1.94 / 2.78 / 3.23 | 13.43 / 25.58 / 27.58 | 0.75 / 10.02 / 16.94 |
| 10,000 clients | 37.05 / 40.05 / 48.59 | 270.11 / 329.47 / 458.87 | 60.78 / 242.69 / 367.83 |
| 1 client + Reconnect | 0.13 / 0.19 / 0.23 | 0.12 / 0.15 / 0.19 | 0.22 / 0.30 / 0.33 |
| 40 clients + Reconnect | 1.25 / 2.72 / 3.95 | 2.05 / 5.08 / 6.45 | 1.84 / 3.95 / 5.49 |
| 120 clients + Reconnect | 3.78 / 8.43 / 11.60 | 6.16 / 14.88 / 18.56 | 6.14 / 13.74 / 18.04 |
| 500 clients + Reconnect | 15.68 / 31.32 / 42.44 | 26.42 / 61.79 / 73.90 | 33.71 / 71.47 / 89.37 |
| 10,000 clients + Reconnect | 344.61 / 645.56 / 877.14 | 490.07 / 1017.51 / 1177.67 | 805.60 / 1742.91 / 1975.98 |
| 1 client + SSL | 0.08 / 0.09 / 0.12 | 0.09 / 0.10 / 0.11 | 0.13 / 0.15 / 0.20 |
| 40 clients + SSL | 0.23 / 0.33 / 0.40 | 1.05 / 1.50 / 1.57 | 0.28 / 0.55 / 0.98 |
| 120 clients + SSL | 0.35 / 0.92 / 1.33 | 3.46 / 7.65 / 8.11 | 0.56 / 1.13 / 1.78 |
| 500 clients + SSL | 2.20 / 3.33 / 3.91 | 15.00 / 29.81 / 33.26 | 1.64 / 9.93 / 16.35 |
| 10,000 clients + SSL | - | - | - |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | -8% |
| 40 clients | x3.9 | -38% |
| 120 clients | x10.9 | ≈0% |
| 500 clients | x10.2 | +6% |
| 10,000 clients | x8.7 | +35% |
| 1 client + Reconnect | -23% | +37% |
| 40 clients + Reconnect | x1.6 | x1.7 |
| 120 clients + Reconnect | x1.6 | +49% |
| 500 clients + Reconnect | x1.9 | N/A |
| 10,000 clients + Reconnect | +24% | x2.2 |
| 1 client + SSL | ≈0% | -5% |
| 40 clients + SSL | x4.4 | -4% |
| 120 clients + SSL | x11.8 | ≈0% |
| 500 clients + SSL | x10.1 | ≈0% |
| 10,000 clients + SSL | - | - |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.08 / 0.09 / 0.11 | 0.08 / 0.09 / 0.10 | 0.07 / 0.09 / 0.10 |
| 40 clients | 0.23 / 0.35 / 0.45 | 0.90 / 1.37 / 2.08 | 0.12 / 0.22 / 0.35 |
| 120 clients | 0.29 / 0.91 / 1.20 | 3.40 / 7.39 / 7.83 | 0.33 / 0.69 / 0.97 |
| 500 clients | 1.82 / 2.58 / 2.99 | 15.05 / 29.21 / 30.77 | 0.57 / 6.67 / 11.18 |
| 10,000 clients | 35.90 / 38.84 / 64.12 | 298.33 / 518.10 / 580.55 | 3.29 / 199.39 / 333.57 |
| 1 client + Reconnect | 0.26 / 0.33 / 0.38 | 0.19 / 0.27 / 0.32 | 0.28 / 0.38 / 0.43 |
| 40 clients + Reconnect | 1.86 / 3.80 / 5.29 | 2.74 / 6.71 / 8.42 | 3.03 / 6.57 / 8.31 |
| 120 clients + Reconnect | 5.53 / 12.19 / 16.88 | 8.23 / 20.16 / 25.05 | 8.37 / 17.45 / 23.11 |
| 500 clients + Reconnect | 21.96 / 46.93 / 61.67 | 40.38 / 88.81 / 104.99 | 37.74 / 82.80 / 103.77 |
| 10,000 clients + Reconnect | 494.09 / 1006.16 / 1228.71 | 582.47 / 1230.63 / 1457.87 | 1211.17 / 2143.45 / 2340.72 |
| 1 client + SSL | 0.08 / 0.09 / 0.11 | 0.08 / 0.09 / 0.13 | 0.08 / 0.10 / 0.13 |
| 40 clients + SSL | 0.23 / 0.35 / 0.43 | 1.03 / 1.61 / 2.56 | 0.17 / 0.36 / 0.71 |
| 120 clients + SSL | 0.36 / 0.95 / 1.30 | 4.48 / 9.28 / 9.85 | 0.35 / 0.55 / 0.73 |
| 500 clients + SSL | 2.10 / 3.21 / 3.77 | 17.50 / 34.87 / 37.96 | 1.07 / 6.13 / 10.81 |
| 10,000 clients + SSL | - | - | - |

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
