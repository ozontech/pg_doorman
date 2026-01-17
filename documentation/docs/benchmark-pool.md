### Internal Pool.get Benchmark Results

**Generated:** 2026-01-17 18:01:27 UTC

#### Overview

This document contains benchmark results for the internal `Pool.get` operation.
These benchmarks measure the overhead of acquiring a connection from the pool
with real PostgreSQL connections.

#### Test Environment

- **PostgreSQL:** Local instance with `max_connections=200`
- **Authentication:** Trust (no password)
- **Connection:** TCP to 127.0.0.1

#### Single Client Results

| Test | Throughput | p50 | p95 | p99 |
|------|------------|-----|-----|-----|
| fifo_pool10 | 1144932 ops/sec | 583 ns | 625 ns | 625 ns |
| lifo_pool10 | 1138981 ops/sec | 583 ns | 625 ns | 666 ns |
| single_pool1 | 1134604 ops/sec | 583 ns | 625 ns | 750 ns |
| single_pool10 | 1135819 ops/sec | 583 ns | 625 ns | 666 ns |
| single_pool50 | 1141100 ops/sec | 583 ns | 625 ns | 625 ns |

#### Concurrent Client Results

| Test | Throughput | p50 | p95 | p99 |
|------|------------|-----|-----|-----|
| concurrent_100c_20p | 1648406 ops/sec | 1834 ns | 3250 ns | 5417 ns |
| concurrent_20c_5p | 1600380 ops/sec | 2001 ns | 6709 ns | 162251 ns |
| concurrent_50c_10p | 1701100 ops/sec | 1917 ns | 3542 ns | 5958 ns |

#### Analysis

##### Key Observations

- **Single client throughput (pool_size=1):** 1134604 ops/sec
- **Median latency (p50):** 583 ns
- **Tail latency (p99):** 750 ns

##### Methodology

1. **Single client tests:** One client repeatedly calls `pool.get()` and immediately
   returns the connection to the pool. This measures the pure pool overhead.

2. **Concurrent tests:** Multiple clients compete for connections from a smaller pool.
   This measures contention handling and semaphore performance.

3. **Queue mode comparison:** FIFO vs LIFO modes are compared to understand
   the impact of connection reuse patterns.
