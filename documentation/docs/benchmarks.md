---
title: Benchmarks
---

## Disclaimer:

Benchmarks always lie to you :) We recommend that you benchmark the application yourself and verify the results.
The current benchmarks provided here are intended to give you an idea of approximate figures.

In addition to being fast, a pooler should also be predictable and error-free.
While benchmarks may not provide real-world experience, they can reveal potential problems.

## Smoke perf test

PG 15, CPU 20, pool_size = 40, client with SCRAM auth.

### Prepared without SSL: 

```shell
PGSSLMODE=disable pgbench -s 10 -T 600 -P 1 -S -n -j 4 -c 80 -M prepared -r
```

| Pooler     | Usage   | TPS     |
|------------|---------|---------|
| PostgreSQL | -       | 230_000 |
| PgBouncer  | 1 CPU   | 45_000  |
| Odyssey    | 5-6 CPU | 125_000 |
| PgCat      | -       | -       |
| PgDoorman  | 6-7 CPU | 135_000 |

Notes:
  + pgcat (1.3.0) settings: `prepared_statements_cache_size = 500`, but prepared with pgbench is not working.


### Simple without SSL: 

```shell
PGSSLMODE=disable pgbench -s 10 -T 600 -P 1 -S -n -j 4 -c 80 -r
```

| Pooler     | Usage   | TPS     |
|------------|---------|---------|
| PostgreSQL | -       | 150_000 |
| PgBouncer  | 1 CPU   | 60_000  |
| Odyssey    | 4 CPU   | 105_000 |
| PgCat      | 3-4 CPU | 85_000  |
| PgDoorman  | 4-5 CPU | 110_000 |

### Reconnect with SSL:

```shell
PGSSLMODE=require PGHOST=10.251.28.154 pgbench -s 10 -T 600 -P 1 -S -n -j 4 -c 80 -M prepared -r -C
```

| Pooler     | Usage | TPS |
|------------|-------|-----|
| PostgreSQL | -     | 190 |
| PgBouncer  | 1 CPU | 240 |
| Odyssey    | 1 CPU | 260 |
| PgCat      | 1 CPU | 530 |
| PgDoorman  | 1 CPU | 260 |

Notes: 
  + odyssey irregular errors: `error: connection to server at "10.251.28.154", port 6432 failed: ERROR:  odyssey: c3d386c5e5c2f: password authentication failed`.
  + pgcat (rutls) is faster pg_doorman (openssl), but we experienced problems when using this library (rutls) with npgsql, and we had switched to openssl-wrappers.
