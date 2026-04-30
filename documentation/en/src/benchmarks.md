---
title: Benchmarks
---

# Benchmarks

Three connection poolers — pg_doorman, pgbouncer, odyssey — driven
by `pgbench` against the same PostgreSQL backend on identical
hardware. Numbers below are relative throughput against each
competitor and absolute per-transaction latency.

_Last updated: 2026-04-30 13:27 UTC._

## TL;DR

- **vs pgbouncer** — pg_doorman peaks at **x8.6** TPS on prepared protocol, 500 clients.
- **vs odyssey** — pg_doorman peaks at **x1.6** TPS on extended protocol, 10,000 clients.
- **Tail spread at 10 000 simple-protocol clients** (`p99/p50`, lower = more predictable) — pg_doorman 1.3× (43.8→57.6ms), pgbouncer 1.4× (273→389ms), odyssey 111× (3.12→346ms).

### Environment

- **Provider**: Ubicloud `standard-30` (eu-central-h1)
- **Resources**: 30 vCPU / 117.9 GB
- **Kernel**: `Linux 5.15.0-139-generic x86_64`
- **Versions**: PostgreSQL 14.22, pg_doorman 3.7.0, pgbouncer 1.25.1, odyssey 1.4.1
- **Workers**: pg_doorman: 15, odyssey: 15
- **Duration per pgbench run**: 30s
- **Started**: 2026-04-30 11:48 UTC
- **Finished**: 2026-04-30 13:24 UTC
- **Total wall-clock**: 1h 35m 55s
- **Commit**: [`8156f438`](https://github.com/ozontech/pg_doorman/commit/8156f4383fea89a2831a86369c179a5f7118738f)

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
| 1 client | -12% | ≈0% |
| 40 clients | x2.7 | -37% |
| 120 clients | x6.0 | -23% |
| 500 clients | x6.1 | -11% |
| 10,000 clients | x6.1 | +8% |
| 1 client + Reconnect | -10% | x2.1 |
| 40 clients + Reconnect | x2.1 | N/A |
| 120 clients + Reconnect | x1.9 | x1.8 |
| 500 clients + Reconnect | x2.0 | N/A |
| 10,000 clients + Reconnect | x1.9 | x2.7 |
| 1 client + SSL | -3% | ≈0% |
| 40 clients + SSL | x3.0 | -35% |
| 120 clients + SSL | x5.6 | -34% |
| 500 clients + SSL | x7.4 | -19% |
| 10,000 clients + SSL | x8.0 | +15% |
| 1 client + SSL + Reconnect | -6% | +41% |
| 40 clients + SSL + Reconnect | +22% | -8% |
| 120 clients + SSL + Reconnect | +17% | -14% |
| 500 clients + SSL + Reconnect | +30% | -7% |
| 10,000 clients + SSL + Reconnect | +27% | +10% |

### Latency (ms; spread = p99 / p50)

| Test | pg_doorman p50/p99 | spread | pgbouncer p50/p99 | spread | odyssey p50/p99 | spread |
|------|-------------------:|-------:|------------------:|-------:|----------------:|-------:|
| 1 client | 0.08 / 0.11 | 1.4× | 0.07 / 0.10 | 1.5× | 0.08 / 0.12 | 1.6× |
| 40 clients | 0.28 / 0.55 | 2.0× | 0.74 / 1.29 | 1.7× | 0.14 / 0.51 | 3.5× |
| 120 clients | 0.39 / 1.82 | 4.6× | 2.75 / 6.24 | 2.3× | 0.23 / 3.09 | 13× |
| 500 clients | 2.15 / 7.89 | 3.7× | 11.6 / 24.9 | 2.1× | 0.71 / 13.8 | 19× |
| 10,000 clients | 43.8 / 57.6 | 1.3× | 273 / 389 | 1.4× | 3.12 / 346 | 111× |
| 1 client + Reconnect | 0.11 / 0.19 | 1.7× | 0.11 / 0.20 | 1.9× | 0.19 / 0.31 | 1.6× |
| 40 clients + Reconnect | 1.03 / 2.60 | 2.5× | 1.96 / 6.00 | 3.1× | 1.67 / 4.15 | 2.5× |
| 120 clients + Reconnect | 3.29 / 8.00 | 2.4× | 5.52 / 17.0 | 3.1× | 5.64 / 15.3 | 2.7× |
| 500 clients + Reconnect | 13.6 / 30.4 | 2.2× | 24.8 / 67.7 | 2.7× | 24.1 / 57.6 | 2.4× |
| 10,000 clients + Reconnect | 308 / 581 | 1.9× | 586 / 1190 | 2.0× | 818 / 1639 | 2.0× |
| 1 client + SSL | 0.09 / 0.12 | 1.4× | 0.09 / 0.12 | 1.4× | 0.09 / 0.14 | 1.6× |
| 40 clients + SSL | 0.29 / 0.71 | 2.4× | 0.86 / 2.14 | 2.5× | 0.15 / 0.75 | 4.9× |
| 120 clients + SSL | 0.58 / 2.21 | 3.8× | 3.57 / 8.07 | 2.3× | 0.29 / 3.96 | 14× |
| 500 clients + SSL | 1.74 / 11.6 | 6.7× | 17.1 / 32.7 | 1.9× | 1.03 / 10.4 | 10× |
| 10,000 clients + SSL | 41.9 / 68.7 | 1.6× | 387 / 571 | 1.5× | 14.3 / 339 | 24× |
| 1 client + SSL + Reconnect | 0.20 / 0.38 | 1.9× | 0.18 / 0.35 | 2.0× | 0.27 / 0.41 | 1.5× |
| 40 clients + SSL + Reconnect | 15.0 / 37.0 | 2.5× | 17.1 / 54.3 | 3.2× | 13.9 / 34.2 | 2.5× |
| 120 clients + SSL + Reconnect | 50.2 / 119 | 2.4× | 55.8 / 155 | 2.8× | 43.5 / 104 | 2.4× |
| 500 clients + SSL + Reconnect | 203 / 451 | 2.2× | 256 / 615 | 2.4× | 188 / 422 | 2.2× |
| 10,000 clients + SSL + Reconnect | 3685 / 8209 | 2.2× | 4865 / 10452 | 2.1× | 4121 / 8875 | 2.2× |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | +19% | x1.8 |
| 40 clients | x2.8 | ≈0% |
| 120 clients | x5.9 | +10% |
| 500 clients | x6.4 | +33% |
| 10,000 clients | x6.6 | x1.6 |
| 1 client + Reconnect | ≈0% | x2.2 |
| 40 clients + Reconnect | x1.8 | N/A |
| 120 clients + Reconnect | x2.0 | N/A |
| 500 clients + Reconnect | x1.9 | N/A |
| 10,000 clients + Reconnect | x1.9 | x2.3 |
| 1 client + SSL | ≈0% | x1.7 |
| 40 clients + SSL | x2.8 | -15% |
| 120 clients + SSL | x5.9 | ≈0% |
| 500 clients + SSL | x7.1 | +23% |
| 10,000 clients + SSL | x7.5 | x1.6 |

### Latency (ms; spread = p99 / p50)

| Test | pg_doorman p50/p99 | spread | pgbouncer p50/p99 | spread | odyssey p50/p99 | spread |
|------|-------------------:|-------:|------------------:|-------:|----------------:|-------:|
| 1 client | 0.06 / 0.09 | 1.5× | 0.08 / 0.10 | 1.4× | 0.11 / 0.18 | 1.6× |
| 40 clients | 0.27 / 0.57 | 2.2× | 0.72 / 1.71 | 2.4× | 0.23 / 0.74 | 3.2× |
| 120 clients | 0.45 / 1.82 | 4.1× | 2.88 / 6.82 | 2.4× | 0.40 / 3.28 | 8.3× |
| 500 clients | 1.82 / 10.2 | 5.6× | 12.6 / 26.2 | 2.1× | 0.88 / 20.7 | 24× |
| 10,000 clients | 41.0 / 52.9 | 1.3× | 282 / 370 | 1.3× | 5.04 / 473 | 94× |
| 1 client + Reconnect | 0.12 / 0.19 | 1.6× | 0.12 / 0.23 | 2.0× | 0.26 / 0.44 | 1.7× |
| 40 clients + Reconnect | 1.07 / 2.72 | 2.5× | 1.77 / 5.86 | 3.3× | 1.71 / 4.85 | 2.8× |
| 120 clients + Reconnect | 3.22 / 7.83 | 2.4× | 5.78 / 17.5 | 3.0× | 5.40 / 13.6 | 2.5× |
| 500 clients + Reconnect | 14.0 / 31.3 | 2.2× | 24.6 / 66.6 | 2.7× | 24.5 / 57.5 | 2.4× |
| 10,000 clients + Reconnect | 300 / 571 | 1.9× | 555 / 1196 | 2.2× | 664 / 1513 | 2.3× |
| 1 client + SSL | 0.08 / 0.11 | 1.3× | 0.08 / 0.12 | 1.5× | 0.13 / 0.23 | 1.7× |
| 40 clients + SSL | 0.30 / 0.73 | 2.5× | 0.85 / 2.05 | 2.4× | 0.23 / 0.88 | 3.9× |
| 120 clients + SSL | 0.56 / 2.31 | 4.1× | 3.67 / 8.24 | 2.2× | 0.51 / 4.55 | 8.9× |
| 500 clients + SSL | 1.73 / 12.0 | 6.9× | 16.7 / 33.6 | 2.0× | 2.06 / 19.9 | 9.7× |
| 10,000 clients + SSL | 46.9 / 81.6 | 1.7× | 386 / 607 | 1.6× | 77.2 / 542 | 7.0× |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | ≈0% |
| 40 clients | x3.1 | -42% |
| 120 clients | x7.5 | -36% |
| 500 clients | x8.6 | -18% |
| 10,000 clients | x8.6 | +5% |
| 1 client + Reconnect | -6% | x2.1 |
| 40 clients + Reconnect | x1.8 | x1.6 |
| 120 clients + Reconnect | x1.9 | x1.6 |
| 500 clients + Reconnect | x1.8 | N/A |
| 10,000 clients + Reconnect | x1.8 | x2.4 |
| 1 client + SSL | +8% | ≈0% |
| 40 clients + SSL | x3.4 | -39% |
| 120 clients + SSL | x7.2 | -33% |
| 500 clients + SSL | x9.0 | -19% |
| 10,000 clients + SSL | x8.8 | +10% |

### Latency (ms; spread = p99 / p50)

| Test | pg_doorman p50/p99 | spread | pgbouncer p50/p99 | spread | odyssey p50/p99 | spread |
|------|-------------------:|-------:|------------------:|-------:|----------------:|-------:|
| 1 client | 0.08 / 0.10 | 1.3× | 0.08 / 0.10 | 1.4× | 0.07 / 0.13 | 1.7× |
| 40 clients | 0.28 / 0.57 | 2.0× | 0.86 / 1.98 | 2.3× | 0.13 / 0.47 | 3.6× |
| 120 clients | 0.46 / 1.80 | 4.0× | 3.69 / 8.26 | 2.2× | 0.23 / 3.00 | 13× |
| 500 clients | 1.79 / 11.1 | 6.2× | 16.9 / 31.9 | 1.9× | 0.69 / 13.6 | 20× |
| 10,000 clients | 42.7 / 56.5 | 1.3× | 375 / 530 | 1.4× | 2.98 / 322 | 108× |
| 1 client + Reconnect | 0.20 / 0.33 | 1.6× | 0.20 / 0.35 | 1.8× | 0.40 / 0.61 | 1.5× |
| 40 clients + Reconnect | 1.71 / 4.26 | 2.5× | 2.76 / 8.43 | 3.0× | 2.68 / 7.10 | 2.6× |
| 120 clients + Reconnect | 4.91 / 11.9 | 2.4× | 8.57 / 24.3 | 2.8× | 7.91 / 20.8 | 2.6× |
| 500 clients + Reconnect | 20.4 / 44.3 | 2.2× | 34.3 / 91.9 | 2.7× | 40.7 / 95.0 | 2.3× |
| 10,000 clients + Reconnect | 409 / 801 | 2.0× | 718 / 1563 | 2.2× | 930 / 2009 | 2.2× |
| 1 client + SSL | 0.08 / 0.10 | 1.3× | 0.09 / 0.12 | 1.4× | 0.08 / 0.14 | 1.8× |
| 40 clients + SSL | 0.29 / 0.70 | 2.4× | 1.00 / 2.31 | 2.3× | 0.14 / 0.65 | 4.6× |
| 120 clients + SSL | 0.55 / 2.23 | 4.0× | 4.32 / 9.69 | 2.2× | 0.29 / 3.83 | 13× |
| 500 clients + SSL | 1.64 / 10.9 | 6.6× | 21.2 / 39.4 | 1.9× | 1.05 / 10.2 | 9.7× |
| 10,000 clients + SSL | 44.4 / 69.5 | 1.6× | 442 / 734 | 1.7× | 11.6 / 351 | 30× |

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
