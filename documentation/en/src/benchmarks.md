---
title: Benchmarks
---

# Benchmarks

pg_doorman vs pgbouncer vs odyssey. Each test runs `pgbench` for 30 seconds through a 40-connection pool.

Last updated: 2026-04-27 05:36 UTC

### Environment

- **Host**: vm51qgfh
- **Resources**: 16 vCPU / 62.8 GB
- **Workers**: pg_doorman: 12, odyssey: 12
- **Duration per pgbench run**: 30s
- **Started**: 2026-04-27T05:14:44Z
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
| 1 client | +6% | +5% |
| 40 clients | x2.2 | -39% |
| 120 clients | x5.4 | -16% |
| 500 clients | x5.4 | ≈0% |
| 10,000 clients | x4.6 | +18% |
| 1 client + Reconnect | -28% | +30% |
| 40 clients + Reconnect | x1.5 | x1.5 |
| 120 clients + Reconnect | x1.6 | x1.5 |
| 500 clients + Reconnect | x1.6 | x1.5 |
| 10,000 clients + Reconnect | x1.7 | x1.7 |
| 1 client + SSL | ≈0% | ≈0% |
| 40 clients + SSL | x2.4 | -36% |
| 120 clients + SSL | x6.0 | -10% |
| 500 clients + SSL | x6.1 | -3% |
| 10,000 clients + SSL | x5.2 | +12% |
| 1 client + SSL + Reconnect | -23% | +11% |
| 40 clients + SSL + Reconnect | +12% | -36% |
| 120 clients + SSL + Reconnect | +16% | -38% |
| 500 clients + SSL + Reconnect | +36% | -29% |
| 10,000 clients + SSL + Reconnect | +34% | N/A |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.07 / 0.08 / 0.09 | 0.07 / 0.09 / 0.10 | 0.07 / 0.10 / 0.12 |
| 40 clients | 0.26 / 0.91 / 1.06 | 0.72 / 1.05 / 1.21 | 0.16 / 0.35 / 0.48 |
| 120 clients | 0.42 / 1.61 / 2.55 | 2.80 / 6.02 / 6.48 | 0.31 / 1.15 / 2.17 |
| 500 clients | 2.88 / 4.68 / 5.87 | 12.22 / 23.64 / 25.97 | 0.86 / 10.08 / 17.53 |
| 10,000 clients | 54.08 / 57.62 / 60.53 | 247.46 / 289.64 / 314.20 | 1.54 / 313.76 / 522.98 |
| 1 client + Reconnect | 0.13 / 0.19 / 0.22 | 0.07 / 0.12 / 0.15 | 0.12 / 0.19 / 0.22 |
| 40 clients + Reconnect | 1.10 / 2.24 / 2.49 | 1.52 / 3.70 / 5.00 | 1.66 / 3.31 / 3.59 |
| 120 clients + Reconnect | 3.33 / 6.18 / 7.60 | 4.88 / 11.76 / 15.38 | 5.05 / 9.23 / 10.91 |
| 500 clients + Reconnect | 14.38 / 25.44 / 31.01 | 20.82 / 47.58 / 60.52 | 21.87 / 39.46 / 43.44 |
| 10,000 clients + Reconnect | 297.36 / 516.43 / 552.18 | 482.52 / 918.16 / 1008.53 | 471.62 / 892.89 / 956.15 |
| 1 client + SSL | 0.08 / 0.09 / 0.11 | 0.08 / 0.10 / 0.11 | 0.08 / 0.11 / 0.14 |
| 40 clients + SSL | 0.28 / 1.05 / 1.24 | 0.88 / 1.30 / 1.43 | 0.18 / 0.42 / 0.64 |
| 120 clients + SSL | 0.47 / 1.80 / 2.90 | 3.49 / 7.42 / 7.99 | 0.34 / 1.30 / 3.69 |
| 500 clients + SSL | 3.22 / 5.41 / 6.74 | 15.41 / 30.09 / 32.86 | 1.02 / 10.64 / 19.16 |
| 10,000 clients + SSL | 64.35 / 69.95 / 72.92 | 332.94 / 385.05 / 454.47 | 2.64 / 354.34 / 634.61 |
| 1 client + SSL + Reconnect | 0.23 / 0.32 / 0.35 | 0.13 / 0.26 / 0.31 | 0.19 / 0.31 / 0.36 |
| 40 clients + SSL + Reconnect | 15.40 / 33.09 / 39.39 | 16.24 / 40.33 / 51.72 | 10.25 / 19.73 / 25.22 |
| 120 clients + SSL + Reconnect | 47.50 / 100.21 / 117.73 | 52.68 / 122.60 / 152.29 | 30.01 / 57.28 / 70.98 |
| 500 clients + SSL + Reconnect | 178.87 / 373.73 / 440.03 | 235.39 / 523.75 / 633.69 | 127.94 / 243.49 / 292.94 |
| 10,000 clients + SSL + Reconnect | 3459.74 / 6714.82 / 7216.46 | 4657.31 / 9101.84 / 10405.15 | 2470.54 / 4808.30 / 5378.25 |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | x1.6 |
| 40 clients | x2.4 | -6% |
| 120 clients | x4.9 | +39% |
| 500 clients | x5.2 | x1.6 |
| 10,000 clients | x4.7 | x1.9 |
| 1 client + Reconnect | -23% | N/A |
| 40 clients + Reconnect | +46% | +50% |
| 120 clients + Reconnect | x1.6 | N/A |
| 500 clients + Reconnect | x1.6 | x1.5 |
| 10,000 clients + Reconnect | x1.7 | x1.7 |
| 1 client + SSL | +4% | x1.6 |
| 40 clients + SSL | x2.5 | ≈0% |
| 120 clients + SSL | x6.0 | +41% |
| 500 clients + SSL | x6.1 | x1.6 |
| 10,000 clients + SSL | x5.1 | x1.8 |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.08 / 0.09 / 0.10 | 0.08 / 0.09 / 0.11 | 0.12 / 0.17 / 0.19 |
| 40 clients | 0.26 / 0.97 / 1.15 | 0.76 / 1.13 / 1.29 | 0.26 / 0.54 / 0.73 |
| 120 clients | 0.42 / 1.67 / 2.75 | 2.93 / 5.50 / 6.37 | 0.69 / 1.91 / 3.02 |
| 500 clients | 2.93 / 4.65 / 5.67 | 11.98 / 23.23 / 25.83 | 1.10 / 16.24 / 27.96 |
| 10,000 clients | 54.64 / 59.00 / 62.88 | 256.75 / 294.68 / 310.26 | 1.82 / 513.87 / 840.13 |
| 1 client + Reconnect | 0.13 / 0.19 / 0.22 | 0.10 / 0.16 / 0.20 | 0.19 / 0.29 / 0.33 |
| 40 clients + Reconnect | 1.13 / 2.29 / 2.55 | 1.52 / 3.65 / 4.92 | 1.68 / 3.40 / 3.67 |
| 120 clients + Reconnect | 3.34 / 6.16 / 7.59 | 4.83 / 11.55 / 15.19 | 5.12 / 9.33 / 11.18 |
| 500 clients + Reconnect | 14.49 / 25.69 / 31.14 | 20.92 / 47.99 / 60.57 | 22.33 / 40.31 / 44.02 |
| 10,000 clients + Reconnect | 294.30 / 511.12 / 545.75 | 488.53 / 948.28 / 1069.20 | 489.24 / 906.83 / 990.74 |
| 1 client + SSL | 0.08 / 0.09 / 0.11 | 0.09 / 0.10 / 0.12 | 0.12 / 0.19 / 0.21 |
| 40 clients + SSL | 0.29 / 1.07 / 1.34 | 0.90 / 1.33 / 1.51 | 0.31 / 0.66 / 0.93 |
| 120 clients + SSL | 0.47 / 1.92 / 3.20 | 3.63 / 7.59 / 8.19 | 0.62 / 2.45 / 4.93 |
| 500 clients + SSL | 3.31 / 5.49 / 6.80 | 16.07 / 29.65 / 32.70 | 1.12 / 19.10 / 33.44 |
| 10,000 clients + SSL | 65.31 / 71.35 / 75.42 | 333.27 / 391.92 / 438.61 | 4.25 / 485.92 / 927.66 |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | +5% | ≈0% |
| 40 clients | x3.1 | -41% |
| 120 clients | x6.2 | -17% |
| 500 clients | x6.4 | ≈0% |
| 10,000 clients | x5.6 | +21% |
| 1 client + Reconnect | -14% | x1.5 |
| 40 clients + Reconnect | x1.5 | +49% |
| 120 clients + Reconnect | x1.6 | x1.5 |
| 500 clients + Reconnect | x1.6 | x1.5 |
| 10,000 clients + Reconnect | x1.6 | x1.5 |
| 1 client + SSL | +4% | ≈0% |
| 40 clients + SSL | x3.2 | -36% |
| 120 clients + SSL | x6.8 | -14% |
| 500 clients + SSL | x7.0 | ≈0% |
| 10,000 clients + SSL | x6.1 | +18% |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.07 / 0.08 / 0.10 | 0.07 / 0.09 / 0.11 | 0.07 / 0.10 / 0.12 |
| 40 clients | 0.26 / 0.94 / 1.10 | 0.99 / 1.38 / 1.52 | 0.15 / 0.34 / 0.48 |
| 120 clients | 0.43 / 1.60 / 2.57 | 3.45 / 7.29 / 7.83 | 0.30 / 1.16 / 2.37 |
| 500 clients | 2.96 / 4.79 / 5.86 | 14.58 / 28.72 / 31.73 | 0.80 / 9.99 / 17.61 |
| 10,000 clients | 55.04 / 57.59 / 60.42 | 306.20 / 359.48 / 408.17 | 1.83 / 325.18 / 576.75 |
| 1 client + Reconnect | 0.19 / 0.31 / 0.35 | 0.15 / 0.26 / 0.32 | 0.29 / 0.43 / 0.48 |
| 40 clients + Reconnect | 1.65 / 3.16 / 3.46 | 2.37 / 5.46 / 7.27 | 2.44 / 4.64 / 4.97 |
| 120 clients + Reconnect | 4.75 / 8.80 / 10.36 | 6.88 / 16.51 / 21.82 | 7.12 / 12.97 / 14.90 |
| 500 clients + Reconnect | 19.54 / 35.17 / 40.06 | 29.77 / 64.86 / 79.54 | 29.78 / 54.53 / 59.98 |
| 10,000 clients + Reconnect | 401.09 / 721.41 / 764.97 | 605.32 / 1143.50 / 1269.70 | 613.07 / 1132.42 / 1272.94 |
| 1 client + SSL | 0.08 / 0.10 / 0.11 | 0.09 / 0.10 / 0.11 | 0.08 / 0.11 / 0.13 |
| 40 clients + SSL | 0.28 / 1.03 / 1.29 | 1.16 / 1.64 / 1.87 | 0.17 / 0.41 / 0.65 |
| 120 clients + SSL | 0.49 / 1.88 / 3.05 | 4.20 / 8.93 / 9.58 | 0.35 / 1.38 / 3.56 |
| 500 clients + SSL | 3.23 / 5.30 / 6.41 | 18.33 / 33.16 / 37.79 | 0.82 / 11.69 / 21.25 |
| 10,000 clients + SSL | 63.08 / 68.10 / 72.38 | 387.37 / 447.98 / 566.76 | 2.19 / 363.33 / 646.83 |

---

### Notes

- Throughput values are relative ratios — comparable across runs on identical hardware
- Latency values are absolute, measured per-transaction

### Unparsed test names

- bench-wrap
