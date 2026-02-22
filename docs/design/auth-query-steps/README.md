# auth_query Implementation Steps

Detailed implementation plans for the [auth_query design](../auth-query.md).
Each step is a self-contained PR with clear scope, dependencies, and checklist.

## Dependency graph

```
Step 1 (config) ─→ Step 2 (executor) ─→ Step 3 (cache) ─→ Step 4 (MVP: MD5 + server_user)
                                                                │
                                                      ┌────────┼────────┐
                                                      ↓        ↓        ↓
                                                   Step 5   Step 7   Step 9
                                                   (SCRAM)  (HBA)    (observability)
                                                      ↓
                                                   Step 6
                                                   (passthrough)
                                                      ↓
                                                   Step 8
                                                   (RELOAD + GC)
```

Steps 7 and 9 can be developed in parallel with Steps 5-6.

## Steps

| Step | Title | Key deliverable | Depends on |
|------|-------|----------------|-----------|
| [1](step-1-config.md) | AuthQueryConfig + get_pool_config | Config structs, validation, pool-level access | — |
| [2](step-2-executor.md) | AuthQueryExecutor | deadpool-postgres executor, parameterized query | 1 |
| [3](step-3-cache.md) | AuthQueryCache | DashMap cache, per-username locks, TTL, rate limiting | 2 |
| [4](step-4-md5-server-user.md) | MD5 + server_user mode (MVP) | End-to-end auth_query, shared pool, BDD tests | 1,2,3 |
| [5](step-5-scram.md) | SCRAM support | ClientKey extraction, SCRAM auth via cache | 4 |
| [6](step-6-passthrough.md) | Passthrough mode | Per-user pools, MD5 pass-the-hash, SCRAM passthrough | 5 |
| [7](step-7-hba.md) | HBA integration | Two-phase HBA check, trust mode | 4 |
| [8](step-8-reload-gc.md) | RELOAD + idle GC | Dynamic pool lifecycle, config change detection | 6 |
| [9](step-9-observability.md) | Admin + Prometheus | SHOW AUTH_QUERY_CACHE, metrics | 4 |

## Key files touched

| File | Steps |
|------|-------|
| `src/config/pool.rs` | 1 |
| `src/config/mod.rs` | 1 |
| `src/pool/mod.rs` | 1, 4, 6, 8 |
| `src/auth/auth_query.rs` (new) | 2, 3 |
| `src/auth/mod.rs` | 2, 4, 5, 7 |
| `src/auth/scram.rs` | 5 |
| `src/server/server_backend.rs` | 6 |
| `src/errors.rs` | 2 |
| `src/admin/` | 9 |
| `src/stats/` or prometheus module | 9 |
