---
title: Benchmarks
---

# Benchmarks

pg_doorman vs pgbouncer vs odyssey. Each test runs `pgbench` for 10 seconds through a 40-connection pool.

Last updated: 2026-04-26 14:33 UTC

### Environment

- **Host**: vm9f8ms9
- **Resources**: 8 vCPU / 31.3 GB
- **Workers**: pg_doorman: 4, odyssey: 4
- **Duration per pgbench run**: 10s
- **Started**: 2026-04-26T14:32:04Z
- **Commit**: `unknown`

### Reading the tables

**Throughput** — pg_doorman TPS relative to each competitor:

| Value | Meaning |
|-------|---------|
| +N% | Faster by N% |
| -N% | Slower by N% |
| ≈0% | Within 3% |
| xN | N times faster or slower |
| ∞ | Competitor got 0 TPS |
| N/A | Unsupported |
| - | Not tested |

**Latency** — per-transaction latency in ms. Each cell: `p50 / p95 / p99`. Lower is better.

---

## Simple protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | - | - |
| 500 clients + Reconnect | - | - |
| 120 clients + SSL | N/A | N/A |
| 500 clients + SSL | N/A | - |
| 10,000 clients + SSL | - | - |
| 1 client + SSL + Reconnect | N/A | N/A |
| 40 clients + SSL + Reconnect | N/A | N/A |
| 120 clients + SSL + Reconnect | N/A | N/A |
| 500 clients + SSL + Reconnect | N/A | N/A |
| 10,000 clients + SSL + Reconnect | N/A | N/A |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | - | - | - |
| 500 clients + Reconnect | - | - | - |
| 120 clients + SSL | - | 3.12 / 3.79 / 7.10 | 0.80 / 2.96 / 5.11 |
| 500 clients + SSL | - | 13.32 / 16.71 / 26.41 | - |
| 10,000 clients + SSL | - | - | - |
| 1 client + SSL + Reconnect | - | 0.15 / 0.29 / 0.33 | 0.17 / 0.28 / 0.32 |
| 40 clients + SSL + Reconnect | - | 22.42 / 54.46 / 72.18 | 18.60 / 38.94 / 51.77 |
| 120 clients + SSL + Reconnect | - | 76.47 / 194.91 / 238.99 | 58.30 / 122.22 / 156.88 |
| 500 clients + SSL + Reconnect | - | 287.80 / 619.42 / 837.16 | 226.23 / 466.95 / 680.04 |
| 10,000 clients + SSL + Reconnect | - | 5015.09 / 9526.63 / 9904.83 | 4817.42 / 8913.56 / 9459.64 |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 120 clients | - | - |
| 10,000 clients | N/A | N/A |
| 120 clients + Reconnect | N/A | N/A |
| 1 client + SSL | N/A | N/A |
| 40 clients + SSL | N/A | N/A |
| 120 clients + SSL | N/A | N/A |
| 500 clients + SSL | N/A | N/A |
| 10,000 clients + SSL | - | - |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 120 clients | - | - | - |
| 10,000 clients | - | 226.31 / 262.72 / 322.46 | 165.40 / 672.29 / 1048.82 |
| 120 clients + Reconnect | - | 6.77 / 15.35 / 20.34 | 8.74 / 15.73 / 19.16 |
| 1 client + SSL | - | 0.08 / 0.12 / 0.15 | 0.12 / 0.19 / 0.24 |
| 40 clients + SSL | - | 0.83 / 1.23 / 1.67 | 0.53 / 1.01 / 1.62 |
| 120 clients + SSL | - | 3.21 / 4.15 / 7.30 | 1.44 / 5.74 / 9.02 |
| 500 clients + SSL | - | 13.63 / 16.54 / 26.75 | 1.54 / 34.25 / 60.68 |
| 10,000 clients + SSL | - | - | - |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client + Reconnect | - | - |
| 1 client + SSL | N/A | N/A |
| 40 clients + SSL | N/A | N/A |
| 120 clients + SSL | N/A | N/A |
| 500 clients + SSL | N/A | N/A |
| 10,000 clients + SSL | - | N/A |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client + Reconnect | - | - | - |
| 1 client + SSL | - | 0.08 / 0.11 / 0.14 | 0.07 / 0.11 / 0.14 |
| 40 clients + SSL | - | 1.00 / 1.42 / 1.61 | 0.32 / 0.52 / 0.78 |
| 120 clients + SSL | - | 3.75 / 6.67 / 8.77 | 0.93 / 3.69 / 6.04 |
| 500 clients + SSL | - | 16.02 / 20.98 / 29.91 | 1.00 / 28.26 / 53.35 |
| 10,000 clients + SSL | - | - | 306.35 / 323.80 / 326.32 |

---

### Notes

- Throughput values are relative ratios — comparable across runs on identical hardware
- Latency values are absolute, measured per-transaction

### Unparsed test names

- -P 1 --protocol=simple postgres -f ${PGBENCH_FILE}
- 1 --protocol=simple --connect postgres -f ${PGBENCH_FILE}
- RT} -U postgres -c 10000 -j ${PGBENCH_JOBS_C10000} -T 30 -P 1 --protocol=extended postgres -f ${PGBENCH_FILE}
- _FILE}
