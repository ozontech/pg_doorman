/**
 * Operator-facing tooltip strings. Keys map to UI elements identified by
 * "what is this column / tile / row" rather than the raw DTO field name,
 * so a future rename of an internal struct does not break the dictionary.
 *
 * Each string answers three questions an on-call operator asks the first
 * time they see a value: "what is it", "what's normal", "what's bad".
 *
 * The full dictionary with code references lives in tooltip-research.md;
 * the short forms below are what fit into a `title=` attribute without
 * forcing the operator into a side panel.
 */

export const tip = {
  // --- PoolDto ---------------------------------------------------------
  poolId:
    "Stable user@database identifier. Same key the admin protocol and Prometheus labels use.",
  poolMode:
    "transaction = backend returns to pool on commit/rollback. session = backend pinned for the client's lifetime (legacy / LISTEN). statement = returns after every statement (autocommit only).",
  saturation:
    "connections / max_connections. Amber ≥ 70%, red ≥ 90%. At 100% new checkouts queue for query_wait_timeout.",
  connectionsTotal:
    "sv_active + sv_idle + sv_used + sv_login at snapshot time. Includes backends still in the SCRAM/LOGIN phase.",
  connectionsActiveIdle:
    "Server-side split. active = backend executing a query. idle = ready for next checkout. For the client side use Overview tiles.",
  waiting:
    "Clients past the burst gate but no backend yet. 0 is healthy. Sustained > pool_size/4 = scale or coordinator exhaustion blocking.",
  oldestActive:
    "Wall-clock age of the single longest-running checkout. Counts client think-time inside an open BEGIN, not just query runtime. > 30s = stuck transaction.",
  queryP95:
    "95th percentile of query duration over the last 60 s. Amber > 100 ms, red > 500 ms.",
  queryP99: "99th percentile of query duration over the last 60 s.",
  txP95:
    "95th percentile of transaction duration over the last 60 s. > 1 s = either slow queries or client think-time.",
  txP99: "99th percentile of transaction duration over the last 60 s.",
  waitAvg: "Average time a client waited for a backend. > 0 means the pool is queueing.",
  waitP95: "95th percentile of wait time. > 100 ms means most checkouts queue.",
  queriesTotal: "Total queries since pg_doorman started. Counter — not a rate.",
  txTotal: "Total transactions since pg_doorman started. Counter — not a rate.",
  errorsTotal:
    "Total errors with a SQLSTATE since pg_doorman started. See SQLSTATE breakdown for the codes.",
  errorsBySqlstate:
    "Cumulative error count grouped by PostgreSQL SQLSTATE. Click a row in Pools for the full breakdown.",
  paused:
    "yes = pool is rejecting new checkouts (PAUSE / RECONNECT in progress). Existing transactions keep running until commit.",
  epoch:
    "RECONNECT bumps this counter. After running RECONNECT, this should increment for every touched pool — confirms cached backends were invalidated.",
  fallbackActive:
    "yes = the local backend is in cooldown and pg_doorman is routing to a Patroni-discovered fallback host. Database-scoped.",
  tlsHandshakeErrors:
    "Failed TLS handshakes to backends, by database. Sustained growth = cert rotation incident or wrong CA on the server.",
  tlsBackendConnections:
    "Live backend connections currently using TLS, by database. Should equal pool.connections when server_tls_mode is required.",
  // --- PoolCoordinatorRowDto -------------------------------------------
  coordMaxDbConn:
    "Database-level cap shared across every user@db pool. Set by general.max_db_connections; 0 = unlimited.",
  coordCurrent:
    "Backends checked out from this database right now (sum across users). Approaching max_db_conn means the next checkout will hit the coordinator gate.",
  coordReserveSize:
    "Reserve permits held back for high-priority traffic. Sized by reserve_pool_size.",
  coordReserveUsed:
    "Reserve permits in use right now. Non-zero = a low-priority client borrowed from reserve.",
  coordEvictions:
    "Idle backends evicted to make room for a higher-priority checkout. Spikes during burst handovers.",
  coordReserveAcq:
    "Successful reserve acquisitions. Each one means the regular cap was full and a high-priority client used a reserve permit instead.",
  coordExhaustions:
    "Times the coordinator failed to grant any permit (regular OR reserve). > 0 = clients are waiting because the database cap is hit.",
  // --- PoolScalingRowDto -----------------------------------------------
  scalingInflight:
    "Backend connections being established right now. Spikes during anticipation or burst growth; should drop back to 0 within a second.",
  scalingCreates:
    "Total backends created since process start. Steady ramp under load = healthy. Flat while waiting > 0 = create_fallback / gate is throttling.",
  scalingGateWaits:
    "Times a checkout waited at the burst-gate (max_concurrent_creates). Brief spikes are fine; sustained = the per-process create cap is the bottleneck.",
  scalingGateBudgetEx:
    "Burst-gate budget exhaustions. > 0 means the gate is dropping requests rather than queueing — usually a sign of explosive client growth.",
  scalingAnticNotify:
    "Anticipation: pool created a connection ahead of demand based on xact_p99. Healthy churn keeps this incrementing.",
  scalingAnticTimeout:
    "Anticipation request did not produce a backend within the deadline. > 10% of antic_notify = backend is slow to accept TCP.",
  scalingCreateFallback:
    "Times pg_doorman fell back to the synchronous create path. Common during cold-start; sustained > 0 = anticipation cannot keep up.",
  scalingReplenishDef:
    "Times replenishment was deferred (cooldown after a backend failure). > 0 right after an outage; should drain to 0.",
  // --- Overview / Process / Memory -------------------------------------
  rss: "Resident memory of the pg_doorman process. Click for the meminfo-style breakdown (caches, jemalloc, code, swap).",
  cpu: "Aggregate CPU time. 100% = one core saturated; (cpu_cores × 100)% = every core busy.",
  threads:
    "Total OS threads in the pg_doorman process. Click to see per-thread CPU; tokio workers + accept threads + metrics.",
  fdOpen:
    "Open file descriptors. Mostly client and backend sockets. Amber at 70% of the soft cap, red at 90%.",
  fdLimit: "Soft FD cap (RLIMIT_NOFILE). Below 65k → check systemd LimitNOFILE; below 8k = will run out.",
  jemallocAllocated:
    "Bytes the application has actually requested. Smallest of the jemalloc numbers; rises and falls with workload.",
  jemallocActive:
    "Pages jemalloc has handed out to size classes. Slightly above allocated due to internal padding.",
  jemallocResident: "Pages backed by physical RAM. This is what RSS counts.",
  jemallocMapped:
    "Address-space pages reserved by jemalloc (mostly virtual). Includes retained pages still mmaped but not in RAM.",
  jemallocRetained:
    "Pages jemalloc keeps mmaped to avoid syscalls on the next allocation. Reclaimable on memory pressure.",
  jemallocMetadata:
    "Bytes jemalloc itself uses for arena bookkeeping. Should be a tiny fraction of allocated.",
  jemallocFragmentation:
    "resident − allocated. The bigger this gap relative to allocated, the more slabs sit half-empty. > 50% of allocated = consider arena.purge.",
  cgroupCurrent:
    "Memory the cgroup currently accounts to this process. Includes RSS, page cache attributable to us, kernel stacks.",
  cgroupPeak:
    "High-water mark since cgroup creation. > current = there was a transient spike — check binary upgrade or burst load.",
  cgroupMax:
    "Hard memory limit (cgroup v2 memory.max). Hitting it triggers OOM kill on the next allocation.",
  cgroupHigh:
    "Soft throttle threshold (cgroup v2 memory.high). Above this the kernel reclaims aggressively, slowing the process.",
  // --- AuthQueryRowDto -------------------------------------------------
  authCacheEntries:
    "Cached username→password rows. Caps at auth_query_cache_capacity per database.",
  authCacheHits:
    "Auth attempts served from the cache. Goal: > 99% of (hits + misses) once the cache warms.",
  authCacheMisses:
    "Auth attempts that had to query the backend auth table. Each miss counts a real round-trip.",
  authCacheRefetches:
    "Background refreshes of stale entries. Healthy = matches your auth_query_cache_ttl cadence.",
  authCacheRateLimited:
    "Auth attempts blocked by the per-user rate limit. > 0 = a client is bruteforcing or has a stale password.",
  authSuccess: "Successful auth_query lookups since process start.",
  authFailure: "Failed auth_query lookups (wrong password or unknown user). Spikes = credentials issue.",
  authExecQueries: "Real SQL roundtrips to the backend auth table. Should be minuscule vs auth_success.",
  authExecErrors:
    "Backend errors during auth_query (SQL fail, network). Sustained > 0 means auth is degraded for new clients.",
  dynPoolsCurrent: "Live dynamic pools (created on-demand from a wildcard pool config).",
  dynPoolsCreated: "Total dynamic pools created since process start.",
  dynPoolsDestroyed:
    "Total dynamic pools removed (idle GC). created − destroyed should equal current ± in-flight.",
} as const;
