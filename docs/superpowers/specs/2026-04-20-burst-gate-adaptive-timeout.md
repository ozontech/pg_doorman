# Burst Gate Adaptive Timeout

## Problem

`acquire_burst_gate` is an infinite loop with no total timeout. Clients waiting for a connection get recycled connections via a shared FIFO waiter queue and never reach `try_acquire_burst_gate()` at the top of the loop. The pool stays at `warm_threshold` (e.g., 8/40) indefinitely because no client ever proceeds to the create path.

Production evidence: `size=8/40, antic_ok=32(static), antic_to=0, fallback=0, inflight=0` — burst gate semaphore is free but unused. Clients cycle recycled connections for 1-2.5 seconds.

## Solution

Add an adaptive total elapsed budget to the burst gate loop. When a client has been in the loop longer than the budget, stop registering as a waiter for handoff and instead proceed to acquire the burst gate semaphore directly (which leads to coordinator permit acquisition and connection creation).

## Design

### Budget Calculation

```rust
fn burst_gate_budget_ms(xact_p99_us: u64) -> u64 {
    if xact_p99_us == 0 {
        100 // cold start: no stats yet
    } else {
        let base = xact_p99_us.saturating_mul(2) / 1000;
        let jitter_range = (base / 5).max(1);
        let jitter = rand::rng().random_range(0..=jitter_range * 2);
        (base.saturating_sub(jitter_range) + jitter).clamp(5, 500)
    }
}
```

- Base: `xact_p99 * 2` (if a connection hasn't returned in 2x p99, pool is undersized)
- Jitter: +/-20% to prevent synchronized creates
- Min: 5ms (don't bypass handoff for sub-millisecond workloads)
- Max: 500ms (hard cap, same as anticipation)
- Computed once at loop entry (not per iteration)

### Behavior Change in `acquire_burst_gate`

```rust
async fn acquire_burst_gate(&self, timeouts: &Timeouts, non_blocking: bool) -> BurstGateOutcome<'_> {
    let loop_start = tokio::time::Instant::now();
    let budget = Duration::from_millis(self.compute_burst_gate_budget());

    loop {
        if let Some(guard) = self.inner.try_acquire_burst_gate() {
            return BurstGateOutcome::Acquired(guard);
        }

        self.inner.scaling_stats.burst_gate_waits.fetch_add(1, Ordering::Relaxed);

        if non_blocking { /* ... unchanged ... */ }

        if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
            return BurstGateOutcome::Recycled(inner);
        }

        // NEW: budget exceeded — stop waiting for handoff, wait for gate directly
        if loop_start.elapsed() > budget {
            self.inner.scaling_stats.burst_gate_budget_exhausted.fetch_add(1, Ordering::Relaxed);
            match tokio::time::timeout(budget, self.inner.burst_gate_semaphore.acquire()).await {
                Ok(Ok(permit)) => {
                    return BurstGateOutcome::Acquired(BurstGateGuard::from_permit(permit));
                }
                _ => {
                    return BurstGateOutcome::Timeout;
                }
            }
        }

        // existing: register waiter + select!(rx, on_create, sleep(5ms))
        let (tx, rx) = oneshot::channel();
        self.inner.slots.lock().waiters.push_back(tx);
        let on_create = self.inner.create_done.notified();

        tokio::select! {
            result = rx => {
                if let Ok(inner) = result {
                    if let Ok(inner) = self.recycle_handoff(inner, timeouts).await {
                        return BurstGateOutcome::Recycled(Box::new(inner));
                    }
                }
            }
            _ = on_create => {}
            _ = tokio::time::sleep(BURST_BACKOFF) => {}
        }

        if let RecycleOutcome::Reused(inner) = self.inner.try_recycle_one(timeouts).await {
            return BurstGateOutcome::Recycled(inner);
        }
    }
}
```

### Expected Behavior

With production stats (`xact_p99=67ms`):
- Budget = 134ms +/- 27ms (range 107..161ms)
- Client spins in handoff loop for ~134ms
- Exceeds budget → acquires burst gate semaphore (free, inflight=0)
- Proceeds to coordinator → create
- Pool grows from 8 to 9, 10, ... up to max_size or coordinator limit
- 40 clients with jitter spread creates over ~54ms window

### Observability

New counter: `burst_gate_budget_exhausted` — tracks how often the budget fires. Exposed in SHOW POOLS diagnostics and Prometheus.

### Interaction with Existing Components

- **Coordinator**: unchanged. After acquiring burst gate, client still goes through `acquire_coordinator_jit`. Coordinator limits total connections across pools.
- **Anticipation**: unchanged. `try_anticipate` runs before burst gate. The existing `capacity_deficit` check remains as-is.
- **warm_threshold**: unchanged. Controls when `should_anticipate` activates; burst gate timeout handles the case where anticipation was skipped.

### Constraints

- Pure function `burst_gate_budget_ms` must be testable without pool context
- Jitter computed once per loop entry (deterministic within a single checkout attempt)
- No config knob — derived from runtime stats. Constants (min=5ms, max=500ms) as module-level consts.
