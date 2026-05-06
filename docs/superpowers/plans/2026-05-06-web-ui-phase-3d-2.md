# Web UI — Phase 3d-2 Implementation Plan: /api/top/queries with backend instrumentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Поднять `GET /api/top/queries?by=count|duration&n=20` с backend instrumentation: добавить per-interner-entry счётчик parse'ов/bind'ов и аккумулятор общей длительности, чтобы Top-N работал. Это первая фаза которая трогает SQL hot path — accuracy сознательно жертвуется ради zero-contention (single Relaxed fetch_add ≈ 2 ns).

**Architecture:**
- `NamedEntry` / `AnonEntry` (`src/server/prepared_statement_cache.rs`) получают `count: AtomicU64` и `total_duration_us: AtomicU64`.
- Public API: `record_query_count(hash, is_anonymous)` и `record_query_duration_us(hash, is_anonymous, micros)`. Каждая делает `DashMap.get(hash)` + `fetch_add(_, Relaxed)`. Если entry GC'нут или ещё не создан — silently no-op.
- Hot path hook на Bind (`src/client/protocol.rs:421+`): после успешного `cached.hash` lookup → `record_query_count(cached.hash, anon)` + сохранить `(hash, anon)` в `PreparedStatementState.last_bound_for_top: Option<(u64, bool)>`.
- Hot path hook на Sync (`src/client/transaction.rs:488` область): если `last_bound_for_top` Some → `record_query_duration_us(hash, anon, micros)` + clear. Cleared at start of next batch.
- /api/top/queries: read named_snapshot+anon_snapshot, build rows `{hash, kind, query, count, total_duration_us, avg_duration_ms}`, sort by chosen `by`, take n.

**Imprecision contract (spec section 8.2 + decision #21):**
- Count = number of Bind events per interner entry. Не Parse, не Execute. Operationally close to «executions» для типичного 1-Bind-per-Sync.
- Duration на Sync аккумулируется ЦЕЛИКОМ на `last_bound_for_top` hash. Multi-Bind batches с разными hash'ами теряют точность (последний hash в batch'е получает всю длительность). Это сознательный trade-off: точная attribution требовала бы поддерживать список (hash, partial_duration) per batch — много кода в hot path.
- Simple queries (без Parse/Bind) не учитываются — они не используют interner.
- Hash не имеет pool-affinity. `?pool=` filter в spec не поддерживается этой фазой; для per-pool triage operators используют `/api/top/clients?pool=`.

**Hot path budget:**
- На Bind: 1 DashMap.get + 1 fetch_add + 1 Option<(u64, bool)> store. ≈ 30-50 ns.
- На Sync: 1 Option load + (если Some) 1 DashMap.get + 1 fetch_add. ≈ 30-50 ns.
- Per query (1 Bind + 1 Sync): ~60-100 ns. На 5kTPS: 0.5 ms/sec = 0.05% CPU.

**Reference:**
- Spec section 8.2.
- Phase 3d-1 commit: `6adf610`.
- Hot path hooks: `src/client/protocol.rs:421` (Bind), `src/client/transaction.rs:488` (Sync, end-of-batch).

**Не входит в фазу 3d-2:**
- `/api/top/prepared` — phase 3d-3.
- `/api/events` — phase 3d-4.
- Pool filter на /api/top/queries — out of scope.

---

## File Structure

**Модифицируемые:**
- `src/server/prepared_statement_cache.rs` — добавить count/total_duration_us в Entry'и + record_* функции.
- `src/client/core.rs` — добавить `last_bound_for_top: Option<(u64, bool)>` в PreparedStatementState.
- `src/client/protocol.rs` — Bind handler hook.
- `src/client/transaction.rs` — Sync handler hook (clear на старте + record на завершении).
- `src/web/routes/dto.rs` — новые DTO.
- `src/web/routes/collect.rs` — collect_top_queries.
- `src/web/routes/mod.rs` — register top_queries module.
- `src/web/server.rs` — arm + dispatch test.
- `src/web/tests.rs` — integration test.

**Новые:**
- `src/web/routes/top_queries.rs` — handler.

---

## Task 0: Baseline

- [ ] **Step 0.1**

```bash
cd /home/vadv/Projects/pg_doorman
git status && git log --oneline -3
```
Expected: HEAD = `6adf610`.

- [ ] **Step 0.2**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 793 passed.

---

## Task 1: backend instrumentation в interner entries

**Files:** Modify `src/server/prepared_statement_cache.rs`.

- [ ] **Step 1.1: Добавить counters в `NamedEntry`**

Линия ~41:

```rust
pub struct NamedEntry {
    text: Arc<str>,
    gc_state: AtomicU8,
    /// Cumulative count of Bind events that referenced this hash.
    /// Used by `/api/top/queries?by=count`. Approximate: see plan.
    count: AtomicU64,
    /// Cumulative microseconds spent across all Sync's that ended a batch
    /// whose last Bind referenced this hash. Approximate per-batch
    /// attribution — multi-Bind batches give the entire duration to the
    /// last hash. See plan for the trade-off.
    total_duration_us: AtomicU64,
}

impl NamedEntry {
    fn new(text: Arc<str>) -> Self {
        Self {
            text,
            gc_state: AtomicU8::new(GC_STATE_ACTIVE),
            count: AtomicU64::new(0),
            total_duration_us: AtomicU64::new(0),
        }
    }

    fn touch(&self) { self.gc_state.store(GC_STATE_ACTIVE, Ordering::Relaxed); }
    pub fn text(&self) -> &Arc<str> { &self.text }

    /// Approximate count of Bind references. Used by `/api/top/queries`.
    pub fn count(&self) -> u64 { self.count.load(Ordering::Relaxed) }
    /// Approximate cumulative execution time in microseconds.
    pub fn total_duration_us(&self) -> u64 { self.total_duration_us.load(Ordering::Relaxed) }
}
```

- [ ] **Step 1.2: Добавить counters в `AnonEntry`**

Аналогично. Существующее поле `last_used` сохраняется.

```rust
pub struct AnonEntry {
    text: Arc<str>,
    last_used: AtomicU64,
    gc_state: AtomicU8,
    count: AtomicU64,
    total_duration_us: AtomicU64,
}

impl AnonEntry {
    fn new(text: Arc<str>, now_ms: u64) -> Self {
        Self {
            text,
            last_used: AtomicU64::new(now_ms),
            gc_state: AtomicU8::new(GC_STATE_ACTIVE),
            count: AtomicU64::new(0),
            total_duration_us: AtomicU64::new(0),
        }
    }

    fn touch(&self, now_ms: u64) { /* unchanged */ }
    pub fn text(&self) -> &Arc<str> { &self.text }
    pub fn idle_ms(&self, now_ms: u64) -> u64 { /* unchanged */ }

    pub fn count(&self) -> u64 { self.count.load(Ordering::Relaxed) }
    pub fn total_duration_us(&self) -> u64 { self.total_duration_us.load(Ordering::Relaxed) }

    #[cfg(test)]
    pub fn last_used_for_test(&self) -> u64 { /* unchanged */ }
}
```

- [ ] **Step 1.3: Public API `record_query_count` и `record_query_duration_us`**

В конце файла (после `now_monotonic_ms`):

```rust
/// Increments the Bind-count atomic on the interner entry that owns `hash`.
/// No-op if the entry has been GC'd or not yet inserted; we accept the
/// resulting count gap to keep the hot path lock-free.
pub fn record_query_count(hash: u64, is_anonymous: bool) {
    if is_anonymous {
        if let Some(entry) = ANON_INTERNER.get(&hash) {
            entry.count.fetch_add(1, Ordering::Relaxed);
        }
    } else if let Some(entry) = NAMED_INTERNER.get(&hash) {
        entry.count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Adds `micros` to the cumulative duration on the interner entry. Same
/// no-op-on-miss policy as `record_query_count`.
pub fn record_query_duration_us(hash: u64, is_anonymous: bool, micros: u64) {
    if is_anonymous {
        if let Some(entry) = ANON_INTERNER.get(&hash) {
            entry.total_duration_us.fetch_add(micros, Ordering::Relaxed);
        }
    } else if let Some(entry) = NAMED_INTERNER.get(&hash) {
        entry.total_duration_us.fetch_add(micros, Ordering::Relaxed);
    }
}
```

- [ ] **Step 1.4: Unit-тест на public API**

В `#[cfg(test)] mod tests` добавить:

```rust
    #[test]
    fn record_query_count_increments_named_entry() {
        let _ = intern_query("select 100", 0xC0FFEE, false);
        super::record_query_count(0xC0FFEE, false);
        super::record_query_count(0xC0FFEE, false);
        let snap = super::named_snapshot();
        let (_, e) = snap.iter().find(|(h, _)| *h == 0xC0FFEE).unwrap();
        assert!(e.count() >= 2);
    }

    #[test]
    fn record_query_count_no_op_on_unknown_hash() {
        // Intentionally use a hash that is not interned — must not panic.
        super::record_query_count(0xDEADC0DE, false);
        super::record_query_count(0xDEADC0DE, true);
    }

    #[test]
    fn record_query_duration_us_accumulates() {
        let _ = intern_query("select 200", 0xD00D00, false);
        super::record_query_duration_us(0xD00D00, false, 100);
        super::record_query_duration_us(0xD00D00, false, 250);
        let snap = super::named_snapshot();
        let (_, e) = snap.iter().find(|(h, _)| *h == 0xD00D00).unwrap();
        assert_eq!(e.total_duration_us(), 350);
    }
```

- [ ] **Step 1.5: cargo build + test**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib server::prepared_statement_cache 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~13 prepared_statement_cache tests passing, clean.

- [ ] **Step 1.6: Не коммитить.**

---

## Task 2: hot path hooks (Bind + Sync)

**Files:**
- Modify: `src/client/core.rs`, `src/client/protocol.rs`, `src/client/transaction.rs`.

- [ ] **Step 2.1: Поле в `PreparedStatementState`**

В `src/client/core.rs:377+` добавить поле:

```rust
pub struct PreparedStatementState {
    pub enabled: bool,
    pub async_client: bool,
    pub cache: PreparedStatementCache,
    pub last_anonymous_hash: Option<u64>,
    /// Hash of the last Bind in the current batch, plus the anonymous flag.
    /// Cleared on Sync completion. Used by /api/top/queries duration
    /// instrumentation to attribute the batch's elapsed time to a single
    /// interner entry.
    pub last_bound_for_top: Option<(u64, bool)>,
    pub skipped_parses: Vec<SkippedParse>,
    // ... existing fields
}
```

В constructor (поищи `PreparedStatementState::new` или impl Default) добавить `last_bound_for_top: None`.

- [ ] **Step 2.2: Bind hook**

В `src/client/protocol.rs:461` (внутри `Some(cached) =>` блока, после `self.prepared.batch_operations.push(BatchOperation::Bind { statement_name: server_name });`) добавить:

```rust
                // /api/top/queries instrumentation. Accept the cache miss /
                // race where the interner entry has been GC'd between intern
                // and Bind — the no-op behaviour in record_query_count is
                // intended to keep the hot path lock-free.
                let is_anonymous = client_given_name.is_empty();
                crate::server::record_query_count(cached.hash, is_anonymous);
                self.prepared.last_bound_for_top = Some((cached.hash, is_anonymous));
```

- [ ] **Step 2.3: Sync hook (record duration)**

В `src/client/transaction.rs:488` (область, где `server.stats.query(query_start_at.elapsed().as_micros() as u64, ...)` ставится) добавить ПЕРЕД этим вызовом:

```rust
        // /api/top/queries duration accounting. The whole batch's elapsed
        // time is attributed to the last Bind's hash; multi-Bind batches
        // give the duration to whichever Bind was last (approximation).
        let micros = query_start_at.elapsed().as_micros() as u64;
        if let Some((hash, anon)) = self.prepared.last_bound_for_top.take() {
            crate::server::record_query_duration_us(hash, anon, micros);
        }
        server.stats.query(
            micros,
            self.server_parameters.get_application_name(),
        );
```

(Заметь: `micros` теперь вычисляется один раз и переиспользуется. Старый код вычислял `query_start_at.elapsed().as_micros() as u64` inline в вызове `server.stats.query`.)

- [ ] **Step 2.4: Аналогично в handle_simple_query**

Симпл queries не имеют Bind, но мы должны убедиться что `last_bound_for_top` не остался stale из прошлого extended batch'а. На входе simple query handler:

```rust
        // Defensively clear any pending extended-protocol attribution.
        // A simple query is opaque to the interner; whatever last_bound_for_top
        // held was from a prior extended batch and would otherwise leak its
        // hash into the next Sync.
        self.prepared.last_bound_for_top = None;
```

(Помещается в начале handle_simple_query, после set_async_mode/set_expected_responses.)

- [ ] **Step 2.5: cargo build + test**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 793+3 passed (3 new from Task 1.4), clean.

- [ ] **Step 2.6: Не коммитить.**

---

## Task 3: DTOs

**Files:** Modify `src/web/routes/dto.rs`.

- [ ] **Step 3.1: TopQueries**

```rust
/// `GET /api/top/queries` — Top-N interner-tracked queries by count or
/// average duration. See plan for accuracy notes (Bind-counted, batch-
/// level duration attribution).
#[derive(Debug, Serialize)]
pub struct TopQueriesDto {
    pub ts: u64,
    pub by: String,
    pub n: u64,
    pub queries: Vec<TopQueryRowDto>,
}

#[derive(Debug, Serialize)]
pub struct TopQueryRowDto {
    /// `0x<hex>` form of the FxHash, matching `/api/interner/top`.
    pub hash: String,
    /// `"named"` or `"anonymous"`.
    pub kind: String,
    /// First 120 characters of the interned text (UTF-8 safe).
    pub query: String,
    pub count: u64,
    pub total_duration_us: u64,
    /// Average duration in milliseconds: `total_duration_us / count / 1000`.
    /// Returns `0.0` when count is 0 (entry interned but never Bound).
    pub avg_duration_ms: f64,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum TopQueryBy {
    #[default]
    Count,
    Duration,
}

impl TopQueryBy {
    pub fn as_str(self) -> &'static str {
        match self {
            TopQueryBy::Count => "count",
            TopQueryBy::Duration => "duration",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TopQueryFilters {
    pub by: TopQueryBy,
    pub n: u64,
}
```

- [ ] **Step 3.2: cargo build + clippy + fmt**

Expected: clean.

- [ ] **Step 3.3: Не коммитить.**

---

## Task 4: collect_top_queries

**Files:** Modify `src/web/routes/collect.rs`.

- [ ] **Step 4.1: Расширить use-импорты + collect_top_queries**

```rust
use crate::web::routes::dto::{
    TopQueriesDto, TopQueryBy, TopQueryFilters, TopQueryRowDto,
    // existing entries
};

pub fn collect_top_queries(filters: &TopQueryFilters) -> TopQueriesDto {
    use crate::server::{anon_snapshot, named_snapshot};

    let n = clamp_top_clients_n(filters.n); // reuse same default 20, max 200

    let mut rows: Vec<TopQueryRowDto> = Vec::new();

    for (hash, entry) in named_snapshot() {
        let count = entry.count();
        let total_duration_us = entry.total_duration_us();
        let avg_duration_ms = if count == 0 {
            0.0
        } else {
            total_duration_us as f64 / count as f64 / 1_000.0
        };
        let preview: String = entry.text().chars().take(120).collect();
        rows.push(TopQueryRowDto {
            hash: format!("{:#x}", hash),
            kind: "named".to_string(),
            query: preview,
            count,
            total_duration_us,
            avg_duration_ms,
        });
    }
    for (hash, entry) in anon_snapshot() {
        let count = entry.count();
        let total_duration_us = entry.total_duration_us();
        let avg_duration_ms = if count == 0 {
            0.0
        } else {
            total_duration_us as f64 / count as f64 / 1_000.0
        };
        let preview: String = entry.text().chars().take(120).collect();
        rows.push(TopQueryRowDto {
            hash: format!("{:#x}", hash),
            kind: "anonymous".to_string(),
            query: preview,
            count,
            total_duration_us,
            avg_duration_ms,
        });
    }

    rows.sort_by(|a, b| match filters.by {
        TopQueryBy::Count => b.count.cmp(&a.count),
        TopQueryBy::Duration => b
            .avg_duration_ms
            .partial_cmp(&a.avg_duration_ms)
            .unwrap_or(std::cmp::Ordering::Equal),
    });

    let queries: Vec<_> = rows.into_iter().take(n as usize).collect();

    TopQueriesDto {
        ts: now_unix_ms(),
        by: filters.by.as_str().to_string(),
        n,
        queries,
    }
}
```

- [ ] **Step 4.2: cargo build + test + clippy + fmt**

Expected: clean.

- [ ] **Step 4.3: Не коммитить.**

---

## Task 5: handler + mux + tests

**Files:**
- Create: `src/web/routes/top_queries.rs`.
- Modify: `src/web/routes/mod.rs`, `src/web/server.rs`, `src/web/tests.rs`.

- [ ] **Step 5.1: top_queries.rs**

```rust
//! GET /api/top/queries?by=count|duration&n=20 handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_top_queries;
use crate::web::routes::dto::{TopQueryBy, TopQueryFilters};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_top_queries(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = TopQueryFilters {
        by: match first(query, "by").as_deref() {
            Some("duration") => TopQueryBy::Duration,
            _ => TopQueryBy::Count,
        },
        n: parse_u64(query, "n", 0),
    };
    Response::ok_json(&collect_top_queries(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_queries_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_top_queries(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"count\""));
        assert!(body.contains("\"n\":20"));
        assert!(body.contains("\"queries\""));
    }

    #[test]
    fn top_queries_by_duration_param() {
        let mut q = BTreeMap::new();
        q.insert("by".into(), vec!["duration".into()]);
        let r = handle_top_queries(&q);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"duration\""));
    }
}
```

- [ ] **Step 5.2: mod.rs**

```rust
pub(crate) mod top_queries;
```

- [ ] **Step 5.3: route_api arm + dispatch test**

Добавить:

```rust
        "/api/top/queries" => routes::top_queries::handle_top_queries(&query),
```

И dispatch-тест:

```rust
    #[test]
    fn dispatch_top_queries_returns_200() {
        let r = dispatch(
            &req("GET", "/api/top/queries"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }
```

- [ ] **Step 5.4: Integration test**

В `src/web/tests.rs`:

```rust
#[tokio::test]
async fn api_top_queries_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/top/queries HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"by\":\"count\""), "raw={raw}");
    assert!(raw.contains("\"queries\""), "raw={raw}");
}
```

- [ ] **Step 5.5: Полный прогон**

```bash
cargo test --lib 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~800 passed (793 + 3 prepared cache + 2 handler + 1 dispatch + 1 integration). Точное число при прогоне.

- [ ] **Step 5.6: Не коммитить.**

---

## Task 6: smoke + commit

- [ ] **Step 6.1: Release smoke**

```bash
cargo build --release 2>&1 | tail -3
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3d2.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/top/queries default ---"
curl -s 'http://127.0.0.1:19127/api/top/queries' | head -c 400
echo ""
echo "--- /api/top/queries?by=duration ---"
curl -s 'http://127.0.0.1:19127/api/top/queries?by=duration&n=5' | head -c 400
echo ""
kill $DPID 2>/dev/null
wait $DPID 2>/dev/null
```

- [ ] **Step 6.2: Pre-commit code review**

Через Agent. Черновик:

```
feat(web): land /api/top/queries with interner-entry instrumentation

Operators triaging a busy pooler can now hit /api/top/queries to see
the heaviest prepared statements by Bind count or by mean execution
time. The endpoint sorts server-side, defaults to by=count with n=20,
caps n at 200.

Two atomic counters per interner entry track this. count is bumped on
every Bind that resolves to a hash; total_duration_us absorbs the
batch's elapsed microseconds at Sync time. The hot path additions are
two Relaxed fetch_adds per query, on the order of 50 ns each — small
enough to fit the project's "stats may be approximate, throughput
may not be" rule.

Approximation contract: count is Bind-count, not Execute-count or
Parse-count. Duration attribution is per-batch — a Sync that ended a
batch with multiple Bind messages credits the entire elapsed time to
the last Bind's hash. Simple queries do not flow through the interner
and are absent from this endpoint; the Top-N for non-prepared traffic
is /api/top/clients.

Tests: <N> lib tests passed (was 793); cargo clippy --lib and
cargo fmt --check clean. Release smoke confirmed both ?by=count and
?by=duration return 200 with the expected envelope.

Phase 3d-2 of seven; phase 3d-3 lands /api/top/prepared with a similar
lightweight per-CacheEntry hit/miss pair.
```

Pre-commit reviewer ловит em-dashes — заранее использовать `;` или фразой реализации заменить.

- [ ] **Step 6.3: commit**

---

## Self-review

**Spec coverage check:**
- ✅ `/api/top/queries?by=count|duration&n=20` (раздел 8.2) — Tasks 3, 4, 5.
- ⚠ `?pool=` filter из spec — out of scope (interner is global, per-pool requires per-pool counters).

**Hot path:**
- 2 Relaxed atomic fetch_add per query (1 on Bind, 1 on Sync). +1 Option<(u64, bool)> store/load. ~50-100 ns/query. 0.05% CPU at 5kTPS.

**Imprecision:** документировано в plan + commit message + DTO doc-comments.

**Type-consistency:** `total_duration_us: AtomicU64` storage matches existing `total_query_time_microseconds: u64` convention в PoolStats. Frontend конвертит ms.

**Placeholder check:** Нет.

---

## Execution Handoff

Plan complete. Subagent-driven execution.
