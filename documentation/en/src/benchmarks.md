---
title: Benchmarks
---

# Benchmarks

pg_doorman vs pgbouncer vs odyssey. Each test runs `pgbench` for 10 seconds through a 40-connection pool.

Last updated: 2026-04-26 17:14 UTC

### Environment

- **Host**: vmcn5q9t
- **Resources**: 8 vCPU / 31.3 GB
- **Workers**: pg_doorman: 4, odyssey: 4
- **Duration per pgbench run**: 10s
- **Started**: 2026-04-26T17:10:02Z
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
| 1 client | -5% | ≈0% |
| 40 clients | x1.7 | -23% |
| 120 clients | x2.1 | -14% |
| 500 clients | x2.0 | -15% |
| 10,000 clients | x1.9 | -13% |
| 1 client + Reconnect | -28% | +23% |
| 40 clients + Reconnect | ≈0% | +43% |
| 120 clients + Reconnect | ≈0% | N/A |
| 500 clients + Reconnect | +9% | +44% |
| 10,000 clients + Reconnect | +16% | +40% |
| 1 client + SSL | ≈0% | -3% |
| 40 clients + SSL | x1.9 | -12% |
| 120 clients + SSL | x2.4 | ≈0% |
| 500 clients + SSL | x2.2 | -9% |
| 10,000 clients + SSL | - | N/A |
| 1 client + SSL + Reconnect | -34% | -5% |
| 40 clients + SSL + Reconnect | -36% | -44% |
| 120 clients + SSL + Reconnect | -5% | -42% |
| 500 clients + SSL + Reconnect | -16% | -37% |
| 10,000 clients + SSL + Reconnect | -30% | -37% |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.08 / 0.10 / 0.12 | 0.07 / 0.10 / 0.11 | 0.07 / 0.11 / 0.13 |
| 40 clients | 0.41 / 0.63 / 0.89 | 0.69 / 1.03 / 1.54 | 0.28 / 0.55 / 0.88 |
| 120 clients | 0.89 / 3.61 / 5.75 | 2.62 / 3.29 / 5.69 | 0.76 / 2.57 / 4.37 |
| 500 clients | 6.01 / 9.82 / 11.63 | 11.17 / 17.89 / 23.27 | 0.93 / 24.41 / 46.34 |
| 10,000 clients | 128.12 / 138.34 / 150.10 | 230.00 / 256.31 / 321.22 | 5.21 / 459.57 / 823.09 |
| 1 client + Reconnect | 0.12 / 0.19 / 0.22 | 0.08 / 0.14 / 0.17 | 0.11 / 0.18 / 0.21 |
| 40 clients + Reconnect | 2.04 / 3.88 / 4.55 | 1.99 / 4.02 / 4.88 | 2.92 / 5.62 / 6.19 |
| 120 clients + Reconnect | 6.23 / 10.94 / 13.70 | 6.02 / 11.18 / 15.14 | 8.68 / 15.68 / 18.71 |
| 500 clients + Reconnect | 26.10 / 45.38 / 48.54 | 27.22 / 51.84 / 65.41 | 36.08 / 64.34 / 69.44 |
| 10,000 clients + Reconnect | 554.35 / 936.70 / 1079.13 | 571.24 / 1191.95 / 1327.94 | 705.78 / 1348.55 / 1523.43 |
| 1 client + SSL | 0.07 / 0.08 / 0.09 | 0.07 / 0.09 / 0.10 | 0.07 / 0.10 / 0.12 |
| 40 clients + SSL | 0.39 / 0.63 / 0.93 | 0.83 / 1.22 / 1.38 | 0.33 / 0.61 / 0.97 |
| 120 clients + SSL | 0.95 / 3.92 / 6.37 | 3.21 / 3.40 / 3.66 | 0.98 / 3.56 / 5.91 |
| 500 clients + SSL | 6.06 / 10.45 / 12.49 | 13.37 / 14.39 / 16.32 | 1.08 / 27.21 / 53.46 |
| 10,000 clients + SSL | - | - | 65.82 / 555.88 / 726.66 |
| 1 client + SSL + Reconnect | 0.22 / 0.34 / 0.36 | 0.15 / 0.28 / 0.31 | 0.23 / 0.32 / 0.36 |
| 40 clients + SSL + Reconnect | 38.85 / 76.89 / 85.22 | 22.75 / 58.00 / 79.27 | 21.35 / 45.70 / 57.01 |
| 120 clients + SSL + Reconnect | 98.39 / 221.47 / 247.02 | 93.12 / 213.48 / 251.31 | 59.15 / 123.91 / 162.44 |
| 500 clients + SSL + Reconnect | 372.84 / 839.98 / 979.66 | 280.93 / 674.53 / 778.16 | 239.06 / 477.18 / 583.75 |
| 10,000 clients + SSL + Reconnect | 4806.59 / 9422.18 / 9900.29 | 4864.54 / 9393.79 / 9858.06 | 4674.85 / 9292.50 / 9840.07 |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | +7% | +44% |
| 40 clients | x1.8 | +31% |
| 120 clients | x2.3 | x1.5 |
| 500 clients | x2.1 | x1.5 |
| 10,000 clients | x1.9 | +44% |
| 1 client + Reconnect | -15% | +28% |
| 40 clients + Reconnect | +10% | +49% |
| 120 clients + Reconnect | +9% | +43% |
| 500 clients + Reconnect | +7% | +43% |
| 10,000 clients + Reconnect | +14% | +37% |
| 1 client + SSL | +7% | x1.6 |
| 40 clients + SSL | x2.0 | +42% |
| 120 clients + SSL | x2.4 | x1.5 |
| 500 clients + SSL | x2.3 | +45% |
| 10,000 clients + SSL | - | N/A |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.07 / 0.09 / 0.10 | 0.08 / 0.10 / 0.12 | 0.10 / 0.14 / 0.17 |
| 40 clients | 0.42 / 0.64 / 0.91 | 0.77 / 1.16 / 1.37 | 0.51 / 0.95 / 1.50 |
| 120 clients | 0.89 / 3.72 / 5.85 | 2.67 / 4.79 / 6.02 | 1.36 / 5.00 / 7.65 |
| 500 clients | 5.61 / 9.14 / 10.69 | 11.24 / 14.09 / 21.50 | 1.43 / 33.52 / 60.38 |
| 10,000 clients | 125.42 / 135.90 / 148.53 | 230.00 / 255.34 / 330.87 | 165.69 / 684.98 / 1067.61 |
| 1 client + Reconnect | 0.08 / 0.14 / 0.16 | 0.07 / 0.10 / 0.15 | 0.12 / 0.17 / 0.22 |
| 40 clients + Reconnect | 1.97 / 3.96 / 4.28 | 2.09 / 4.26 / 5.46 | 2.96 / 5.77 / 6.22 |
| 120 clients + Reconnect | 6.05 / 10.53 / 12.97 | 6.30 / 13.24 / 18.78 | 8.39 / 15.15 / 18.38 |
| 500 clients + Reconnect | 26.51 / 45.96 / 49.25 | 27.11 / 50.33 / 65.00 | 36.87 / 66.01 / 69.74 |
| 10,000 clients + Reconnect | 547.10 / 935.93 / 1124.95 | 619.03 / 1094.35 / 1206.60 | 718.48 / 1330.87 / 1416.80 |
| 1 client + SSL | 0.07 / 0.08 / 0.09 | 0.08 / 0.09 / 0.10 | 0.11 / 0.15 / 0.19 |
| 40 clients + SSL | 0.43 / 0.64 / 0.79 | 0.88 / 1.29 / 1.62 | 0.60 / 0.97 / 1.43 |
| 120 clients + SSL | 1.04 / 4.46 / 7.22 | 3.40 / 5.79 / 7.51 | 1.55 / 5.88 / 9.12 |
| 500 clients + SSL | 6.22 / 10.21 / 11.97 | 13.93 / 15.06 / 24.04 | 1.48 / 36.55 / 64.47 |
| 10,000 clients + SSL | - | - | 185.36 / 635.52 / 758.43 |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | ≈0% | ≈0% |
| 40 clients | x2.3 | -21% |
| 120 clients | x2.6 | -15% |
| 500 clients | x2.8 | -17% |
| 10,000 clients | x2.5 | -10% |
| 1 client + Reconnect | -11% | +27% |
| 40 clients + Reconnect | +8% | x1.5 |
| 120 clients + Reconnect | +4% | +45% |
| 500 clients + Reconnect | -4% | +40% |
| 10,000 clients + Reconnect | +6% | +39% |
| 1 client + SSL | ≈0% | -7% |
| 40 clients + SSL | x2.3 | -20% |
| 120 clients + SSL | x2.9 | -6% |
| 500 clients + SSL | x3.2 | -12% |
| 10,000 clients + SSL | - | - |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.07 / 0.09 / 0.11 | 0.08 / 0.09 / 0.11 | 0.07 / 0.10 / 0.12 |
| 40 clients | 0.41 / 0.60 / 0.74 | 0.96 / 1.40 / 1.70 | 0.29 / 0.52 / 0.82 |
| 120 clients | 0.89 / 3.62 / 5.64 | 3.14 / 3.90 / 7.12 | 0.77 / 2.64 / 4.40 |
| 500 clients | 5.89 / 9.59 / 11.21 | 13.31 / 26.93 / 28.95 | 0.88 / 23.42 / 45.05 |
| 10,000 clients | 121.66 / 134.01 / 179.22 | 298.62 / 348.01 / 481.62 | 3.52 / 497.14 / 809.00 |
| 1 client + Reconnect | 0.14 / 0.26 / 0.31 | 0.14 / 0.23 / 0.28 | 0.17 / 0.32 / 0.38 |
| 40 clients + Reconnect | 2.76 / 5.29 / 5.79 | 2.90 / 5.72 / 7.35 | 4.19 / 7.90 / 8.60 |
| 120 clients + Reconnect | 8.38 / 14.93 / 17.63 | 8.46 / 16.01 / 20.61 | 11.81 / 21.54 / 25.07 |
| 500 clients + Reconnect | 36.35 / 64.78 / 69.58 | 34.01 / 62.10 / 72.21 | 48.64 / 88.37 / 96.49 |
| 10,000 clients + Reconnect | 715.31 / 1299.84 / 1459.65 | 739.38 / 1404.39 / 1625.08 | 974.59 / 1790.82 / 1950.88 |
| 1 client + SSL | 0.09 / 0.10 / 0.12 | 0.09 / 0.11 / 0.13 | 0.08 / 0.11 / 0.14 |
| 40 clients + SSL | 0.42 / 0.63 / 0.83 | 1.00 / 1.50 / 2.17 | 0.33 / 0.51 / 0.76 |
| 120 clients + SSL | 1.03 / 4.13 / 6.33 | 3.76 / 7.49 / 8.80 | 0.97 / 3.59 / 5.89 |
| 500 clients + SSL | 6.49 / 10.61 / 12.35 | 16.69 / 32.89 / 35.35 | 1.01 / 28.05 / 53.26 |
| 10,000 clients + SSL | - | - | - |

---

### Notes

- Throughput values are relative ratios — comparable across runs on identical hardware
- Latency values are absolute, measured per-transaction

### Unparsed test names

- bench-wrap
