---
title: Benchmarks
---

# Performance Benchmarks

## Introduction

Performance benchmarks provide valuable insights into the operational characteristics of connection poolers. While these benchmarks offer a comparative view of different poolers, we strongly recommend conducting your own tests in an environment that matches your production setup for the most accurate results.

The benchmarks presented here are designed to:
- Compare PgDoorman with other popular PostgreSQL connection poolers
- Demonstrate performance under different workload scenarios
- Highlight the resource utilization patterns of each pooler

## Testing Environment

All benchmarks were conducted with the following configuration:
- PostgreSQL 15
- Server with 20 CPU cores
- Connection pool size: 40 connections
- Client authentication: SCRAM
- Testing tool: pgbench
- Test duration: 600 seconds (10 minutes)
- Client connections: 80
- Job parallelism: 4

## Benchmark Scenarios

### 1. Prepared Statements without SSL

This scenario tests the performance of prepared statements execution without SSL encryption.

**Command:**
```shell
PGSSLMODE=disable pgbench -s 10 -T 600 -P 1 -S -n -j 4 -c 80 -M prepared -r
```

**Results:**

| Pooler     | CPU Usage | Transactions Per Second |
|------------|-----------|-------------------------|
| PostgreSQL | -         | 230,000                 |
| PgBouncer  | 1 core    | 45,000                  |
| Odyssey    | 5-6 cores | 125,000                 |
| PgCat      | -         | Not available*          |
| PgDoorman  | 6-7 cores | 135,000                 |

*Note: PgCat (version 1.3.0) was configured with `prepared_statements_cache_size = 500`, but prepared statements with pgbench were not functioning correctly.

### 2. Simple Queries without SSL

This scenario tests the performance of simple queries without SSL encryption.

**Command:**
```shell
PGSSLMODE=disable pgbench -s 10 -T 600 -P 1 -S -n -j 4 -c 80 -r
```

**Results:**

| Pooler     | CPU Usage | Transactions Per Second |
|------------|-----------|-------------------------|
| PostgreSQL | -         | 150,000                 |
| PgBouncer  | 1 core    | 60,000                  |
| Odyssey    | 4 cores   | 105,000                 |
| PgCat      | 3-4 cores | 85,000                  |
| PgDoorman  | 4-5 cores | 110,000                 |

### 3. Reconnect Performance with SSL

This scenario tests the connection establishment rate with SSL encryption enabled.

**Command:**
```shell
PGSSLMODE=require pgbench -s 10 -T 600 -P 1 -S -n -j 4 -c 80 -M prepared -r -C
```

**Results:**

| Pooler     | CPU Usage | Connections Per Second |
|------------|-----------|------------------------|
| PostgreSQL | -         | 190                    |
| PgBouncer  | 1 core    | 240                    |
| Odyssey    | 1 core    | 260*                   |
| PgCat      | 1 core    | 530**                  |
| PgDoorman  | 1 core    | 260                    |

**Notes:**
* *Odyssey occasionally produced authentication errors: `error: connection to server at "10.251.28.154", port 6432 failed: ERROR: odyssey: c3d386c5e5c2f: password authentication failed`.
* **PgCat (using rustls) showed higher connection rates than PgDoorman (using OpenSSL), but we encountered compatibility issues with rustls when using certain client drivers like npgsql, which led us to adopt OpenSSL wrappers instead.

## Conclusion

These benchmarks demonstrate that PgDoorman offers competitive performance compared to other PostgreSQL connection poolers. While it may use more CPU resources in some scenarios, it delivers higher throughput, particularly for prepared statements and simple queries.

When selecting a connection pooler, consider not only raw performance but also stability, feature set, and compatibility with your specific application stack.
