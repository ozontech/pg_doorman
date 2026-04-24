# Burst Gate Adaptive Timeout — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent clients from being stuck indefinitely in the burst gate loop by adding an adaptive timeout based on xact p99 stats.

**Architecture:** Single function change in `acquire_burst_gate` + new counter in ScalingStats. Budget reuses existing `anticipation_base_ms` with jitter, computed once at loop entry.

**Tech Stack:** Rust, tokio, rand

---

### Task 1: Add `burst_gate_budget_exhausted` counter to ScalingStats

**Files:**
- Modify: `src/pool/inner.rs:127-163` (ScalingStats struct)
- Modify: `src/pool/inner.rs:165-178` (ScalingStatsSnapshot struct)
- Modify: `src/pool/inner.rs:1459-1471` (scaling_stats() method)

- [ ] **Step 1: Add field to ScalingStats**

In `ScalingStats` struct (after `replenish_deferred` field, line ~157):

```rust
    pub(crate) burst_gate_budget_exhausted: AtomicU64,
```

- [ ] **Step 2: Add field to ScalingStatsSnapshot**

In `ScalingStatsSnapshot` struct (after `burst_gate_waits` field, line ~169):

```rust
    pub burst_gate_budget_exhausted: u64,
```

- [ ] **Step 3: Wire into scaling_stats() snapshot method**

In `scaling_stats()` method (after `burst_gate_waits` load, line ~1463):

```rust
            burst_gate_budget_exhausted: s.burst_gate_budget_exhausted.load(Ordering::Relaxed),
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build 2>&1 | head -30`
Expected: compile errors in admin/show.rs and prometheus because the new field isn't handled yet. That's fine — we'll fix in Task 4.

---

### Task 2: Add `BURST_GATE_MIN_BUDGET_MS` constant and budget function

**Files:**
- Modify: `src/pool/inner.rs:236-254` (constants section)

- [ ] **Step 1: Add constant**

After `ANTICIPATION_FALLBACK_BUDGET_MS` (line 253), add:

```rust

/// Burst gate adaptive timeout: minimum budget before exiting the handoff loop.
/// DBA recommendation: below 20ms, fork() + shared_buffers attach on large instances
/// can take longer, causing unnecessary creates during brief spikes.
const BURST_GATE_MIN_BUDGET_MS: u64 = 20;
```

- [ ] **Step 2: Add pure budget function**

After `anticipation_base_ms` function (line 264), add:

```rust
/// Compute burst gate adaptive budget (ms) from xact_p99.
/// Reuses anticipation_base_ms for the base, adds ±20% jitter.
/// Pure function except for the random jitter.
#[inline]
fn burst_gate_budget(xact_p99_us: u64) -> Duration {
    let base_ms = anticipation_base_ms(xact_p99_us);
    let jitter_range = (base_ms / 5).max(1);
    let jitter = rand::rng().random_range(0..=jitter_range * 2);
    let budget_ms = (base_ms.saturating_sub(jitter_range) + jitter)
        .clamp(BURST_GATE_MIN_BUDGET_MS, ANTICIPATION_HARD_CAP_MS);
    Duration::from_millis(budget_ms)
}
```

- [ ] **Step 3: Add unit test**

In the `#[cfg(test)]` module at the bottom of the file:

```rust
    #[test]
    fn burst_gate_budget_cold_start() {
        // No stats: base = 100ms cold start. With jitter ±20ms, clamped to [20, 500].
        let budget = burst_gate_budget(0);
        assert!(budget.as_millis() >= 20);
        assert!(budget.as_millis() <= 500);
    }

    #[test]
    fn burst_gate_budget_normal_workload() {
        // xact_p99 = 67ms (67000us). base = 67000*2/1000 = 134ms.
        // jitter ±27ms -> range 107..161, clamped to [20, 500].
        let budget = burst_gate_budget(67_000);
        assert!(budget.as_millis() >= 20);
        assert!(budget.as_millis() <= 500);
    }

    #[test]
    fn burst_gate_budget_fast_workload() {
        // xact_p99 = 700us. base = 700*2/1000 = 1ms.
        // Below min → clamped to 20ms.
        let budget = burst_gate_budget(700);
        assert_eq!(budget.as_millis(), 20);
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p pg_doorman burst_gate_budget`
Expected: all 3 tests pass.

---

### Task 3: Modify `acquire_burst_gate` to use adaptive timeout

**Files:**
- Modify: `src/pool/inner.rs:606-656` (acquire_burst_gate function)

- [ ] **Step 1: Add loop_start and budget at function entry**

After line 610 (`non_blocking: bool,`) `-> BurstGateOutcome<'_> {`, before the `loop {`:

```rust
        let (_, _, _, xact_p99_us) =
            self.inner.server_pool.address().stats.get_xact_percentiles();
        let budget = burst_gate_budget(xact_p99_us);
        let loop_start = tokio::time::Instant::now();
```

- [ ] **Step 2: Add budget check before waiter registration**

After the `try_recycle_one` at line 630-632, BEFORE the "Register a direct-handoff waiter" comment (line 634), insert:

```rust
            // Adaptive timeout: if we've been in this loop longer than the budget,
            // stop waiting for handoff and wait for the burst gate directly.
            if loop_start.elapsed() > budget {
                self.inner
                    .scaling_stats
                    .burst_gate_budget_exhausted
                    .fetch_add(1, Ordering::Relaxed);
                // Wait for gate with a short bounded timeout. Gate is typically free
                // (inflight=0 in steady state when pool is undersized).
                let deadline = Duration::from_millis(50);
                let gate_wait = self.inner.create_done.notified();
                tokio::time::timeout(deadline, gate_wait).await.ok();
                // Re-check gate at top of loop (or timeout → BurstGateOutcome::Timeout)
                if self.inner.try_acquire_burst_gate().is_some() {
                    // Can't return guard from is_some — redo at top
                }
                continue;
            }
```

Wait — this is wrong. Let me reconsider. After budget exhaustion, we want to ACQUIRE the gate. The gate is typically free (inflight=0). The simplest approach:

Replace the insert above with:

```rust
            if loop_start.elapsed() > budget {
                self.inner
                    .scaling_stats
                    .burst_gate_budget_exhausted
                    .fetch_add(1, Ordering::Relaxed);
                // Budget exhausted. Stop accepting recycled connections.
                // Loop back to top where try_acquire_burst_gate will succeed
                // if gate is free (inflight=0 in undersized steady state).
                // If gate is busy, wait for create_done with bounded timeout.
                let notify = self.inner.create_done.notified();
                let _ = tokio::time::timeout(Duration::from_millis(50), notify).await;
                continue;
            }
```

This `continue` goes back to the top of the loop → `try_acquire_burst_gate()`. If gate is free → Acquired → exit. If busy → the 50ms wait + `continue` retries. After a few iterations, gate frees up.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build 2>&1 | head -30`
Expected: success (or only admin/prometheus errors from Task 1 field addition).

---

### Task 4: Wire counter into admin SHOW POOLS and Prometheus

**Files:**
- Modify: `src/admin/show.rs` (find where ScalingStatsSnapshot fields are used)
- Modify: `src/prometheus/metrics.rs` or `src/prometheus/mod.rs`

- [ ] **Step 1: Find SHOW POOLS usage**

Run: `grep -n "burst_gate_waits\|scaling_stats" src/admin/show.rs | head -10`

Add `burst_gate_budget_exhausted` to the same output format, next to `burst_gate_waits`.

- [ ] **Step 2: Find Prometheus usage**

Run: `grep -n "burst_gate_waits\|scaling_stats" src/prometheus/metrics.rs src/prometheus/mod.rs | head -10`

Add `pgdoorman_burst_gate_budget_exhausted_total` counter, following the same pattern as `burst_gate_waits`.

- [ ] **Step 3: Build clean**

Run: `cargo build`
Expected: success, no warnings.

---

### Task 5: Add to slow checkout diagnostics

**Files:**
- Modify: `src/client/transaction.rs` (where slow checkout is logged)

- [ ] **Step 1: Find slow checkout log format**

Run: `grep -n "slow checkout" src/client/transaction.rs | head -5`

Add `bg_timeout={}` to the format string, sourcing from the scaling stats snapshot (same place where `gate_waits` is already logged).

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: success.

---

### Task 6: Full test suite + clippy

- [ ] **Step 1: cargo fmt**

Run: `cargo fmt`

- [ ] **Step 2: cargo clippy**

Run: `cargo clippy -- --deny "warnings"`
Expected: no warnings.

- [ ] **Step 3: Run all unit tests**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/pool/inner.rs src/admin/show.rs src/prometheus/metrics.rs src/prometheus/mod.rs src/client/transaction.rs
git commit -m "Burst gate adaptive timeout based on xact p99

What was needed: burst gate loop had no total timeout — clients could spin
indefinitely getting recycled connections, preventing pool growth above
warm_threshold.

What changed: added adaptive budget (2x xact_p99 ±20% jitter, min 20ms,
max 500ms) to the burst gate loop. When budget is exhausted, client stops
registering as a handoff waiter and proceeds to acquire the burst gate
semaphore for a new connection create. New counter burst_gate_budget_exhausted
tracks how often the budget fires."
```
