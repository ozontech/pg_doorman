# Web UI — Phase 3d-3 Implementation Plan: per-CacheEntry hit/miss + /api/top/prepared

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Поднять `GET /api/top/prepared?by=hits|misses&n=20`. Добавить per-CacheEntry hit/miss counters + hot path hook в Parse handler. Расширить существующий `/api/prepared` ответ полями `hits` и `misses` (additive change).

**Architecture:**
- `CacheEntry` (`src/server/prepared_statement_cache.rs:352`) получает два новых поля: `hit_count: AtomicU64`, `miss_count: AtomicU64`.
- Public methods на `PreparedStatementCache`:
  - `record_hit(hash)` — `DashMap.get(hash)` + `fetch_add(_, Relaxed)` на `hit_count`.
  - `record_miss(hash)` — same для `miss_count`.
- Hot path hook: единственный sites — `src/client/protocol.rs:266` в Parse handler. Там есть `hash` (line 183) и `pool`. После `if server.has_prepared_statement(&server_stmt_name)` развилки записываем hit либо miss на CacheEntry с этим hash.
- `get_entries` сигнатура расширяется до `Vec<(u64, Arc<Parse>, u64, CacheEntryKind, u64, u64)>` — последние два поля hits и misses. Колл-сайты обновляются (admin/show.rs, collect.rs:712 и :758).
- `/api/prepared` (3c-3) — `PreparedRowDto` получает `hits: u64` и `misses: u64`. Operators сразу видят hit-rate per statement.
- Новый `/api/top/prepared?by=hits|misses&n=20` — public endpoint без preview (privacy: same as /api/prepared). Sort by chosen counter desc.

**Rationale для Parse-handler hook:** spec semantic для /api/top/prepared by hits|misses — «which prepared statements does the server already have when the client asks?». Это про PREPARE-time check, не Bind-time. Parse handler — natural site: client shipped Parse, мы спрашиваем server.has_prepared_statement(name), и right at that point знаем hit/miss + hash. На Bind также вызывается has_prepared_statement (через ensure_prepared_statement_is_on_server → register_prepared_statement), но это re-verification existing state, double-counting smear.

**Hot path budget:** 1 DashMap.get + 1 fetch_add(Relaxed) per Parse hit OR miss. ≈ 30-50 ns. Parse — реже Bind (одна Parse часто покрывает множественные Binds через prepared statement reuse). На 5kTPS prepared-heavy load это много меньше 0.05% CPU.

**Imprecision contract:** Counters per-CacheEntry per-pool (CacheEntry живёт в pool's PreparedStatementCache). Если оператор хочет «глобальный hit-rate per statement», он суммирует hits/misses через все pools (frontend). LRU-eviction CacheEntry → counters теряются для evicted entries. Это уже задокументировано в DTO.

**Reference:**
- Spec section 8.2.
- Phase 3d-2 commit: `e31d570`.
- Hot path hook site: `src/client/protocol.rs:266`.

**Не входит в фазу 3d-3:**
- /api/events — phase 3d-4.

---

## File Structure

**Модифицируемые:**
- `src/server/prepared_statement_cache.rs` — CacheEntry + record_hit/miss + get_entries signature.
- `src/admin/show.rs` — обновить show_prepared_statements destructuring.
- `src/web/routes/collect.rs` — обновить collect_prepared / collect_prepared_text destructuring + collect_top_prepared.
- `src/web/routes/dto.rs` — extend PreparedRowDto + add TopPreparedDto/TopPreparedRowDto.
- `src/client/protocol.rs` — Parse handler hook at line 266 area.
- `src/web/routes/mod.rs` — register top_prepared module.
- `src/web/server.rs` — arm + dispatch test.
- `src/web/tests.rs` — integration test.

**Новые:**
- `src/web/routes/top_prepared.rs` — handler.

---

## Task 0: Baseline

```bash
cd /home/vadv/Projects/pg_doorman
git status && git log --oneline -3
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: HEAD=`e31d570`, 800 tests, clean.

---

## Task 1: Backend instrumentation в CacheEntry + public API

**Files:** Modify `src/server/prepared_statement_cache.rs`.

- [ ] **Step 1.1: Расширить CacheEntry**

Линия ~352:

```rust
struct CacheEntry {
    parse: Arc<Parse>,
    count_used: u64,
    kind_flags: AtomicU8,
    /// Cumulative count of Parse-time has_prepared_statement(server_name) hits
    /// for this hash. Approximate per-pool counter — see plan for the LRU
    /// eviction caveat.
    hit_count: AtomicU64,
    /// Cumulative count of Parse-time has_prepared_statement(server_name)
    /// misses for this hash.
    miss_count: AtomicU64,
}

impl CacheEntry {
    fn new(parse: Arc<Parse>, count_used: u64, initial_kind: CacheEntryKind) -> Self {
        let bits = match initial_kind {
            CacheEntryKind::Named => FLAG_NAMED,
            CacheEntryKind::Anonymous => FLAG_ANONYMOUS,
            CacheEntryKind::Mixed => FLAG_NAMED | FLAG_ANONYMOUS,
        };
        Self {
            parse,
            count_used,
            kind_flags: AtomicU8::new(bits),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
        }
    }

    // ... existing methods (note_named, note_anonymous, kind) unchanged
}
```

- [ ] **Step 1.2: Public methods на PreparedStatementCache**

В `impl PreparedStatementCache` добавить (рядом с promote / get_entries):

```rust
    /// Atomically increments the hit counter on the entry for `hash`.
    /// Silently no-ops when the entry was evicted or never inserted —
    /// keeps the hot path lock-free.
    pub fn record_hit(&self, hash: u64) {
        if let Some(entry) = self.cache.get(&hash) {
            entry.hit_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Same as `record_hit`, but for misses.
    pub fn record_miss(&self, hash: u64) {
        if let Some(entry) = self.cache.get(&hash) {
            entry.miss_count.fetch_add(1, Ordering::Relaxed);
        }
    }
```

- [ ] **Step 1.3: Расширить get_entries сигнатуру**

```rust
    /// Returns all entries with stats. Tuple is
    /// `(hash, parse, count_used, kind, hit_count, miss_count)`.
    pub fn get_entries(&self) -> Vec<(u64, Arc<Parse>, u64, CacheEntryKind, u64, u64)> {
        self.cache
            .iter()
            .map(|entry| {
                (
                    *entry.key(),
                    entry.parse.clone(),
                    entry.count_used,
                    entry.kind(),
                    entry.hit_count.load(Ordering::Relaxed),
                    entry.miss_count.load(Ordering::Relaxed),
                )
            })
            .collect()
    }
```

- [ ] **Step 1.4: Unit-тесты на record_hit / record_miss**

В `#[cfg(test)] mod tests` добавить:

```rust
    #[test]
    fn record_hit_no_op_when_hash_absent() {
        let cache = super::PreparedStatementCache::new(NonZeroUsize::new(8).unwrap());
        cache.record_hit(0xDEADBEEF);
        cache.record_miss(0xDEADBEEF);
        // No panic = pass; counters unobservable on absent hash.
    }

    #[test]
    fn record_hit_increments_existing_entry() {
        let cache = super::PreparedStatementCache::new(NonZeroUsize::new(8).unwrap());
        let parse = Arc::new(Parse::try_from(&example_parse_bytes("SELECT 1")).unwrap());
        let _ = cache.get_or_insert(0x1111, parse, super::CacheEntryKind::Named);
        cache.record_hit(0x1111);
        cache.record_hit(0x1111);
        cache.record_miss(0x1111);
        let entries = cache.get_entries();
        let row = entries.iter().find(|e| e.0 == 0x1111).unwrap();
        assert_eq!(row.4, 2, "hits");
        assert_eq!(row.5, 1, "misses");
    }
```

(Если `example_parse_bytes` нет — использовать существующий fixture pattern из соседних тестов в этом же mod tests; см. line ~575.)

- [ ] **Step 1.5: Build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib server::prepared_statement_cache 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: clean. Compilation errors на admin/show.rs:177 и collect.rs:712,758 ожидаемы — это callers get_entries которые надо обновить в Task 2.

(Если build падает на этих местах — продолжаем в Task 2; не возвращаемся к Task 1.)

- [ ] **Step 1.6: Не коммитить.**

---

## Task 2: Обновить callers `get_entries`

**Files:**
- Modify: `src/admin/show.rs`, `src/web/routes/collect.rs`.

- [ ] **Step 2.1: admin/show.rs:178 destructuring**

Заменить:

```rust
            for (hash, parse, last_used, kind) in entries {
```

На:

```rust
            for (hash, parse, last_used, kind, _hits, _misses) in entries {
```

(`_` префикс — show_prepared_statements admin command не выдаёт hits/misses в данную колоночную форму; добавлять колонки в admin output вне scope этой фазы.)

- [ ] **Step 2.2: collect.rs:712 destructuring (collect_prepared)**

Заменить:

```rust
        for (hash, parse, count_used, kind) in cache.get_entries() {
            prepared.push(PreparedRowDto {
                pool: identifier.to_string(),
                hash: hash.to_string(),
                name: parse.name.clone(),
                count_used,
                kind: kind.as_str().to_string(),
            });
        }
```

На:

```rust
        for (hash, parse, count_used, kind, hits, misses) in cache.get_entries() {
            prepared.push(PreparedRowDto {
                pool: identifier.to_string(),
                hash: hash.to_string(),
                name: parse.name.clone(),
                count_used,
                hits,
                misses,
                kind: kind.as_str().to_string(),
            });
        }
```

- [ ] **Step 2.3: collect.rs:758 destructuring (collect_prepared_text)**

Заменить:

```rust
        for (h, parse, _count, kind) in cache.get_entries() {
            if h == hash {
                return Some(PreparedTextDto {
```

На:

```rust
        for (h, parse, _count, kind, _hits, _misses) in cache.get_entries() {
            if h == hash {
                return Some(PreparedTextDto {
```

(prepared/text endpoint не выдаёт hits/misses — это admin lookup конкретного query, не aggregate.)

- [ ] **Step 2.4: Build**

```bash
cargo build --lib 2>&1 | tail -5
```
Expected: dto.rs ругается на missing field `hits`/`misses` в PreparedRowDto — fix в Task 3.

(Можно перейти в Task 3.)

---

## Task 3: DTOs — extend PreparedRowDto + новый TopPrepared

**Files:** Modify `src/web/routes/dto.rs`.

- [ ] **Step 3.1: Расширить PreparedRowDto**

Найти существующий `PreparedRowDto` (был добавлен в 3c-3, должен быть в районе линии ~430+):

```rust
#[derive(Debug, Serialize)]
pub struct PreparedRowDto {
    pub pool: String,
    pub hash: String,
    pub name: String,
    pub count_used: u64,
    /// Cumulative Parse-time hits — server already had this prepared statement
    /// when the client asked. Per-pool, per-CacheEntry. Lost on LRU eviction.
    pub hits: u64,
    /// Cumulative Parse-time misses — server lacked this prepared statement,
    /// requiring a fresh Parse to PostgreSQL. Per-pool, per-CacheEntry.
    pub misses: u64,
    pub kind: String,
}
```

- [ ] **Step 3.2: TopPreparedDto + TopPreparedRowDto + filters**

В конец dto.rs:

```rust
/// `GET /api/top/prepared?by=hits|misses&n=20` — Top-N prepared statements
/// across all pools, sorted by cumulative hit or miss count. Public; no SQL
/// preview — for the body use admin-only `/api/prepared/text/{hash}`.
#[derive(Debug, Serialize)]
pub struct TopPreparedDto {
    pub ts: u64,
    pub by: String,
    pub n: u64,
    pub prepared: Vec<TopPreparedRowDto>,
}

#[derive(Debug, Serialize)]
pub struct TopPreparedRowDto {
    pub pool: String,
    pub hash: String,
    pub name: String,
    pub count_used: u64,
    pub hits: u64,
    pub misses: u64,
    pub kind: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum TopPreparedBy {
    #[default]
    Hits,
    Misses,
}

impl TopPreparedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            TopPreparedBy::Hits => "hits",
            TopPreparedBy::Misses => "misses",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TopPreparedFilters {
    pub by: TopPreparedBy,
    pub n: u64,
}
```

- [ ] **Step 3.3: Build**

```bash
cargo build --lib 2>&1 | tail -5
```
Expected: clean (PreparedRowDto теперь имеет hits/misses, что Task 2.2 уже инжектит).

---

## Task 4: collect_top_prepared

**Files:** Modify `src/web/routes/collect.rs`.

- [ ] **Step 4.1: Расширить use-импорты**

```rust
use crate::web::routes::dto::{
    TopPreparedBy, TopPreparedDto, TopPreparedFilters, TopPreparedRowDto,
    // existing
};
```

- [ ] **Step 4.2: collect_top_prepared**

Добавить (после collect_prepared_text):

```rust
pub fn collect_top_prepared(filters: &TopPreparedFilters) -> TopPreparedDto {
    let n = clamp_top_clients_n(filters.n);

    let mut rows: Vec<TopPreparedRowDto> = Vec::new();
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        for (hash, parse, count_used, kind, hits, misses) in cache.get_entries() {
            rows.push(TopPreparedRowDto {
                pool: identifier.to_string(),
                hash: hash.to_string(),
                name: parse.name.clone(),
                count_used,
                hits,
                misses,
                kind: kind.as_str().to_string(),
            });
        }
    }

    rows.sort_by(|a, b| match filters.by {
        TopPreparedBy::Hits => b.hits.cmp(&a.hits),
        TopPreparedBy::Misses => b.misses.cmp(&a.misses),
    });

    let prepared: Vec<_> = rows.into_iter().take(n as usize).collect();

    TopPreparedDto {
        ts: now_unix_ms(),
        by: filters.by.as_str().to_string(),
        n,
        prepared,
    }
}
```

- [ ] **Step 4.3: Build + clippy + fmt**

Expected: clean.

---

## Task 5: hot path hook + handler + mux + tests

**Files:**
- Modify: `src/client/protocol.rs`, `src/web/routes/mod.rs`, `src/web/server.rs`, `src/web/tests.rs`.
- Create: `src/web/routes/top_prepared.rs`.

- [ ] **Step 5.1: Parse handler hook**

В `src/client/protocol.rs` найти линию ~266:

```rust
        if server.has_prepared_statement(&server_stmt_name) {
```

Заменить на:

```rust
        let server_has_it = server.has_prepared_statement(&server_stmt_name);
        if let Some(cache) = pool.prepared_statement_cache.as_ref() {
            // Per-CacheEntry hit/miss for /api/top/prepared. Silent no-op
            // when the entry was evicted between register_parse_to_cache and
            // here — same lock-free policy as /api/top/queries.
            if server_has_it {
                cache.record_hit(hash);
            } else {
                cache.record_miss(hash);
            }
        }
        if server_has_it {
```

(Альтернатива — просто добавить два вызова перед `if server.has_prepared_statement(...)`, но это вызвало бы has_prepared_statement дважды и удвоило per-server counter; поэтому переменная server_has_it.)

- [ ] **Step 5.2: top_prepared.rs**

```rust
//! GET /api/top/prepared?by=hits|misses&n=20 handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_top_prepared;
use crate::web::routes::dto::{TopPreparedBy, TopPreparedFilters};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_top_prepared(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = TopPreparedFilters {
        by: match first(query, "by").as_deref() {
            Some("misses") => TopPreparedBy::Misses,
            _ => TopPreparedBy::Hits,
        },
        n: parse_u64(query, "n", 0),
    };
    Response::ok_json(&collect_top_prepared(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_prepared_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_top_prepared(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"hits\""));
        assert!(body.contains("\"n\":20"));
        assert!(body.contains("\"prepared\""));
    }

    #[test]
    fn top_prepared_by_misses_param() {
        let mut q = BTreeMap::new();
        q.insert("by".into(), vec!["misses".into()]);
        let r = handle_top_prepared(&q);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"misses\""));
    }
}
```

- [ ] **Step 5.3: mod.rs**

```rust
pub(crate) mod top_prepared;
```

- [ ] **Step 5.4: route_api arm + dispatch test**

В `src/web/server.rs::route_api`:

```rust
        "/api/top/prepared" => routes::top_prepared::handle_top_prepared(&query),
```

Dispatch-тест:

```rust
    #[test]
    fn dispatch_top_prepared_returns_200() {
        let r = dispatch(
            &req("GET", "/api/top/prepared"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }
```

- [ ] **Step 5.5: Integration test**

В `src/web/tests.rs`:

```rust
#[tokio::test]
async fn api_top_prepared_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/top/prepared HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"by\":\"hits\""), "raw={raw}");
    assert!(raw.contains("\"prepared\""), "raw={raw}");
}
```

- [ ] **Step 5.6: Полный прогон**

```bash
cargo test --lib 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~806 passed (800 + 2 cache + 2 handler + 1 dispatch + 1 integration). Точное число при прогоне.

- [ ] **Step 5.7: Не коммитить.**

---

## Task 6: smoke + commit

- [ ] **Step 6.1: Release smoke**

```bash
cargo build --release 2>&1 | tail -3
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3d3.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/prepared (with new hits/misses) ---"
curl -s 'http://127.0.0.1:19127/api/prepared' | head -c 400
echo ""
echo "--- /api/top/prepared default by=hits ---"
curl -s 'http://127.0.0.1:19127/api/top/prepared' | head -c 400
echo ""
echo "--- /api/top/prepared?by=misses&n=5 ---"
curl -s 'http://127.0.0.1:19127/api/top/prepared?by=misses&n=5' | head -c 400
echo ""
kill $DPID 2>/dev/null
wait $DPID 2>/dev/null
```

- [ ] **Step 6.2: Pre-commit code review**

Через Agent (правило CLAUDE.md). Черновик:

```
feat(web): land /api/top/prepared with per-CacheEntry hit/miss instrumentation

The Caches page can now show which prepared statements are seeing the
most cache hits versus misses. /api/top/prepared sorts pool-cache
entries server-side by hits or misses, defaults to by=hits with n=20,
caps n at 200. /api/prepared response gains hits and misses fields so
the existing endpoint also benefits.

Two atomic counters per CacheEntry track this. The hot path hook is a
single Parse-handler call site after the existing
has_prepared_statement check; on hit we increment hits, on miss we
increment misses. Both via a DashMap.get + Relaxed fetch_add — same
lock-free no-op-on-absence pattern used by /api/top/queries in
phase 3d-2.

Approximation contract: counters are per-pool per-CacheEntry. LRU
eviction discards counters; long-lived prepared statements with many
re-Parses keep their numbers, ephemeral statements that churn out of
the LRU lose theirs. Operators triage with this caveat in mind.

Tests: <N> lib tests passed (was 800); cargo clippy --lib and
cargo fmt --check clean. Release smoke confirmed both ?by=hits and
?by=misses return 200 with the expected envelope; /api/prepared
includes the new hits and misses fields.

Phase 3d-3 of seven; phase 3d-4 lands /api/events with the admin
command ring buffer.
```

Pre-commit reviewer наверняка найдёт em-dashes — заранее использовать `;` или восстановить полные предложения.

- [ ] **Step 6.3: commit**

---

## Self-review

**Spec coverage:**
- ✅ `/api/top/prepared?by=hits|misses&n=20` (раздел 8.2) — Tasks 1, 3, 4, 5.
- ✅ Public access — handlers без auth (mux обрабатывает ui_anonymous).
- ✅ Privacy — нет query preview в response.

**Hot path:**
- 1 fetch_add Relaxed per Parse hit OR miss. Plus the no-op DashMap.get on miss. ~50 ns.

**Side benefit:** /api/prepared (3c-3) теперь содержит hits/misses — operators видят hit-rate per statement.

**Imprecision:** counters per-CacheEntry per-pool, теряются при LRU eviction. Документировано в DTO + commit message + plan.

**Type-consistency:** `hits/misses: u64` matches existing pool.errors/queries/transactions naming pattern.

**Placeholder check:** Нет.

---

## Execution Handoff

Plan complete. Subagent-driven execution.
