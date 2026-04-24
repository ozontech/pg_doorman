# Adaptive Anticipation Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace static 300-500ms anticipation wait with adaptive budget based on xact_p99, so pools grow to adequate size within seconds instead of minutes.

**Architecture:** In `try_anticipate()`, replace `PHASE_4_HARD_CAP_BASE_MS` (500ms) with `max(5ms, min(xact_p99_ms * 2 ± 20% jitter, 500ms))`. Cold start (p99=0) uses 100ms ± 20% default. xact_p99 read from `server_pool.address().stats.get_xact_percentiles()` — one Mutex lock on HDR histogram per anticipation call (not hot path — only reached after warm_threshold when idle=0).

**Tech Stack:** Rust, parking_lot::Mutex (HDR histogram lock), rand for jitter

---

### Task 1: Add `xact_p99_us()` helper to ServerPool

**Files:**
- Modify: `src/pool/server_pool.rs`

- [ ] **Step 1: Add method to ServerPool**

```rust
// In src/pool/server_pool.rs, after `pub fn current_epoch()`:

/// Returns the 99th percentile transaction time in microseconds.
/// Used by anticipation to size the wait budget proportionally to
/// actual backend transaction duration. Returns 0 when the histogram
/// has no data (cold start).
pub fn xact_p99_us(&self) -> u64 {
    let (_, _, _, p99) = self.address.stats.get_xact_percentiles();
    p99
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --lib 2>&1 | grep error` (in test container)
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src/pool/server_pool.rs
git commit -m "Add xact_p99_us() helper to ServerPool for adaptive anticipation"
```

---

### Task 2: Replace static PHASE_4_HARD_CAP with adaptive budget

**Files:**
- Modify: `src/pool/inner.rs:815-820`

- [ ] **Step 1: Replace the static cap calculation**

Current code (lines 816-820):
```rust
const PHASE_4_HARD_CAP_BASE_MS: u64 = 500;
const PHASE_4_HARD_CAP_JITTER_MS: u64 = 200;
let cap_ms = PHASE_4_HARD_CAP_BASE_MS
    - rand::rng().random_range(0..=PHASE_4_HARD_CAP_JITTER_MS);
let effective_budget = total_budget.min(Duration::from_millis(cap_ms));
```

Replace with:
```rust
// Adaptive anticipation budget based on real transaction latency.
// At cold start (p99=0): default 100ms ± 20% — conservative enough
// to not overwhelm coordinator, fast enough to fill the pool.
// At steady state: xact_p99 * 2 ± 20% — wait proportionally to
// how long transactions actually take. If a return doesn't arrive
// within 2x the p99 xact time, it won't arrive soon — create.
const HARD_CAP_MS: u64 = 500;
const COLD_START_DEFAULT_MS: u64 = 100;
const MIN_BUDGET_MS: u64 = 5;

let xact_p99_us = self.inner.server_pool.xact_p99_us();
let base_ms = if xact_p99_us == 0 {
    COLD_START_DEFAULT_MS
} else {
    // xact_p99 is in microseconds, convert to ms and multiply by 2
    (xact_p99_us / 1000).saturating_mul(2)
};

// ±20% jitter to prevent synchronized creates across pools
let jitter_range = (base_ms / 5).max(1);
let jitter = rand::rng().random_range(0..=jitter_range * 2);
let with_jitter = base_ms.saturating_sub(jitter_range) + jitter;

let cap_ms = with_jitter.clamp(MIN_BUDGET_MS, HARD_CAP_MS);
let effective_budget = total_budget.min(Duration::from_millis(cap_ms));
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --lib 2>&1 | grep error` (in test container)
Expected: no errors

- [ ] **Step 3: Run BDD tests**

Run: `make test-bdd TAGS="@client-migration"`
Expected: 16 scenarios passed

- [ ] **Step 4: Run pool-specific BDD tests**

Run: `make test-bdd TAGS="@rust-3"`
Expected: all passed

- [ ] **Step 5: Commit**

```bash
git add src/pool/inner.rs
git commit -m "Adaptive anticipation budget: xact_p99 * 2 instead of static 500ms

Cold start (p99=0): 100ms ± 20% default.
Steady state: xact_p99 * 2 ± 20%, capped at 500ms, min 5ms.

Pools with coordinator previously stabilized at a fraction of
pool_size (e.g. 12/40) because anticipation waited 300-500ms
for returns that came back in <1ms. With adaptive budget at
xact_p99=0.7ms: wait = max(5, 0.7*2) = 5ms. Pool fills to
adequate size in seconds instead of minutes."
```

---

### Task 3: Add logging for adaptive budget (debug level)

**Files:**
- Modify: `src/pool/inner.rs` (inside try_anticipate, after budget calculation)

- [ ] **Step 1: Add debug log**

After `let effective_budget = ...`:
```rust
log::debug!(
    "[{}@{}] anticipation budget: {}ms (xact_p99={}us, base={}ms, cap={}ms)",
    self.inner.pool_name,
    self.inner.username,
    effective_budget.as_millis(),
    xact_p99_us,
    base_ms,
    cap_ms,
);
```

- [ ] **Step 2: Commit**

```bash
git add src/pool/inner.rs
git commit -m "Add debug logging for adaptive anticipation budget"
```

---

### Task 4: Update documentation (EN)

**Files:**
- Modify: `documentation/en/src/tutorials/pool-pressure.md`

- [ ] **Step 1: Find anticipation section and update**

Search for "PHASE_4" or "anticipation" or "300-500ms" in the file. Update the description to reflect adaptive budget:

Add after the anticipation explanation:
```markdown
#### Adaptive anticipation budget

The anticipation wait budget adapts to actual transaction latency:

| Pool state | Budget formula | Example |
|------------|---------------|---------|
| Cold start (no stats yet) | 100ms ± 20% jitter | 80-120ms |
| Steady state | xact_p99 × 2 ± 20% jitter | p99=0.7ms → 5ms (min); p99=50ms → 100ms |
| High latency | Capped at 500ms | p99=300ms → 500ms |

This prevents pools from stabilizing at a fraction of `pool_size` when
the coordinator is enabled. Without adaptive budget, anticipation waits
a fixed 300-500ms for a return even when transactions complete in <1ms,
artificially holding the pool at the warm threshold.
```

- [ ] **Step 2: Commit**

```bash
git add documentation/en/src/tutorials/pool-pressure.md
git commit -m "docs: document adaptive anticipation budget in pool-pressure guide"
```

---

### Task 5: Update documentation (RU)

**Files:**
- Modify: `documentation/ru/pool-pressure.md`

- [ ] **Step 1: Find anticipation section and add equivalent Russian text**

```markdown
#### Адаптивный бюджет anticipation

Бюджет ожидания anticipation подстраивается под реальную latency транзакций:

| Состояние пула | Формула бюджета | Пример |
|---------------|----------------|--------|
| Холодный старт (нет статистики) | 100ms ± 20% jitter | 80-120ms |
| Steady state | xact_p99 × 2 ± 20% jitter | p99=0.7ms → 5ms (min); p99=50ms → 100ms |
| Высокая latency | Ограничено 500ms | p99=300ms → 500ms |

Без адаптивного бюджета anticipation ждёт фиксированные 300-500ms
возврата соединения даже когда транзакции завершаются за <1ms. Это
искусственно удерживает пул на уровне warm_threshold (20% от pool_size)
при включённом coordinator.
```

- [ ] **Step 2: Commit**

```bash
git add documentation/ru/pool-pressure.md
git commit -m "docs(ru): document adaptive anticipation budget"
```

---

### Task 6: Update changelog

**Files:**
- Modify: `documentation/en/src/changelog.md`

- [ ] **Step 1: Add entry under 3.5.2**

Under the "Pool cold start fix" section in 3.5.2:

```markdown
- **Adaptive anticipation budget.** The anticipation wait (formerly a fixed 300-500ms) now scales with `xact_p99 × 2`. At cold start: 100ms default. At steady state with fast transactions (p99=0.7ms): 5ms. Pools reach adequate size in seconds instead of waiting for `pre-replace` to trigger at 95% `server_lifetime`.
```

- [ ] **Step 2: Commit**

```bash
git add documentation/en/src/changelog.md
git commit -m "changelog: add adaptive anticipation budget entry"
```

---

### Task 7: Build and validate on production binary

- [ ] **Step 1: Build Ubuntu 22.04 binary**

```bash
docker build -f /tmp/Dockerfile.ubuntu2204 -t pg_doorman:ubuntu2204-tls /home/vadv/Projects/pg_doorman
docker cp $(docker create pg_doorman:ubuntu2204-tls):/usr/bin/pg_doorman ./pg_doorman
```

- [ ] **Step 2: Push PR**

```bash
git push --force
```

---

## Performance Impact Analysis

**Hot path concern:** `try_anticipate()` is NOT the hot path. It's only called when:
1. `should_anticipate = true` (pool size >= warm_threshold)
2. Fast spin (10 iterations) failed to find an idle connection
3. `capacity_deficit` is false (idle queue empty AND pool is full, or coordinator is present)

The new code adds:
- One `parking_lot::Mutex` lock on HDR histogram (`get_xact_percentiles`) — same lock already taken every 15s by stats collector. Uncontended in practice.
- Integer arithmetic (division, multiplication, clamp) — nanoseconds
- One `random_range` call — already existed for jitter

The TRUE hot path is: `pool.get()` → semaphore acquire → `try_recycle_one()` → return idle connection. Anticipation is only reached when idle queue is empty.
