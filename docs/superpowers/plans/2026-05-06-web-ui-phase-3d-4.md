# Web UI — Phase 3d-4 Implementation Plan: admin events ring buffer + /api/events

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Поднять `GET /api/events?since=<seq>&max=<200>`. Bounded ring buffer админ-событий (RELOAD, PAUSE, RESUME, RECONNECT) для timeline-аннотаций на frontend Overview графиках.

**Architecture:**
- Новый module `src/admin/events.rs` с:
  - `pub struct EventEntry { seq: u64, ts_ms: u64, target: &'static str, message: String }`
  - Static `EVENT_BUFFER: OnceLock<Mutex<VecDeque<EventEntry>>>` с capacity 1024.
  - `pub fn push_event(target: &'static str, message: String)` — увеличивает seq, append в buffer, pop_front если переполнено.
  - `pub fn get_events_since(since: u64, max: usize) -> (Vec<EventEntry>, u64)` — возвращает (entries, next_seq).
- Hook 4 admin command'а в `src/admin/commands.rs`:
  - `reload` — `push_event("RELOAD", "config reloaded")` после успешной перезагрузки.
  - `pause` — `push_event("PAUSE", format!("pool {identifier} paused"))` per pool.
  - `resume` — `push_event("RESUME", format!("pool {identifier} resumed"))` per pool.
  - `reconnect` — `push_event("RECONNECT", format!("pool {identifier} reconnected (epoch={new_epoch})"))` per pool.
- /api/events?since=<seq>&max=<N> handler (default max=200, cap 1000).

**Hot path:** не трогается. Admin commands вызываются ~раз в несколько минут на cluster, Mutex там — норма. Полностью вне SQL пути.

**Reference:**
- Spec section 8.2.
- Phase 3d-3 commit: `809e8d9`.

**Не входит:** LogTap для общих логов — phase 4. /api/events это узкоспециализированный subset для админ-команд, не replacement для LogTap.

---

## File Structure

**Новые:**
- `src/admin/events.rs` — ring buffer + push/get API + unit tests.
- `src/web/routes/events.rs` — handler.

**Модифицируемые:**
- `src/admin/mod.rs` — `pub mod events;`.
- `src/admin/commands.rs` — 4 hooks.
- `src/web/routes/dto.rs` — `EventsDto`, `EventEntryDto`.
- `src/web/routes/collect.rs` — `collect_events`.
- `src/web/routes/mod.rs` — register events module.
- `src/web/server.rs` — arm + dispatch test.
- `src/web/tests.rs` — integration test.

---

## Task 0: Baseline

```bash
cd /home/vadv/Projects/pg_doorman
git status && git log --oneline -3
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: HEAD=`809e8d9`, 806 tests, clean.

---

## Task 1: src/admin/events.rs ring buffer

**Files:** Create `src/admin/events.rs`, modify `src/admin/mod.rs`.

- [ ] **Step 1.1: Создать `src/admin/events.rs`**

```rust
//! Bounded ring buffer of admin command events (RELOAD, PAUSE, RESUME,
//! RECONNECT). Frontend reads via `/api/events?since=<seq>&max=` to
//! render vertical-line annotations on Overview graphs.
//!
//! This module is intentionally simple: a single Mutex<VecDeque>, no
//! lock-free fancy. Admin commands fire on the order of one per few
//! minutes per cluster; contention is negligible. Reads come from a
//! handful of Web UI operators per hour.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of events retained in the ring. At one admin command
/// every few minutes this is well over a day of history; oldest events
/// are dropped silently when the buffer fills.
const BUFFER_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub struct EventEntry {
    /// Monotonically increasing sequence number assigned at push time.
    pub seq: u64,
    /// Wall-clock timestamp in milliseconds since unix epoch.
    pub ts_ms: u64,
    /// One of `"RELOAD"`, `"PAUSE"`, `"RESUME"`, `"RECONNECT"`.
    pub target: &'static str,
    /// Human-readable description of the event (e.g. `"pool main@db1 paused"`).
    pub message: String,
}

static SEQ_COUNTER: AtomicU64 = AtomicU64::new(1);

fn buffer() -> &'static Mutex<VecDeque<EventEntry>> {
    static BUFFER: OnceLock<Mutex<VecDeque<EventEntry>>> = OnceLock::new();
    BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(BUFFER_CAPACITY)))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Records an admin event in the ring. Called from `src/admin/commands.rs`
/// after a successful command.
pub fn push_event(target: &'static str, message: String) {
    let entry = EventEntry {
        seq: SEQ_COUNTER.fetch_add(1, Ordering::Relaxed),
        ts_ms: now_ms(),
        target,
        message,
    };
    if let Ok(mut buf) = buffer().lock() {
        if buf.len() >= BUFFER_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

/// Returns all events with `seq > since`, capped at `max` entries, plus the
/// next sequence number an operator should poll with. Empty when no events
/// have been pushed.
pub fn get_events_since(since: u64, max: usize) -> (Vec<EventEntry>, u64) {
    let buf = match buffer().lock() {
        Ok(b) => b,
        Err(_) => return (Vec::new(), since),
    };
    let mut next_seq = since;
    let entries: Vec<EventEntry> = buf
        .iter()
        .filter(|e| e.seq > since)
        .take(max)
        .map(|e| {
            next_seq = next_seq.max(e.seq);
            e.clone()
        })
        .collect();
    (entries, next_seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Tests touch the global SEQ_COUNTER and BUFFER, so serialise them.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: OnceLock<StdMutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| StdMutex::new(())).lock().unwrap()
    }

    fn drain() {
        let _ = buffer().lock().map(|mut b| b.clear());
    }

    #[test]
    fn push_event_appends_with_increasing_seq() {
        let _g = lock();
        drain();
        push_event("RELOAD", "config reloaded".into());
        push_event("PAUSE", "pool main@db1 paused".into());
        let (events, next) = get_events_since(0, 100);
        assert_eq!(events.len(), 2);
        assert!(events[0].seq < events[1].seq);
        assert_eq!(next, events[1].seq);
        assert_eq!(events[0].target, "RELOAD");
        assert_eq!(events[1].target, "PAUSE");
    }

    #[test]
    fn get_events_since_filters_by_seq() {
        let _g = lock();
        drain();
        push_event("RELOAD", "first".into());
        push_event("PAUSE", "second".into());
        let (first_batch, next1) = get_events_since(0, 100);
        let (after_first, next2) = get_events_since(first_batch[0].seq, 100);
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].message, "second");
        assert_eq!(next2, next1);
    }

    #[test]
    fn get_events_since_respects_max() {
        let _g = lock();
        drain();
        for i in 0..10 {
            push_event("RELOAD", format!("event {i}"));
        }
        let (events, _) = get_events_since(0, 3);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn buffer_drops_oldest_when_full() {
        let _g = lock();
        drain();
        for i in 0..(BUFFER_CAPACITY + 50) {
            push_event("RELOAD", format!("event {i}"));
        }
        let (events, _) = get_events_since(0, BUFFER_CAPACITY * 2);
        assert_eq!(events.len(), BUFFER_CAPACITY);
        // Earliest 50 dropped — first surviving event is index 50 (message "event 50").
        assert_eq!(events[0].message, "event 50");
    }
}
```

- [ ] **Step 1.2: src/admin/mod.rs**

Добавить:

```rust
pub mod events;
```

(в конец, рядом с другими `pub mod` в admin/mod.rs.)

- [ ] **Step 1.3: build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib admin::events 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 4 admin::events tests passing, clean.

- [ ] **Step 1.4: Не коммитить.**

---

## Task 2: hooks в admin/commands.rs

**Files:** Modify `src/admin/commands.rs`.

- [ ] **Step 2.1: reload hook**

В функции `pub async fn reload(...)`, после `reload_config(client_server_map).await?;`:

```rust
    crate::admin::events::push_event("RELOAD", "config reloaded".to_string());
```

- [ ] **Step 2.2: pause hook**

В цикле `for (identifier, pool) in pools.iter()`, после `pool.database.pause();`:

```rust
        crate::admin::events::push_event(
            "PAUSE",
            format!("pool {identifier} paused"),
        );
```

(Перед существующим `info!("PAUSE: paused pool {}", identifier);` либо после — порядок не критичен.)

- [ ] **Step 2.3: resume hook**

Аналогично:

```rust
        crate::admin::events::push_event(
            "RESUME",
            format!("pool {identifier} resumed"),
        );
```

- [ ] **Step 2.4: reconnect hook**

В цикле, после `let new_epoch = pool.database.reconnect();`:

```rust
        crate::admin::events::push_event(
            "RECONNECT",
            format!("pool {identifier} reconnected (epoch={new_epoch})"),
        );
```

- [ ] **Step 2.5: build + clippy + fmt**

Expected: clean.

- [ ] **Step 2.6: Не коммитить.**

---

## Task 3: DTOs

**Files:** Modify `src/web/routes/dto.rs`.

- [ ] **Step 3.1: EventsDto + EventEntryDto**

В конец dto.rs:

```rust
/// `GET /api/events?since=<seq>&max=<N>` — admin command timeline used
/// for vertical-line annotations on the Overview graphs. Bounded ring
/// buffer; oldest events drop silently when full.
#[derive(Debug, Serialize)]
pub struct EventsDto {
    pub ts: u64,
    /// Sequence number to poll with on the next request to receive only
    /// events newer than this batch. Equal to `since` when nothing new.
    pub next_seq: u64,
    pub events: Vec<EventEntryDto>,
}

#[derive(Debug, Serialize)]
pub struct EventEntryDto {
    pub seq: u64,
    pub ts_ms: u64,
    /// One of `"RELOAD"`, `"PAUSE"`, `"RESUME"`, `"RECONNECT"`.
    pub target: String,
    pub message: String,
}
```

- [ ] **Step 3.2: build + clippy + fmt**

Expected: clean.

- [ ] **Step 3.3: Не коммитить.**

---

## Task 4: collect_events

**Files:** Modify `src/web/routes/collect.rs`.

- [ ] **Step 4.1: use-импорт + функция**

```rust
use crate::admin::events::get_events_since;
use crate::web::routes::dto::{EventEntryDto, EventsDto, /* existing */};

pub fn collect_events(since: u64, max: u64) -> EventsDto {
    // Cap max at 1000 — protects against accidental ?max=10000 over the wire.
    const HARD_CAP: usize = 1000;
    let max_n = (max.min(HARD_CAP as u64) as usize).max(1);
    let (entries, next_seq) = get_events_since(since, max_n);

    let events: Vec<EventEntryDto> = entries
        .into_iter()
        .map(|e| EventEntryDto {
            seq: e.seq,
            ts_ms: e.ts_ms,
            target: e.target.to_string(),
            message: e.message,
        })
        .collect();

    EventsDto {
        ts: now_unix_ms(),
        next_seq,
        events,
    }
}
```

- [ ] **Step 4.2: build + clippy + fmt**

Expected: clean.

- [ ] **Step 4.3: Не коммитить.**

---

## Task 5: handler + mux + tests

**Files:**
- Create: `src/web/routes/events.rs`.
- Modify: `src/web/routes/mod.rs`, `src/web/server.rs`, `src/web/tests.rs`.

- [ ] **Step 5.1: events.rs**

```rust
//! GET /api/events?since=<seq>&max=<N> handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_events;
use crate::web::routes::query::parse_u64;
use crate::web::server::Response;

pub(crate) fn handle_events(query: &BTreeMap<String, Vec<String>>) -> Response {
    let since = parse_u64(query, "since", 0);
    let max = parse_u64(query, "max", 200);
    Response::ok_json(&collect_events(since, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_events(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"next_seq\""));
        assert!(body.contains("\"events\""));
    }
}
```

- [ ] **Step 5.2: mod.rs**

```rust
pub(crate) mod events;
```

- [ ] **Step 5.3: route_api arm + dispatch test**

В `src/web/server.rs::route_api`:

```rust
        "/api/events" => routes::events::handle_events(&query),
```

Dispatch-тест:

```rust
    #[test]
    fn dispatch_events_returns_200() {
        let r = dispatch(
            &req("GET", "/api/events"),
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
async fn api_events_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/events HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"events\""), "raw={raw}");
    assert!(raw.contains("\"next_seq\""), "raw={raw}");
}
```

- [ ] **Step 5.5: Полный прогон**

```bash
cargo test --lib 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~813 passed (806 + 4 admin::events + 1 handler + 1 dispatch + 1 integration). Точное число при прогоне.

- [ ] **Step 5.6: Не коммитить.**

---

## Task 6: smoke + commit

- [ ] **Step 6.1: Release smoke**

```bash
cargo build --release 2>&1 | tail -3
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3d4.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/events (empty initially) ---"
curl -s 'http://127.0.0.1:19127/api/events' | head -c 200
echo ""
echo "--- /api/events?since=0&max=10 (still empty) ---"
curl -s 'http://127.0.0.1:19127/api/events?since=0&max=10' | head -c 200
echo ""
kill $DPID 2>/dev/null
wait $DPID 2>/dev/null
```

(Без admin connection нельзя trigger RELOAD/PAUSE — buffer останется пустым в smoke. Это ОК; integration tests + admin::events unit tests покрывают push path.)

- [ ] **Step 6.2: Pre-commit code review**

Через Agent. Черновик:

```
feat(web): land /api/events with admin command ring buffer

The Web UI's Overview graphs can now render vertical-line annotations
for the four state-changing admin commands. /api/events takes since=
and max= query parameters, returns the events newer than `since`, and
echoes the next sequence number so the next poll picks up where this
one stopped. The ring buffer holds 1024 entries; older events drop
silently when full, which is well over a day of history at typical
admin cadence.

Producer side: each successful RELOAD, PAUSE, RESUME, or RECONNECT
admin command pushes an entry under a Mutex<VecDeque>. Admin commands
fire at the rate of a handful per cluster per day; contention is
nonexistent. The SQL hot path is untouched.

Tests: <N> lib tests passed (was 806); cargo clippy --lib and
cargo fmt --check clean. The unit tests for the ring buffer cover the
sequence-monotonic, since-filter, max-cap, and overflow-drops-oldest
behaviours.

Phase 3d-4 of seven; this closes phase 3d. Phase 4 lands the LogTap
infrastructure for the admin-only /api/logs endpoint.
```

Pre-commit reviewer наверняка найдёт em-dashes.

- [ ] **Step 6.3: commit**

---

## Self-review

**Spec coverage:**
- ✅ /api/events?since=<seq>&max=<N> (раздел 8.2) — Tasks 1, 3, 4, 5.
- ✅ Admin command targets (RELOAD, PAUSE, RESUME, RECONNECT) — Task 2.
- ✅ Public access (handlers без admin auth, mux обрабатывает ui_anonymous).

**Hot path:** ВНЕ. Admin commands ratе of one per few minutes; Mutex contention nil.

**Imprecision:** Bounded ring drop-oldest — задокументировано в DTO.

**Completion:** Этим коммитом phase 3d полностью закрыта (3d-1, 3d-2, 3d-3, 3d-4). Roadmap pending: phase 4 (LogTap), 5-6 (frontend), 7 (embedding+CI+BDD).

---

## Execution Handoff

Plan complete. Subagent-driven execution.
