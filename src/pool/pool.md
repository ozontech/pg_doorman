# Pool Implementation

## Overview

Connection pool manages PostgreSQL connections with:
- Bounded concurrency via semaphore
- Connection reuse through queue (LIFO/FIFO)
- Event-driven anticipation + bounded burst create to avoid thundering-herd

## Connection Acquisition Flow

```
pool.get()
  ↓
1. Acquire semaphore permit (limit concurrent operations)
  ↓
2. Try pop_front() — HOT PATH
   └─ If available → recycle → return (fast!)
  ↓
3. If queue empty → check anticipation zone
   └─ size < warm_threshold (20%) → skip anticipation, go to create
   └─ size >= warm_threshold → enter anticipation zone
  ↓
4. Anticipation zone (Phase A → Phase B):
   - Phase A: 10 fast retries with yield_now (~10-50μs)
   - Phase B: register `idle_returned.notified()` then await with select{
       notified, sleep(min(max_anticipation_wait_ms, wait_timeout/2))
     }
   - On wake → re-try recycle
  ↓
5. Coordinator permit (only if max_db_connections > 0)
   - acquire() may evict idle from peer pools, wait for a return,
     or fall back to a reserve permit
  ↓
6. Bounded burst gate
   - try_take_burst_slot(inflight_creates, max_parallel_creates)
   - If over cap → wait on select{create_done, idle_returned, BURST_BACKOFF}
     and retry recycle, then loop
   - If under cap → take slot (released on RAII guard drop)
  ↓
7. server_pool.create() → install permit + return Object
```

### Why coordinator before burst gate

The two limiters cap different things and must be ordered so the slow one
never holds the fast one. The coordinator can wait up to `wait_timeout` for
a peer pool to return a connection or for an eviction to land. The burst
gate is per-pool and woken in milliseconds by `return_object` or by a peer
create completing.

If the gate were taken first, two callers in one pool could grab the only
two slots, both block on the coordinator for seconds, and the rest of the
pool would starve waiting for those two — head-of-line blocking. With
coordinator first, the gate caps **actual `connect()` calls**, not
**waiting time on a peer pool**.

## Anticipation + Bounded Burst

```rust
ScalingConfig {
    warm_pool_ratio: 0.2,            // 0-20% of max_size: instant creation
    fast_retries: 10,                // yield_now spin retry count
    max_anticipation_wait_ms: 100,   // upper bound on event-driven idle wait
    max_parallel_creates: 2,         // hard cap on concurrent creates per pool
}
```

**Why it exists.** Without bounded burst, N parallel `timeout_get` callers
that all miss the idle pool simultaneously each issue an independent backend
connect, producing thundering-herd bursts (5+ concurrent server connects)
under load. The legacy `cooldown_sleep_ms` was a per-task blind sleep that
neither coordinated waiters nor reacted to returns within the sleep window.

The new mechanism has two layers:

1. **Anticipation wait** (Phase B in the acquisition flow). When the pool is
   above the warm threshold and no idle connection is available, the caller
   waits on a `tokio::sync::Notify` woken by `return_object()`. Exactly one
   waiter is woken per return, which serializes recycle naturally and avoids
   waking all queued tasks at once. The wait is bounded by both the operator
   `max_anticipation_wait_ms` and half of the caller's remaining `wait_timeout`,
   so anticipation never burns the entire client deadline.

2. **Bounded burst gate** (Phase 5). A per-pool `AtomicUsize` caps how many
   `server_pool.create()` calls run concurrently. Excess callers wait on
   either `create_done` (a peer create finished) or `idle_returned` (a peer
   returned an idle connection), with a 5 ms safety-net sleep so progress is
   guaranteed even if both signals are missed. On wake the caller retries
   recycle before retrying the gate.

**Non-blocking checkout** (`wait_timeout = 0`) skips both layers — the
caller wants either an immediate idle hit or a fresh create with no waits.

**Replenish** (the retain-loop background task) defers when the gate is at
capacity rather than queueing — there is no value in client-traffic-driven
creates competing with replenish during a load spike.

## Performance

Microbenchmarks of the new code path (`benches/pool_anticipation_benchmarks.rs`):

| Operation                              | Cost     |
|----------------------------------------|----------|
| `try_take_burst_slot` happy path       | ~3 ns    |
| `try_take_burst_slot` cap rejection    | ~3 ns    |
| Buffered notify (`notify_one` → await) | ~104 ns  |
| 32 tasks contending cap=2 burst gate   | ~27 µs   |

Hot path (idle reuse) is unchanged — the coordinator permit and the burst
gate are only touched on the new-connection path.

## Components

- **Pool** — Cloneable handle (Arc internally)
- **Object** — RAII wrapper, returns connection on drop
- **Slots** — Mutex-protected VecDeque of connections
- **ScalingConfig** — anticipation + bounded burst tuning
- **idle_returned** — Notify woken by `return_object()` for anticipation
- **inflight_creates** — AtomicUsize counter for the bounded burst gate
- **create_done** — Notify woken by completed creates for queued waiters

## Observability

`SHOW POOL_SCALING` admin command and `pg_doorman_pool_scaling*` Prometheus
metrics expose the per-pool counters used to tune the new path:

| Counter | Meaning |
|---|---|
| `inflight_creates` | Gauge: server creates currently in `connect()` |
| `creates_started` | Total creates that took a burst slot |
| `burst_gate_waits` | Total times a caller waited for a slot |
| `anticipation_wakes_notify` | Anticipation woke on a real `idle_returned` |
| `anticipation_wakes_timeout` | Anticipation budget elapsed without a return |
| `create_fallback` | Anticipation finished without avoiding the create |
| `replenish_deferred` | Background replenish skipped due to gate full |

Tuning rules of thumb:
- High `burst_gate_waits` and low `replenish_deferred` → `max_parallel_creates`
  is too low for the offered load.
- High `anticipation_wakes_timeout` and low `anticipation_wakes_notify`
  → `max_anticipation_wait_ms` is too low for query latency, or the pool is
  genuinely under-sized.
- Persistent non-zero `replenish_deferred` → `min_pool_size` cannot be sustained
  under current load; expect new connections to be created on the request path
  rather than prewarmed.

## Queue Modes

- **LIFO** (default): reuse hot connections, better cache locality
- **FIFO**: fair distribution, even wear

## Recycling

Every `pool.get()` recycles the connection:
- Validates connection alive
- Cleans state (rollback transactions, etc.)
- Updates metrics

If recycle fails → connection removed from pool, size decremented.
