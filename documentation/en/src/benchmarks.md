---
title: Benchmarks
---

# Benchmarks

Three connection poolers — pg_doorman, pgbouncer, odyssey — driven
by `pgbench` against the same PostgreSQL backend on identical
hardware. Numbers below are relative throughput against each
competitor and absolute per-transaction latency.

_Last updated: 2026-04-27 07:37 UTC._

## TL;DR

- **vs pgbouncer** — pg_doorman peaks at **x11.4** TPS on prepared protocol, 120 clients.
- **vs odyssey** — pg_doorman wins by **+42%** at most (extended protocol, 120 clients).
- **Tail latency at 10 000 simple-protocol clients** — pg_doorman **p99 66ms** (others: odyssey 71ms, pgbouncer 456ms).

### Environment

- **Provider**: Ubicloud `standard-60` (eu-central-h1)
- **Resources**: 60 vCPU / 235.9 GB
- **Kernel**: `Linux 5.15.0-139-generic x86_64`
- **Versions**: PostgreSQL 14.22, pg_doorman 3.6.1, pgbouncer 1.25.1, odyssey 1.4.1
- **Workers**: pg_doorman: 30, odyssey: 30
- **Duration per pgbench run**: 5s
- **Started**: 2026-04-27 07:14 UTC
- **Finished**: 2026-04-27 07:33 UTC
- **Total wall-clock**: 19m 22s
- **Commit**: [`c73c0147`](https://github.com/ozontech/pg_doorman/commit/c73c0147626793173966f1e1ddd88c2a8bd09915)

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

**Latency** — per-transaction p99 in ms, one number per pooler. Lower is
better. Full p50/p95/p99 series live in the raw `pgbench --log` files
shipped alongside this report.

---

## Simple protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | -3% | -9% |
| 40 clients | x2.9 | -43% |
| 120 clients | x8.9 | ≈0% |
| 500 clients | x6.3 | -34% |
| 10,000 clients | x4.4 | -30% |
| 1 client + Reconnect | -21% | x2.0 |
| 40 clients + Reconnect | +27% | x1.6 |
| 120 clients + Reconnect | x1.6 | +47% |
| 500 clients + Reconnect | x1.8 | x1.6 |
| 10,000 clients + Reconnect | +34% | x2.0 |
| 1 client + SSL | -6% | +5% |
| 40 clients + SSL | x4.0 | -26% |
| 120 clients + SSL | x8.6 | +6% |
| 500 clients + SSL | x10.1 | +22% |
| 10,000 clients + SSL | - | - |
| 1 client + SSL + Reconnect | -20% | +8% |
| 40 clients + SSL + Reconnect | -18% | -35% |
| 120 clients + SSL + Reconnect | -16% | -41% |
| 500 clients + SSL + Reconnect | ≈0% | -30% |
| 10,000 clients + SSL + Reconnect | -13% | -3% |

### p99 latency (ms, lower is better)

| Test | pg_doorman | pgbouncer | odyssey |
|------|---:|---:|---:|
| 1 client | 0.12 | 0.10 | 0.11 |
| 40 clients | 0.49 | 1.72 | 0.31 |
| 120 clients | 0.94 | 6.36 | 2.07 |
| 500 clients | 4.45 | 26.7 | 7.51 |
| 10,000 clients | 65.5 | 456 | 71.2 |
| 1 client + Reconnect | 0.21 | 0.18 | 0.30 |
| 40 clients + Reconnect | 4.14 | 5.43 | 6.21 |
| 120 clients + Reconnect | 11.4 | 17.9 | 15.4 |
| 500 clients + Reconnect | 40.8 | 71.1 | 66.1 |
| 10,000 clients + Reconnect | 921 | 1095 | 1602 |
| 1 client + SSL | 0.12 | 0.09 | 0.14 |
| 40 clients + SSL | 0.45 | 2.40 | 0.32 |
| 120 clients + SSL | 1.11 | 8.08 | 2.20 |
| 500 clients + SSL | 2.52 | 33.9 | 5.99 |
| 10,000 clients + SSL | - | - | - |
| 1 client + SSL + Reconnect | 0.40 | 0.37 | 0.42 |
| 40 clients + SSL + Reconnect | 43.0 | 48.6 | 33.5 |
| 120 clients + SSL + Reconnect | 130 | 138 | 102 |
| 500 clients + SSL + Reconnect | 540 | 593 | 475 |
| 10,000 clients + SSL + Reconnect | 4959 | 4891 | 4964 |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | x1.6 |
| 40 clients | x2.6 | -18% |
| 120 clients | x9.4 | +42% |
| 500 clients | x7.1 | -8% |
| 10,000 clients | x4.8 | -5% |
| 1 client + Reconnect | -26% | +47% |
| 40 clients + Reconnect | x1.6 | x1.5 |
| 120 clients + Reconnect | x1.7 | x1.5 |
| 500 clients + Reconnect | x1.7 | N/A |
| 10,000 clients + Reconnect | +38% | x2.1 |
| 1 client + SSL | -5% | x1.5 |
| 40 clients + SSL | x3.3 | +4% |
| 120 clients + SSL | x9.1 | x1.6 |
| 500 clients + SSL | x10.1 | x1.7 |
| 10,000 clients + SSL | - | - |

### p99 latency (ms, lower is better)

| Test | pg_doorman | pgbouncer | odyssey |
|------|---:|---:|---:|
| 1 client | 0.11 | 0.12 | 0.18 |
| 40 clients | 0.50 | 1.08 | 0.49 |
| 120 clients | 0.91 | 6.79 | 3.46 |
| 500 clients | 4.14 | 27.0 | 8.62 |
| 10,000 clients | 63.5 | 394 | 279 |
| 1 client + Reconnect | 0.20 | 0.20 | 0.34 |
| 40 clients + Reconnect | 3.85 | 6.19 | 5.63 |
| 120 clients + Reconnect | 11.4 | 19.0 | 15.6 |
| 500 clients + Reconnect | 43.4 | 69.7 | 79.0 |
| 10,000 clients + Reconnect | 857 | 1224 | 1632 |
| 1 client + SSL | 0.11 | 0.12 | 0.20 |
| 40 clients + SSL | 0.46 | 1.32 | 0.50 |
| 120 clients + SSL | 1.09 | 8.70 | 3.38 |
| 500 clients + SSL | 2.58 | 33.0 | 7.46 |
| 10,000 clients + SSL | - | - | - |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | -10% |
| 40 clients | x4.1 | -40% |
| 120 clients | x11.4 | ≈0% |
| 500 clients | x8.0 | -28% |
| 10,000 clients | x5.9 | -26% |
| 1 client + Reconnect | -21% | x1.6 |
| 40 clients + Reconnect | x1.7 | x1.6 |
| 120 clients + Reconnect | x1.6 | x1.5 |
| 500 clients + Reconnect | x1.7 | +49% |
| 10,000 clients + Reconnect | +33% | x2.3 |
| 1 client + SSL | ≈0% | -6% |
| 40 clients + SSL | x4.3 | -28% |
| 120 clients + SSL | x10.6 | +8% |
| 500 clients + SSL | x12.4 | +17% |
| 10,000 clients + SSL | - | - |

### p99 latency (ms, lower is better)

| Test | pg_doorman | pgbouncer | odyssey |
|------|---:|---:|---:|
| 1 client | 0.11 | 0.10 | 0.11 |
| 40 clients | 0.44 | 2.35 | 0.28 |
| 120 clients | 0.96 | 7.87 | 1.26 |
| 500 clients | 4.22 | 32.2 | 7.88 |
| 10,000 clients | 125 | 3999 | 210 |
| 1 client + Reconnect | 0.39 | 0.36 | 0.54 |
| 40 clients + Reconnect | 4.48 | 8.33 | 7.20 |
| 120 clients + Reconnect | 15.7 | 24.3 | 20.5 |
| 500 clients + Reconnect | 57.3 | 96.1 | 80.8 |
| 10,000 clients + Reconnect | 1106 | 1352 | 2215 |
| 1 client + SSL | 0.13 | 0.12 | 0.13 |
| 40 clients + SSL | 0.46 | 2.65 | 0.28 |
| 120 clients + SSL | 1.12 | 9.76 | 2.50 |
| 500 clients + SSL | 2.57 | 37.7 | 6.37 |
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
