# Web UI — Phase 3b Implementation Plan: /api/clients + /api/servers

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Поднять `GET /api/clients` и `GET /api/servers` с server-side filter/sort/pagination, plus expose client-side state-age (current_query_age_ms, wait_ms) которых сегодня в backend нет. Это два самых нагруженных endpoint'а для UI — список клиентов pooler'а может быть на тысячи строк, поэтому пагинация и фильтрация делается на сервере (нарушаем decision #21 «thin server» только для list endpoints с возможно-большим объёмом — обоснование ниже).

**Architecture:** Расширяем `ClientStats` одним atomic timestamp `state_since_nanos` (mirror того, что уже есть в `ServerStats` как `active_since_nanos_from_connect`). При каждом state-transition записываем nanos-from-connect; читатель вычисляет `current_query_age_ms` или `wait_ms` в зависимости от текущего state. Server-side: новый `collect_clients(filters)` возвращает уже отфильтрованный/отсортированный/пагинированный список. Аналогично `collect_servers(filters)`. Mux парсит query string и передаёт structured фильтры в handler.

**Decision rationale (отступление от #21):** Полный transfer всех клиентов поверх HTTP при тысячах активных соединений — это килобайты JSON каждые 1.5s. Server-side filter/sort/pagination даёт O(N) cost ровно один раз на стороне backend и O(visible) на стороне frontend. Frontend дополнительно фильтрует уже-отображаемые клиенты по UI-state (например, search в text-box).

**Tech Stack:** То же что и phase 3a. Парсер query string — ручной (нет `serde_urlencoded` в deps; добавлять не стоит).

**Reference:**
- Spec: `2026-05-06-web-ui-design.md`, разделы 8.2 (общие query params), 8.5 (/api/clients shape), 15.3 (per-client highlights что нужны UI).
- Phase 3a commit: `14ca557`.
- Decision log #16, #17, #21.

**Не входит в фазу 3b:**
- /api/connections, /api/stats, /api/databases, /api/users, /api/config — phase 3c.
- /api/top/*, /api/apps, /api/events — phase 3d.
- Top-N endpoints — phase 3d.
- Реальный backend timing-tracking (мы добавим только state_since_nanos для client, не больше).

---

## File Structure

**Новые файлы:**
- `src/web/routes/clients.rs` — handler для /api/clients.
- `src/web/routes/servers.rs` — handler для /api/servers.
- `src/web/routes/query.rs` — небольшой парсер query string в `BTreeMap<String, Vec<String>>` либо typed Filter struct.

**Модифицируемые файлы:**
- `src/stats/client.rs` — добавить `state_since_nanos: AtomicU64`, hooks в `set_state`/`set_wait`/`set_state_wait`, accessors `current_query_age_ms()` + `wait_ms()`.
- `src/web/routes/dto.rs` — добавить ClientDto, ClientsDto, ServerDto, ServersDto.
- `src/web/routes/collect.rs` — добавить `collect_clients(filters)` и `collect_servers(filters)`.
- `src/web/routes/mod.rs` — `pub mod clients; pub mod servers; pub mod query;`.
- `src/web/server.rs` — `route_api` парсит query string и передаёт в новые handlers.
- `src/web/tests.rs` — integration tests на оба endpoint'а.

---

## Task 0: Baseline проверка

**Files:** none modified.

- [ ] **Step 0.1: Зафиксировать чистое состояние ветки**

```bash
cd /home/vadv/Projects/pg_doorman
git status
git log --oneline -3
```
Expected: HEAD = `14ca557`, working tree clean.

- [ ] **Step 0.2: Зафиксировать baseline**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 674 passed, clippy clean, fmt clean.

---

## Task 1: расширить ClientStats — state_since_nanos + age accessors

**Files:**
- Modify: `src/stats/client.rs`.

- [ ] **Step 1.1: Добавить поле**

В структуре `ClientStats` (примерно после строки `pub error_count: AtomicU64`) добавить:

```rust
    /// Nanoseconds elapsed since `connect_time` at the moment of the latest
    /// state transition (set by `set_state`/`set_wait`/`set_state_wait`).
    /// Used by `current_query_age_ms()` and `wait_ms()` accessors.
    pub state_since_nanos: AtomicU64,
```

- [ ] **Step 1.2: Initialize в `ClientStats::new`**

В функции `pub fn new(...) -> Self`, в struct literal initialise добавить:

```rust
            state_since_nanos: AtomicU64::new(0),
```

- [ ] **Step 1.3: helper `nanos_from_connect`**

Добавить inline-helper:

```rust
    #[inline]
    fn nanos_from_connect(&self) -> u64 {
        self.connect_time.elapsed().as_nanos() as u64
    }
```

- [ ] **Step 1.4: hook в `set_state`**

В `pub fn set_state(&self, state: u8) { ... }` (около строки 190), перед `state_atomic.store(state, ...)`, добавить:

```rust
        self.state_since_nanos
            .store(self.nanos_from_connect(), Ordering::Relaxed);
```

То же самое в `set_wait` и `set_state_wait` — обновлять timestamp на каждом state/wait transition.

- [ ] **Step 1.5: accessor `current_query_age_ms`**

Добавить метод:

```rust
    /// Returns the milliseconds elapsed since this client entered ACTIVE state.
    /// Returns `None` when the client is not currently ACTIVE.
    #[inline]
    pub fn current_query_age_ms(&self) -> Option<u64> {
        // CLIENT_STATE_ACTIVE — see the constants used by set_state callers.
        // Here we accept any value reported as "active" by state_to_string().
        if self.state_to_string() != "active" {
            return None;
        }
        let since = self.state_since_nanos.load(Ordering::Relaxed);
        if since == 0 {
            return None;
        }
        Some(self.nanos_from_connect().saturating_sub(since) / 1_000_000)
    }
```

(If there's a public `CLIENT_STATE_ACTIVE` constant already exported, prefer comparing `self.state() == CLIENT_STATE_ACTIVE` instead of the string compare. Read `src/stats/client.rs` near `set_state` callers to find the constant.)

- [ ] **Step 1.6: accessor `wait_ms`**

```rust
    /// Returns the milliseconds elapsed since this client entered WAITING state.
    /// Returns `None` when the client is not currently waiting.
    #[inline]
    pub fn wait_ms(&self) -> Option<u64> {
        if self.state_to_string() != "waiting" {
            return None;
        }
        let since = self.state_since_nanos.load(Ordering::Relaxed);
        if since == 0 {
            return None;
        }
        Some(self.nanos_from_connect().saturating_sub(since) / 1_000_000)
    }
```

- [ ] **Step 1.7: добавить unit-тест**

В существующем `#[cfg(test)] mod tests` (есть в файле — line ~540) добавить тест что `current_query_age_ms` возвращает `None` для нового клиента, и `Some(0..)` после `active_*` (если такие helpers есть). Если slot transition в тестах сложнее — покрыть только базовый «не active → None».

```rust
    #[test]
    fn current_query_age_ms_none_when_not_active() {
        let stats = ClientStats::new(/* построить как в существующих тестах */);
        assert_eq!(stats.current_query_age_ms(), None);
        assert_eq!(stats.wait_ms(), None);
    }
```

(Скопировать конструктор из соседних тестов — существующая `let now = clock::now();` etc.)

- [ ] **Step 1.8: cargo build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib stats::client 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -5
cargo fmt --check 2>&1 | tail -3
```
Expected: clean build, тесты passing.

- [ ] **Step 1.9: Не коммитить.**

---

## Task 2: ClientDto / ServerDto + collect functions

**Files:**
- Modify: `src/web/routes/dto.rs`, `src/web/routes/collect.rs`.

- [ ] **Step 2.1: Добавить DTOs в `dto.rs`**

После `PoolDto`:

```rust
#[derive(Debug, Serialize)]
pub struct ClientsDto {
    pub ts: u64,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
    pub clients: Vec<ClientDto>,
}

#[derive(Debug, Serialize)]
pub struct ClientDto {
    pub client_id: String,           // "#c12345"
    pub database: String,
    pub user: String,
    pub application_name: String,
    pub addr: String,                // "10.1.2.3:54321"
    pub tls: bool,
    pub state: String,               // "active" | "idle" | "waiting" | ...
    pub wait: String,                // "none" | "lock" | ...
    pub wait_ms: u64,                // 0 if not waiting
    pub transactions_total: u64,
    pub queries_total: u64,
    pub errors_total: u64,
    pub age_seconds: u64,
    pub current_query_age_ms: u64,   // 0 if not active
}

#[derive(Debug, Serialize)]
pub struct ServersDto {
    pub ts: u64,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
    pub servers: Vec<ServerDto>,
}

#[derive(Debug, Serialize)]
pub struct ServerDto {
    pub server_id: i32,
    pub process_id: i32,
    pub database: String,
    pub user: String,
    pub application_name: String,
    pub tls: bool,
    pub state: String,
    pub wait: String,
    pub age_seconds: u64,
    pub active_age_ms: u64,          // 0 if not active
    pub transactions_total: u64,
    pub queries_total: u64,
    pub errors_total: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub prepared_hits_total: u64,
    pub prepared_misses_total: u64,
    pub prepared_cache_size: u64,
}
```

- [ ] **Step 2.2: Добавить ClientFilters / ServerFilters structs**

В `dto.rs` (или в `query.rs` — на ваш выбор; решение: пусть filters живут рядом с DTO, потому что они часть API contract):

```rust
#[derive(Debug, Default, Clone)]
pub struct ClientFilters {
    pub limit: u64,
    pub offset: u64,
    pub sort: ClientSort,
    pub order: SortOrder,
    pub pool: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub application_name: Vec<String>,
    pub state: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum ClientSort {
    #[default]
    QueriesTotal,
    ErrorsTotal,
    AgeSeconds,
    CurrentQueryAgeMs,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum SortOrder {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Default, Clone)]
pub struct ServerFilters {
    pub limit: u64,
    pub offset: u64,
    pub sort: ServerSort,
    pub order: SortOrder,
    pub pool: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum ServerSort {
    #[default]
    AgeSeconds,
    QueriesTotal,
    ErrorsTotal,
    ActiveAgeMs,
}
```

- [ ] **Step 2.3: collect_clients и collect_servers**

В `collect.rs` добавить:

```rust
use crate::web::routes::dto::{
    ClientDto, ClientFilters, ClientSort, ClientsDto, ServerDto, ServerFilters, ServerSort,
    ServersDto, SortOrder,
};

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 1000;

pub fn collect_clients(filters: &ClientFilters) -> ClientsDto {
    let stats_map = get_client_stats();
    let pools_map = get_all_pools();

    let mut rows: Vec<ClientDto> = stats_map
        .values()
        .filter(|s| {
            let pool_name = s.pool_name();
            let user = s.username();
            let app = s.application_name();
            let state = s.state_to_string();
            // Pool filter is by composite "user@db" id (consistency with PoolDto.id).
            // Pool-name field on ClientStats is "db" only; combine to match.
            // For phase 3b we filter against pool_name == database, since
            // ClientStats.pool_name() returns only the database part. UI uses
            // either ?database= or ?user= for narrower filtering.
            if let Some(p) = &filters.pool {
                let id = format!("{}@{}", user, pool_name);
                if id != *p {
                    return false;
                }
            }
            if let Some(db) = &filters.database {
                if pool_name != *db {
                    return false;
                }
            }
            if let Some(u) = &filters.user {
                if user != *u {
                    return false;
                }
            }
            if !filters.application_name.is_empty() && !filters.application_name.contains(&app) {
                return false;
            }
            if !filters.state.is_empty() && !filters.state.contains(&state) {
                return false;
            }
            true
        })
        .map(|s| {
            let age_seconds = s.connect_time().elapsed().as_secs();
            ClientDto {
                client_id: format!("#c{}", s.connection_id()),
                database: s.pool_name(),
                user: s.username(),
                application_name: s.application_name(),
                addr: s.ipaddr(),
                tls: s.tls(),
                state: s.state_to_string(),
                wait: s.wait_to_string(),
                wait_ms: s.wait_ms().unwrap_or(0),
                transactions_total: s.transaction_count.load(std::sync::atomic::Ordering::Relaxed),
                queries_total: s.query_count.load(std::sync::atomic::Ordering::Relaxed),
                errors_total: s.error_count.load(std::sync::atomic::Ordering::Relaxed),
                age_seconds,
                current_query_age_ms: s.current_query_age_ms().unwrap_or(0),
            }
        })
        .collect();

    let total = rows.len() as u64;

    // Sort
    rows.sort_by(|a, b| {
        let ord = match filters.sort {
            ClientSort::QueriesTotal => a.queries_total.cmp(&b.queries_total),
            ClientSort::ErrorsTotal => a.errors_total.cmp(&b.errors_total),
            ClientSort::AgeSeconds => a.age_seconds.cmp(&b.age_seconds),
            ClientSort::CurrentQueryAgeMs => a.current_query_age_ms.cmp(&b.current_query_age_ms),
        };
        match filters.order {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        }
    });

    let limit = filters.limit.clamp(1, MAX_LIMIT);
    let offset = filters.offset;
    let page: Vec<_> = rows
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    let _ = pools_map; // pools_map currently unused but reserved for future enrichments.

    ClientsDto {
        ts: now_unix_ms(),
        total,
        limit,
        offset,
        clients: page,
    }
}

pub fn collect_servers(filters: &ServerFilters) -> ServersDto {
    let stats_map = get_server_stats();

    let mut rows: Vec<ServerDto> = stats_map
        .values()
        .filter(|s| {
            let pool_name = s.pool_name();
            let user = s.username();
            if let Some(p) = &filters.pool {
                let id = format!("{}@{}", user, pool_name);
                if id != *p {
                    return false;
                }
            }
            if let Some(db) = &filters.database {
                if pool_name != *db {
                    return false;
                }
            }
            if let Some(u) = &filters.user {
                if user != *u {
                    return false;
                }
            }
            true
        })
        .map(|s| {
            let age_seconds = s.connect_time().elapsed().as_secs();
            ServerDto {
                server_id: s.server_id(),
                process_id: s.process_id(),
                database: s.pool_name(),
                user: s.username(),
                application_name: s.application_name(),
                tls: s.tls(),
                state: s.state_to_string(),
                wait: s.wait_to_string(),
                age_seconds,
                active_age_ms: s.active_age_ms().unwrap_or(0),
                transactions_total: s.transaction_count.load(std::sync::atomic::Ordering::Relaxed),
                queries_total: s.query_count.load(std::sync::atomic::Ordering::Relaxed),
                errors_total: s.error_count.load(std::sync::atomic::Ordering::Relaxed),
                bytes_sent: s.bytes_sent.load(std::sync::atomic::Ordering::Relaxed),
                bytes_received: s.bytes_received.load(std::sync::atomic::Ordering::Relaxed),
                prepared_hits_total: s.prepared_hit_count.load(std::sync::atomic::Ordering::Relaxed),
                prepared_misses_total: s.prepared_miss_count.load(std::sync::atomic::Ordering::Relaxed),
                prepared_cache_size: s.prepared_cache_size.load(std::sync::atomic::Ordering::Relaxed),
            }
        })
        .collect();

    let total = rows.len() as u64;

    rows.sort_by(|a, b| {
        let ord = match filters.sort {
            ServerSort::AgeSeconds => a.age_seconds.cmp(&b.age_seconds),
            ServerSort::QueriesTotal => a.queries_total.cmp(&b.queries_total),
            ServerSort::ErrorsTotal => a.errors_total.cmp(&b.errors_total),
            ServerSort::ActiveAgeMs => a.active_age_ms.cmp(&b.active_age_ms),
        };
        match filters.order {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        }
    });

    let limit = filters.limit.clamp(1, MAX_LIMIT);
    let offset = filters.offset;
    let page: Vec<_> = rows
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    ServersDto {
        ts: now_unix_ms(),
        total,
        limit,
        offset,
        servers: page,
    }
}
```

**Adapt:** verify accessor имена (`error_count` vs `errors`, `transaction_count` vs `transactions`, etc.) — ClientStats fields names from Task 1. Если ServerStats не имеет `error_count` — возвращать 0 placeholder (verify первым).

- [ ] **Step 2.4: cargo build**

```bash
cargo build --lib 2>&1 | tail -10
```
Expected: clean.

- [ ] **Step 2.5: clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 2.6: Не коммитить.**

---

## Task 3: query string parser + handlers

**Files:**
- Create: `src/web/routes/query.rs`.
- Create: `src/web/routes/clients.rs`.
- Create: `src/web/routes/servers.rs`.
- Modify: `src/web/routes/mod.rs`.

- [ ] **Step 3.1: Создать `src/web/routes/query.rs`**

```rust
//! Hand-rolled query string parser.
//!
//! Returns `BTreeMap<key, Vec<value>>` so multi-value keys (e.g.
//! `?application_name=a&application_name=b`) are preserved in order.
//! Keeps the dependency surface small (no `serde_urlencoded`).

use std::collections::BTreeMap;

pub fn parse_query(q: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if q.is_empty() {
        return out;
    }
    for part in q.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = match part.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(part), String::new()),
        };
        out.entry(k).or_default().push(v);
    }
    out
}

fn decode(s: &str) -> String {
    // Minimal URL decoding: %XX hex pairs and '+' → space. We don't accept
    // anything fancier (no `;` separators, no UTF-8 multibyte percent-encoded).
    // Sufficient for typical filter values like "myapp@v3" or "main@db1".
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '+' => out.push(' '),
            '%' => {
                let hi = chars.next();
                let lo = chars.next();
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    if let (Some(hi), Some(lo)) = (hi.to_digit(16), lo.to_digit(16)) {
                        out.push(((hi << 4 | lo) as u8) as char);
                        continue;
                    }
                }
                // Malformed — drop the percent prefix silently.
            }
            other => out.push(other),
        }
    }
    out
}

pub fn first(map: &BTreeMap<String, Vec<String>>, key: &str) -> Option<String> {
    map.get(key).and_then(|v| v.first()).cloned()
}

pub fn parse_u64(map: &BTreeMap<String, Vec<String>>, key: &str, default: u64) -> u64 {
    first(map, key).and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_empty_map() {
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn single_value() {
        let m = parse_query("limit=50");
        assert_eq!(m.get("limit"), Some(&vec!["50".to_string()]));
    }

    #[test]
    fn multiple_values_for_same_key() {
        let m = parse_query("application_name=a&application_name=b");
        assert_eq!(
            m.get("application_name"),
            Some(&vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn percent_decoding() {
        let m = parse_query("user=alice%40example");
        assert_eq!(m.get("user"), Some(&vec!["alice@example".to_string()]));
    }

    #[test]
    fn plus_to_space() {
        let m = parse_query("application_name=my+app");
        assert_eq!(
            m.get("application_name"),
            Some(&vec!["my app".to_string()])
        );
    }

    #[test]
    fn parse_u64_with_default() {
        let m = parse_query("limit=42");
        assert_eq!(parse_u64(&m, "limit", 100), 42);
        assert_eq!(parse_u64(&m, "missing", 100), 100);
    }
}
```

- [ ] **Step 3.2: Создать `src/web/routes/clients.rs`**

```rust
//! GET /api/clients handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_clients;
use crate::web::routes::dto::{ClientFilters, ClientSort, SortOrder};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_clients(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = parse_filters(query);
    Response::ok_json(&collect_clients(&filters))
}

fn parse_filters(query: &BTreeMap<String, Vec<String>>) -> ClientFilters {
    ClientFilters {
        limit: parse_u64(query, "limit", 100),
        offset: parse_u64(query, "offset", 0),
        sort: match first(query, "sort").as_deref() {
            Some("errors_total") => ClientSort::ErrorsTotal,
            Some("age_seconds") => ClientSort::AgeSeconds,
            Some("current_query_age_ms") => ClientSort::CurrentQueryAgeMs,
            _ => ClientSort::QueriesTotal,
        },
        order: match first(query, "order").as_deref() {
            Some("asc") => SortOrder::Asc,
            _ => SortOrder::Desc,
        },
        pool: first(query, "pool"),
        database: first(query, "database"),
        user: first(query, "user"),
        application_name: query
            .get("application_name")
            .cloned()
            .unwrap_or_default(),
        state: query.get("state").cloned().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_response_is_200_json_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_clients(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"total\"",
            "\"limit\"",
            "\"offset\"",
            "\"clients\"",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }

    #[test]
    fn parse_filters_picks_up_query_params() {
        let mut q = BTreeMap::new();
        q.insert("limit".into(), vec!["50".into()]);
        q.insert("sort".into(), vec!["errors_total".into()]);
        q.insert("order".into(), vec!["asc".into()]);
        q.insert("pool".into(), vec!["main@db1".into()]);
        let f = parse_filters(&q);
        assert_eq!(f.limit, 50);
        assert!(matches!(f.sort, ClientSort::ErrorsTotal));
        assert!(matches!(f.order, SortOrder::Asc));
        assert_eq!(f.pool.as_deref(), Some("main@db1"));
    }
}
```

- [ ] **Step 3.3: Создать `src/web/routes/servers.rs`**

```rust
//! GET /api/servers handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_servers;
use crate::web::routes::dto::{ServerFilters, ServerSort, SortOrder};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_servers(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = parse_filters(query);
    Response::ok_json(&collect_servers(&filters))
}

fn parse_filters(query: &BTreeMap<String, Vec<String>>) -> ServerFilters {
    ServerFilters {
        limit: parse_u64(query, "limit", 100),
        offset: parse_u64(query, "offset", 0),
        sort: match first(query, "sort").as_deref() {
            Some("queries_total") => ServerSort::QueriesTotal,
            Some("errors_total") => ServerSort::ErrorsTotal,
            Some("active_age_ms") => ServerSort::ActiveAgeMs,
            _ => ServerSort::AgeSeconds,
        },
        order: match first(query, "order").as_deref() {
            Some("asc") => SortOrder::Asc,
            _ => SortOrder::Desc,
        },
        pool: first(query, "pool"),
        database: first(query, "database"),
        user: first(query, "user"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_response_is_200_json_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_servers(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"total\"",
            "\"limit\"",
            "\"offset\"",
            "\"servers\"",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }
}
```

- [ ] **Step 3.4: `src/web/routes/mod.rs` — добавить новые modules**

В `src/web/routes/mod.rs` добавить:

```rust
pub mod clients;
pub mod query;
pub mod servers;
```

(в alphabetic position).

- [ ] **Step 3.5: cargo build + test для новых модулей**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib web::routes::query:: 2>&1 | tail -10
cargo test --lib web::routes::clients:: 2>&1 | tail -10
cargo test --lib web::routes::servers:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 6 query tests passed, 2 clients tests passed, 1 server test passed, clippy/fmt clean.

- [ ] **Step 3.6: Не коммитить.**

---

## Task 4: интеграция в mux

**Files:**
- Modify: `src/web/server.rs`.

- [ ] **Step 4.1: Обновить `route_api`**

Найти `fn route_api` в `src/web/server.rs`. Сейчас он принимает только `req: &ParsedRequest<'_>` и парсит path с `split('?')`. Изменить чтобы передавать query string в handlers:

```rust
fn route_api(req: &ParsedRequest<'_>) -> Response {
    use crate::web::routes;
    use crate::web::routes::query::parse_query;

    let (path, query_str) = match req.path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (req.path, ""),
    };
    let query = parse_query(query_str);

    match path {
        "/api/version" => routes::version::handle_version(),
        "/api/overview" => routes::overview::handle_overview(),
        "/api/pools" => routes::pools::handle_pools(),
        "/api/clients" => routes::clients::handle_clients(&query),
        "/api/servers" => routes::servers::handle_servers(&query),
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
}
```

Старые handlers (version/overview/pools) остаются без аргументов.

- [ ] **Step 4.2: Добавить unit-тесты в `src/web/server.rs`**

```rust
    #[test]
    fn dispatch_clients_returns_200() {
        let r = dispatch(
            &req("GET", "/api/clients"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_clients_with_query_params_returns_200() {
        let r = dispatch(
            &req("GET", "/api/clients?limit=10&sort=errors_total"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_servers_returns_200() {
        let r = dispatch(
            &req("GET", "/api/servers"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }
```

- [ ] **Step 4.3: Прогон тестов**

```bash
cargo test --lib web::server::tests:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 21 server tests passed (18 phase 3a + 3 new). clippy/fmt clean.

- [ ] **Step 4.4: Не коммитить.**

---

## Task 5: integration tests

**Files:**
- Modify: `src/web/tests.rs`.

- [ ] **Step 5.1: Добавить integration-тесты**

```rust
#[tokio::test]
async fn api_clients_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/clients HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"clients\""), "raw={raw}");
    assert!(raw.contains("\"total\""), "raw={raw}");
    assert!(raw.contains("\"limit\":100"), "raw={raw}");
    assert!(raw.contains("\"offset\":0"), "raw={raw}");
}

#[tokio::test]
async fn api_clients_with_query_params() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/clients?limit=50&offset=10&sort=age_seconds&order=asc HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"limit\":50"), "raw={raw}");
    assert!(raw.contains("\"offset\":10"), "raw={raw}");
}

#[tokio::test]
async fn api_servers_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/servers HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"servers\""), "raw={raw}");
}
```

- [ ] **Step 5.2: Полный прогон**

```bash
cargo test --lib 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~692 passed (674 baseline + 6 query + 2 clients + 1 servers + 1 client_stats + 3 server unit + 3 integration). Точное число подтверждаем при прогоне.

- [ ] **Step 5.3: Не коммитить.**

---

## Task 6: Final-проверка + commit

- [ ] **Step 6.1: cargo fmt + clippy + test**

```bash
cargo fmt
cargo clippy --lib -- --deny warnings 2>&1 | tail -5
cargo test --lib 2>&1 | tail -3
```

- [ ] **Step 6.2: Smoke check release build**

```bash
cargo build --release 2>&1 | tail -3
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3b.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/clients ---"
curl -s 'http://127.0.0.1:19127/api/clients?limit=10' | head -c 600
echo ""
echo "--- /api/clients with sort ---"
curl -s 'http://127.0.0.1:19127/api/clients?sort=errors_total&order=desc&limit=5' | head -c 400
echo ""
echo "--- /api/servers ---"
curl -s 'http://127.0.0.1:19127/api/servers' | head -c 500
echo ""
echo "--- /api/clients?pool=u@smoke (filter) ---"
curl -s 'http://127.0.0.1:19127/api/clients?pool=u%40smoke' | head -c 300
echo ""
kill $DPID
wait $DPID 2>/dev/null
```
Expected: оба endpoint'а отдают валидный JSON, query params работают, percent-encoding в pool filter работает.

- [ ] **Step 6.3: Pre-commit code review**

Через Agent (правило CLAUDE.md). Черновик commit-сообщения:

```
feat(web): land /api/clients and /api/servers with filter, sort, pagination

Operators inspecting a busy pooler can now hit /api/clients and
/api/servers, narrow the result by pool, database, user, application
name, or state, and page through the response with ?limit and ?offset.
The default sort is the most useful one for triage: clients ordered by
queries_total desc, servers by connection age desc.

To make the per-client age signals real, ClientStats now records the
nanoseconds-from-connect of the latest state transition. wait_ms and
current_query_age_ms surface that as observable JSON fields, mirroring
what ServerStats has already had via active_age_ms.

Server-side filter and pagination are a pragmatic step away from the
"thin server" rule (decision #21): a busy pooler may have thousands of
clients and shipping that volume across HTTP every 1.5s is wasteful.
The frontend keeps doing client-side substring search on the visible
window.

Tests: <N> passed (was 674), clippy and fmt clean. Verified by
release-build smoke against both endpoints with filter, sort, and
percent-encoded values.

Phase 3b of seven; phase 3c lands ConfigState routes.
```

Если ревью даёт блокеры — fix, repeat.

- [ ] **Step 6.4: Создать коммит**

```bash
git add -A
git status
git commit -m "$(cat <<'EOF'
<finalised commit message>
EOF
)"
git log --oneline -3
git status
```

---

## Self-review

**Spec coverage check:**
- ✅ /api/clients shape (раздел 8.5) — Tasks 1, 2, 3.
- ✅ /api/servers shape — Tasks 1, 2, 3.
- ✅ Filter/sort/pagination params — Tasks 2, 3, 4.
- ✅ Multi-value query params (`application_name`, `state`) — Tasks 2, 3.
- ✅ Percent-encoding в query — Tasks 3.
- ✅ Backend extension для wait_ms / current_query_age_ms — Task 1.

**Не покрыто этой фазой:**
- /api/connections, /api/stats, etc. — phase 3c.
- Top-N — phase 3d.
- Drawer for server detail per-pool — frontend phase 6.

**Type-consistency check:**
- `ClientDto`, `ServerDto` поля имена совпадают со спекой.
- Filter struct field имена совпадают с handler parsers.
- `SortOrder` shared between client/server filters.

**Placeholder check:** не используем «placeholder phase 3e» pattern. Backend gap для state_started_at реально решён в Task 1, не отложен.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-3b.md`.

Subagent-driven execution; controller dispatches one implementer per task plus two-stage review.
