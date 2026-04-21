---
title: Benchmarks
---

# Benchmarks

pg_doorman vs pgbouncer vs odyssey. Each test runs `pgbench` for 30 seconds through a 40-connection pool.

Last updated: 2026-04-21 18:54 UTC

### Environment

- **Instance**: AWS Fargate (16 vCPU, 32 GB RAM)
- **Workers**: pg_doorman: 12, odyssey: 12
- **pgbench jobs**: 4 (global override)
- **Started**: 2026-04-21 17:31:01 UTC
- **Finished**: 2026-04-21 18:54:11 UTC
- **Total duration**: 1h 23m 9s

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
| 1 client | -3% | -9% |
| 40 clients | +82% | -5% |
| 120 clients | x2.6 | ≈0% |
| 500 clients | x2.5 | +6% |
| 10,000 clients | x2.7 | +18% |
| 1 client + Reconnect | -6% | x9.0 |
| 40 clients + Reconnect | +21% | x2.1 |
| 120 clients + Reconnect | +19% | +98% |
| 500 clients + Reconnect | +20% | +97% |
| 10,000 clients + Reconnect | +63% | x2.1 |
| 1 client + SSL | -8% | -8% |
| 40 clients + SSL | x2.1 | ≈0% |
| 120 clients + SSL | x3.1 | +6% |
| 500 clients + SSL | x3.0 | +11% |
| 10,000 clients + SSL | x3.2 | +17% |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.07 / 0.07 / 0.08 | 0.07 / 0.08 / 0.08 | 0.06 / 0.07 / 0.07 |
| 40 clients | 0.26 / 0.36 / 0.44 | 0.46 / 0.67 / 0.71 | 0.22 / 0.42 / 0.57 |
| 120 clients | 0.49 / 1.86 / 3.03 | 1.81 / 2.12 / 2.24 | 0.54 / 1.33 / 1.84 |
| 500 clients | 3.54 / 5.64 / 6.46 | 7.59 / 8.71 / 9.33 | 0.99 / 13.19 / 22.41 |
| 10,000 clients | 69.42 / 71.72 / 75.83 | 184.39 / 202.56 / 213.44 | 2.70 / 326.10 / 570.34 |
| 1 client + Reconnect | 0.06 / 0.08 / 0.09 | 0.06 / 0.06 / 0.07 | 0.07 / 0.09 / 0.10 |
| 40 clients + Reconnect | 1.07 / 2.19 / 2.52 | 1.02 / 2.34 / 11.93 | 1.00 / 2.74 / 3.35 |
| 120 clients + Reconnect | 3.36 / 6.33 / 7.62 | 3.37 / 7.10 / 31.61 | 4.67 / 9.45 / 11.54 |
| 500 clients + Reconnect | 13.77 / 25.15 / 28.86 | 13.61 / 30.11 / 125.79 | 22.99 / 41.95 / 48.02 |
| 10,000 clients + Reconnect | 295.83 / 515.96 / 559.54 | 562.42 / 926.93 / 972.88 | 597.47 / 1078.06 / 1365.21 |
| 1 client + SSL | 0.08 / 0.09 / 0.10 | 0.08 / 0.08 / 0.09 | 0.07 / 0.09 / 0.09 |
| 40 clients + SSL | 0.29 / 0.44 / 0.56 | 0.64 / 0.93 / 1.00 | 0.27 / 0.51 / 0.67 |
| 120 clients + SSL | 0.59 / 2.32 / 3.93 | 2.56 / 2.93 / 3.14 | 0.67 / 1.65 / 2.30 |
| 500 clients + SSL | 4.16 / 6.85 / 7.84 | 10.89 / 12.64 / 13.55 | 1.23 / 16.20 / 27.94 |
| 10,000 clients + SSL | 82.15 / 86.57 / 91.75 | 262.65 / 289.09 / 367.39 | 4.24 / 387.21 / 753.84 |

---

## Extended protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | +5% | +40% |
| 40 clients | +98% | +43% |
| 120 clients | x2.8 | +60% |
| 500 clients | x2.7 | +64% |
| 10,000 clients | x2.8 | +74% |
| 1 client + Reconnect | -4% | x3.0 |
| 40 clients + Reconnect | +20% | x2.2 |
| 120 clients + Reconnect | +21% | +88% |
| 500 clients + Reconnect | +21% | +100% |
| 10,000 clients + Reconnect | +61% | +27% |
| 1 client + SSL | +4% | +36% |
| 40 clients + SSL | x2.3 | +48% |
| 120 clients + SSL | x3.2 | +65% |
| 500 clients + SSL | x3.4 | +69% |
| 10,000 clients + SSL | x3.4 | +73% |
| 1 client + SSL + Reconnect | +9% | +13% |
| 40 clients + SSL + Reconnect | +96% | +5% |
| 120 clients + SSL + Reconnect | +99% | +5% |
| 500 clients + SSL + Reconnect | x2.0 | +5% |
| 10,000 clients + SSL + Reconnect | +93% | +5% |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.07 / 0.07 / 0.08 | 0.07 / 0.08 / 0.08 | 0.09 / 0.10 / 0.11 |
| 40 clients | 0.25 / 0.35 / 0.43 | 0.48 / 0.72 / 0.76 | 0.32 / 0.62 / 0.84 |
| 120 clients | 0.47 / 1.82 / 2.99 | 1.87 / 2.27 / 2.39 | 0.83 / 2.33 / 3.48 |
| 500 clients | 3.45 / 5.48 / 6.26 | 7.77 / 9.35 / 9.69 | 1.31 / 18.35 / 31.99 |
| 10,000 clients | 67.94 / 70.05 / 72.43 | 188.62 / 206.44 / 218.36 | 3.74 / 468.64 / 810.47 |
| 1 client + Reconnect | 0.07 / 0.08 / 0.08 | 0.06 / 0.08 / 0.09 | 0.10 / 0.12 / 0.13 |
| 40 clients + Reconnect | 1.08 / 2.21 / 2.55 | 1.03 / 2.40 / 11.13 | 0.92 / 2.77 / 3.66 |
| 120 clients + Reconnect | 3.27 / 6.20 / 7.44 | 3.23 / 7.11 / 33.54 | 4.61 / 9.52 / 11.37 |
| 500 clients + Reconnect | 13.83 / 25.40 / 29.12 | 13.80 / 30.20 / 118.88 | 23.89 / 43.77 / 67.05 |
| 10,000 clients + Reconnect | 298.47 / 519.98 / 573.71 | 569.79 / 921.35 / 966.33 | 549.71 / 1052.37 / 1402.38 |
| 1 client + SSL | 0.08 / 0.09 / 0.09 | 0.08 / 0.10 / 0.10 | 0.11 / 0.12 / 0.13 |
| 40 clients + SSL | 0.29 / 0.44 / 0.59 | 0.67 / 0.99 / 1.07 | 0.39 / 0.80 / 1.08 |
| 120 clients + SSL | 0.56 / 2.31 / 3.90 | 2.62 / 3.06 / 3.24 | 1.07 / 2.90 / 4.35 |
| 500 clients + SSL | 4.16 / 6.87 / 7.88 | 12.30 / 14.21 / 15.64 | 1.71 / 22.66 / 38.83 |
| 10,000 clients + SSL | 81.93 / 86.01 / 89.18 | 280.73 / 308.19 / 385.99 | 139.94 / 557.71 / 844.51 |
| 1 client + SSL + Reconnect | 0.10 / 0.12 / 0.13 | 0.08 / 0.10 / 0.11 | 0.09 / 0.12 / 0.12 |
| 40 clients + SSL + Reconnect | 12.31 / 23.03 / 25.02 | 24.07 / 44.29 / 46.70 | 12.99 / 24.30 / 26.07 |
| 120 clients + SSL + Reconnect | 37.28 / 69.57 / 76.18 | 73.98 / 137.78 / 147.32 | 39.21 / 72.80 / 79.97 |
| 500 clients + SSL + Reconnect | 157.07 / 292.82 / 319.41 | 311.77 / 593.21 / 673.84 | 165.06 / 306.22 / 336.63 |
| 10,000 clients + SSL + Reconnect | 2954.51 / 5951.58 / 6556.46 | 5078.21 / 11531.36 / 12358.70 | 3096.16 / 6192.08 / 6844.73 |

---

## Prepared protocol

### Throughput

| Test | vs pgbouncer | vs odyssey |
|------|--------------|------------|
| 1 client | -4% | -8% |
| 40 clients | x2.4 | -7% |
| 120 clients | x3.5 | ≈0% |
| 500 clients | x3.3 | +8% |
| 10,000 clients | x3.1 | +16% |
| 1 client + Reconnect | ≈0% | ∞ |
| 40 clients + Reconnect | ≈0% | ∞ |
| 120 clients + Reconnect | ≈0% | ∞ |
| 500 clients + Reconnect | +4% | ∞ |
| 10,000 clients + Reconnect | +25% | ∞ |
| 1 client + SSL | -4% | -5% |
| 40 clients + SSL | x2.7 | ≈0% |
| 120 clients + SSL | x3.8 | +6% |
| 500 clients + SSL | x3.7 | +11% |
| 10,000 clients + SSL | x3.9 | +15% |

### Latency — p50 / p95 / p99 (ms)

| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |
|------|----------------|----------------|--------------|
| 1 client | 0.06 / 0.07 / 0.07 | 0.06 / 0.07 / 0.07 | 0.06 / 0.07 / 0.07 |
| 40 clients | 0.24 / 0.34 / 0.42 | 0.60 / 0.88 / 0.91 | 0.20 / 0.40 / 0.53 |
| 120 clients | 0.47 / 1.72 / 2.79 | 2.25 / 2.60 / 2.69 | 0.51 / 1.27 / 1.79 |
| 500 clients | 3.31 / 5.28 / 6.04 | 9.35 / 10.58 / 11.20 | 0.90 / 12.75 / 21.56 |
| 10,000 clients | 66.31 / 69.32 / 72.46 | 205.26 / 221.39 / 243.49 | 2.76 / 306.09 / 534.07 |
| 1 client + Reconnect | 0.11 / 0.13 / 0.14 | 0.10 / 0.12 / 0.13 | 0.15 / 0.18 / 0.19 |
| 40 clients + Reconnect | 1.56 / 3.01 / 3.33 | 1.59 / 2.95 / 3.31 | 1.86 / 3.56 / 4.61 |
| 120 clients + Reconnect | 4.49 / 8.39 / 9.73 | 4.60 / 8.55 / 9.72 | 6.76 / 83.39 / 88.89 |
| 500 clients + Reconnect | 18.64 / 34.07 / 37.72 | 19.43 / 35.46 / 39.86 | 24.27 / 197.01 / 297.22 |
| 10,000 clients + Reconnect | 396.39 / 710.66 / 769.05 | 483.59 / 927.03 / 1000.58 | 483.21 / 1256.91 / 1563.08 |
| 1 client + SSL | 0.08 / 0.09 / 0.09 | 0.07 / 0.08 / 0.09 | 0.07 / 0.08 / 0.09 |
| 40 clients + SSL | 0.28 / 0.42 / 0.57 | 0.80 / 1.15 / 1.24 | 0.24 / 0.48 / 0.65 |
| 120 clients + SSL | 0.56 / 2.19 / 3.67 | 3.00 / 3.27 / 3.49 | 0.63 / 1.60 / 2.27 |
| 500 clients + SSL | 3.95 / 6.56 / 7.51 | 12.70 / 13.91 / 15.23 | 1.10 / 15.54 / 26.49 |
| 10,000 clients + SSL | 79.24 / 84.27 / 88.85 | 308.74 / 326.58 / 488.80 | 5.17 / 364.12 / 633.46 |

---

### Notes

- Odyssey performs poorly with extended query protocol in transaction pooling mode
- Throughput values are relative ratios — comparable across runs on identical hardware
- Latency values are absolute, measured per-transaction
