# Monitoring the Query Interner

The query interner deduplicates Parse texts in pg_doorman's process
memory. Two halves run different policies: NAMED is bounded by
passive `Arc::strong_count` GC, ANON by per-entry idle TTL
(`query_interner_anon_idle_ttl_seconds`). Both expose Prometheus
gauges, eviction counters, and a sweep duration histogram, plus a
counter for the synthetic SQLSTATE 26000 returned to clients whose
anonymous prepared statement is no longer in any cache.

This page is the operator companion to those metrics: dashboard
recipe, alert rules, and tuning guidance.

## Dashboard

### Above-the-fold (top three panels)

1. **Stat — interner total bytes.**
   `sum(pg_doorman_query_interner_bytes)` per instance, with red
   threshold at 1.5 GiB and yellow at 500 MiB. Drives most
   memory-related decisions.
2. **Time series — entries by kind.** Two lines:
   - `pg_doorman_query_interner_entries{kind="named"}`
   - `pg_doorman_query_interner_entries{kind="anonymous"}`
   Six-hour window. Sustained growth on either line is the cue to
   open the drill-down panels.
3. **Time series — synthetic 26000 rate.**
   `rate(pg_doorman_query_interner_synthetic_misses_total[5m])`.
   Flat zero is the normal case; any spike means TTL trimmed
   something a client referenced or the driver depended on
   cross-batch unnamed.

### Drill-down

4. Eviction rate, stacked by reason:
   `sum by (kind, reason) (rate(pg_doorman_query_interner_evictions_total[5m]))`
5. GC sweep duration heatmap:
   `histogram_quantile(0.5, rate(pg_doorman_query_interner_gc_duration_seconds_bucket[5m]))`,
   with a P99 line on top.
6. Average bytes per entry:
   `pg_doorman_query_interner_bytes / pg_doorman_query_interner_entries`,
   per kind.

### Correlations

7. Anon eviction rate vs total query rate. Linear correlation =
   normal traffic; non-linear = ORM dynamic-SQL explosion.
8. Synthetic 26000 rate vs P99 query latency. Correlation = TTL is
   killing real traffic; investigate the slow path.

### Recommended dashboard variables

- `instance` — to compare replicas.
- `kind` — to slice gauges and counters down to one half at a time.

Pool, user, and database labels do not apply to the interner — it
is process-global. Adding those labels to interner panels would
mislead readers.

## Alert rules

A complete `groups:` block is shipped at
`monitoring/prometheus-rules/query-interner.yaml`. The five
alerts:

- **`PgDoormanAnonInternerMemoryHigh`** (critical) — ANON bytes
  > 1.5 GiB. Tighten TTL or check for ORM dynamic SQL.
- **`PgDoormanAnonTTLTooShort`** (critical) — synthetic 26000 rate
  > 1/s for 10 min. Raise TTL or fix the offending driver.
- **`PgDoormanAnonInternerNotShrinking`** (warning) — ANON keeps
  growing while TTL evictions are flat. Either TTL is set too long
  or the workload is pushing unique queries faster than they expire.
- **`PgDoormanInternerGCSlow`** (warning) — GC sweep P99 > 50 ms
  for 15 min. Lengthen `query_interner_gc_interval_seconds` (this
  knob is **restart-only**; reload won't change the running sweep
  cadence) or shrink the interner via `RESET INTERNER` plus
  cache-size tuning.
- **`PgDoormanNamedInternerGrowsUnbounded`** (warning) — NAMED
  entries above 100k with near-zero eviction rate. Almost always a
  code bug holding `Arc<str>` strong refs forever.

Cold-start guard: every alert above uses `for: > 5m`, so the empty
interner immediately after process start does not trip them.

## Sizing

Steady-state ANON interner footprint, assuming 50% of queries take the
prepared path and the average SQL text is 2 KiB:

| RPS    | TTL = 60s          | TTL = 300s          |
|--------|--------------------|---------------------|
| 100    | ~12k entries / ~24 MiB | ~60k / ~120 MiB |
| 1 000  | ~120k / ~240 MiB   | ~600k / ~1.2 GiB    |
| 10 000 | ~1.2M / ~2.4 GiB   | refuse to size      |

The interner is process-global, so the cluster-wide footprint scales
linearly with the number of pg_doorman replicas. Use this as the
starting estimate for `query_interner_anon_idle_ttl_seconds` and the
RAM budget per host; the live `pg_doorman_query_interner_bytes`
gauge is authoritative.

## Effective TTL

The eviction policy is two-cycle mark-and-sweep over a sweep that
ticks at `gc_interval / 4`. With the defaults
(`gc_interval = 60 s`, `anon_idle_ttl = 60 s`) the sweep runs every
15 s, so an entry is marked between 60 s and 75 s after it last got
touched, and removed on the next sweep that still sees it as a
candidate — i.e. between 75 s and 120 s of total idle time. A shorter
TTL than the 60 s default does not buy you sub-15-second eviction:
`gc_interval` controls the sweep cadence.

## Tuning recipes

### Reduce TTL when memory pressure dominates

Trigger: `PgDoormanAnonInternerNotShrinking` fires, ANON bytes
approaches the budget for the host.

Action: drop `query_interner_anon_idle_ttl_seconds` in `general`
config (e.g. 60 → 30). Reload pg_doorman. Watch the eviction rate
catch up to the new threshold.

### Raise TTL when synthetic 26000 fires

Trigger: `PgDoormanAnonTTLTooShort` fires.

Action: identify which client and what query — the synthetic-miss
counter has no labels, so use the WARN log line emitted with each
miss for client / pool / connection_id context. If the offender is
a driver that legitimately reuses unnamed Bind across batches,
raise TTL to cover the gap (e.g. 60 → 300). If it is not, switch
that client to named prepared.

### Run RESET INTERNER

Trigger: ad-hoc diagnostics or memory containment incident.

Action: `psql admin@:6432 -c "RESET INTERNER"`. Returns
`CommandComplete RESET`. In-flight clients re-Parse on next reuse;
short-lived ones see no effect because their `last_anonymous_hash`
remembers the hash they registered before the reset, and the next
Bind discovers the missing entry and emits 26000 once before the
client driver re-issues Parse.

## Recording rules

Cluster-wide aggregates worth pre-computing for cheaper dashboards:

```yaml
groups:
  - name: pg_doorman_query_interner_recording
    interval: 30s
    rules:
      - record: pg_doorman:query_interner_total_bytes:5m
        expr: sum without (instance) (pg_doorman_query_interner_bytes)
      - record: pg_doorman:query_interner_eviction_rate:5m
        expr: |
          sum without (instance) (rate(pg_doorman_query_interner_evictions_total[5m]))
```

The first lets the cluster-wide stat panel scrape one series; the
second drives the eviction-rate-by-reason panel without re-running
`rate()` on every dashboard load.
