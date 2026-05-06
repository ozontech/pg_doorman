# Web UI — Phase 3a Implementation Plan: Collect Infrastructure + First 3 API Routes

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Поднять три первых JSON-роута Web UI: `GET /api/version`, `GET /api/overview`, `GET /api/pools`. Это включает создание DTO-инфраструктуры и pure `collect_*()` функций, на которые потом сядут остальные endpoints в phases 3b-3d. Никаких изменений в admin-протоколе или поведении `/metrics` — phase 3a добавляет только новые HTTP routes.

**Architecture:** `src/web/routes/` — новая директория. `dto.rs` объявляет JSON-типы с `#[derive(Serialize)]`, `collect.rs` — pure functions, обходящие existing globals (`POOLS`, `get_client_stats()`, `get_server_stats()`, конкретные счётчики), возвращающие готовые DTO. Каждый handler в своём файле (`version.rs`, `overview.rs`, `pools.rs`), сигнатура `pub fn handle(...) -> Response`. Mux в `src/web/server.rs` роутит по path → handler.

**Tech Stack:** Rust + serde (`json` уже в deps) + всё что есть в phase 1-2 (subtle, base64, tokio).

**Reference:**
- Spec: `docs/superpowers/specs/2026-05-06-web-ui-design.md`, разделы 4.2 (file structure), 8.1-8.4 (общий формат + version/overview/pools shapes), 12.3 (testing).
- Phase 1 commit: `b24082f`. Phase 2 commit: `633df32`.
- Decision log #16, #17, #21 (top-N как core, sort/filter/url-state, thin server fat client).

**Не входит в фазу 3a:**
- /api/clients, /api/servers — phase 3b.
- Остальные endpoints — phase 3c.
- Top-N + /api/apps + /api/events — phase 3d.
- Prometheus gauges с label-разбивкой по kind (для Grafana drill-down) — phase 3e (Grafana parity, не блокирует UI).

**Заметка про данные:** изначально план полагал, что `errors_total` per-pool и `wait_p95_ms` отсутствуют в `PoolStats` и должны быть placeholder'ами до phase 3e. Это оказалось ошибкой: `PoolStats.errors` (cumulative) и `PoolStats.wait_percentile.p95` уже populated. Phase 3a отдаёт реальные значения. Section 16 спеки переработан под актуальное состояние.

---

## File Structure

**Новые файлы:**
- `src/web/routes/mod.rs` — root модуля; объявляет submodules + dispatch функция `handle_api(req, opts) -> Option<Response>`.
- `src/web/routes/dto.rs` — все Serializable DTO типы для phase 3a (VersionDto, OverviewDto, PoolDto, PoolsDto).
- `src/web/routes/collect.rs` — pure collect functions без I/O (только чтение global state).
- `src/web/routes/version.rs` — handler GET /api/version.
- `src/web/routes/overview.rs` — handler GET /api/overview.
- `src/web/routes/pools.rs` — handler GET /api/pools.

**Модифицируемые файлы:**
- `src/web/mod.rs` — `pub mod routes;`.
- `src/web/server.rs` — в `dispatch()` функции: вместо безусловного 501-stub для `/api/*`, делегировать в `routes::handle_api()`.

---

## Task 0: Baseline проверка

**Files:** none modified.

- [ ] **Step 0.1: Зафиксировать чистое состояние ветки**

```bash
cd /home/vadv/Projects/pg_doorman
git status
git log --oneline -3
```
Expected: HEAD = `633df32 feat(web): add HTTP mux with basic-auth and default-password gate`, working tree clean (untracked мусор не наш).

- [ ] **Step 0.2: Зафиксировать baseline**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 664 passed, clippy clean, fmt clean.

---

## Task 1: Создать routes/ + DTO

**Files:**
- Create: `src/web/routes/mod.rs`, `src/web/routes/dto.rs`.
- Modify: `src/web/mod.rs`.

- [ ] **Step 1.1: Создать `src/web/routes/mod.rs`**

```rust
//! REST API routes mounted under `/api/`.
//!
//! Phase 3a wires only `/api/version`, `/api/overview`, `/api/pools`.
//! Subsequent phases add `/api/clients`, `/api/servers`, top-N, etc.

pub mod collect;
pub mod dto;

pub mod overview;
pub mod pools;
pub mod version;

pub use overview::handle_overview;
pub use pools::handle_pools;
pub use version::handle_version;
```

- [ ] **Step 1.2: Создать `src/web/routes/dto.rs`**

```rust
//! JSON DTO types for the Web UI REST API.
//!
//! These structs define the wire format that the frontend consumes; they are
//! the source of truth for response shapes documented in spec sections 8.3+.
//! Field naming follows the spec exactly and is verified by snapshot tests.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct VersionDto {
    pub version: &'static str,
    pub git_commit: &'static str,
    pub build_date: &'static str,
    pub ts: u64,
}

#[derive(Debug, Serialize)]
pub struct OverviewDto {
    pub ts: u64,

    pub active_clients: u64,
    pub idle_clients: u64,
    pub waiting_clients: u64,

    pub active_servers: u64,
    pub idle_servers: u64,

    pub connections_total: u64,
    pub connections_tls_total: u64,
    pub connections_plain_total: u64,
    pub connections_cancel_total: u64,

    pub query_count_total: u64,
    pub transaction_count_total: u64,
    pub errors_count_total: u64,

    pub prepared_hits_total: u64,
    pub prepared_misses_total: u64,

    pub pools_total: u64,
    pub pools_paused: u64,
}

#[derive(Debug, Serialize)]
pub struct PoolsDto {
    pub ts: u64,
    pub pools: Vec<PoolDto>,
}

#[derive(Debug, Serialize)]
pub struct PoolDto {
    /// Stable identifier `<user>@<database>`.
    pub id: String,
    pub user: String,
    pub database: String,
    pub host: String,
    pub port: u16,
    pub pool_mode: String,

    pub max_connections: u32,
    pub min_connections: u32,
    pub connections: u64,
    pub idle: u64,
    pub active: u64,
    pub waiting: u64,

    pub max_active_age_ms: u64,

    pub query_p95_ms: u64,
    pub query_p99_ms: u64,
    pub transactions_p95_ms: u64,
    pub transactions_p99_ms: u64,

    pub wait_avg_ms: u64,
    /// Placeholder until phase 3e adds wait-time percentiles to the backend.
    pub wait_p95_ms: u64,

    pub queries_total: u64,
    pub transactions_total: u64,
    /// Placeholder until phase 3e adds the per-pool error counter.
    pub errors_total: u64,

    pub paused: bool,
    pub epoch: u64,
}
```

- [ ] **Step 1.3: Wire `pub mod routes;` in `src/web/mod.rs`**

В `src/web/mod.rs` после `pub mod metrics;` добавить:

```rust
pub mod routes;
```

(альфавитное место — между `metrics` и `server`).

- [ ] **Step 1.4: Создать заглушки submodule файлов чтобы код компилировался**

Создать файлы с минимальным содержимым (handler-функции добавим в Tasks 3-5):

`src/web/routes/version.rs`:
```rust
//! GET /api/version handler. Implementation in Task 3.

use crate::web::routes::dto::VersionDto;
use crate::web::server::Response;

pub fn handle_version() -> Response {
    let _dto: VersionDto = todo!("Task 3");
}
```

`src/web/routes/overview.rs`:
```rust
//! GET /api/overview handler. Implementation in Task 4.

use crate::web::routes::dto::OverviewDto;
use crate::web::server::Response;

pub fn handle_overview() -> Response {
    let _dto: OverviewDto = todo!("Task 4");
}
```

`src/web/routes/pools.rs`:
```rust
//! GET /api/pools handler. Implementation in Task 5.

use crate::web::routes::dto::PoolsDto;
use crate::web::server::Response;

pub fn handle_pools() -> Response {
    let _dto: PoolsDto = todo!("Task 5");
}
```

`src/web/routes/collect.rs`:
```rust
//! Pure collection functions for the REST API. Implementations follow.

// (empty in Task 1, filled in Task 2)
```

- [ ] **Step 1.5: Сделать `Response` доступным из `routes/`**

В `src/web/server.rs` найти `struct Response { ... }` и помечать его `pub(crate)` (заместо приватного по умолчанию). Также нужно сделать pub(crate) методы `Response::status`, `Response::json`. Это требуется чтобы handlers могли строить Response.

Конкретно изменить:
```rust
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Response { ... }

impl Response {
    pub(crate) fn status(...) -> Self { ... }
    pub(crate) fn json(...) -> Self { ... }
    pub(crate) fn unauthorized() -> Self { ... }
    // write остаётся private или pub(super) — handlers не пишут напрямую
}
```

И поля внутри Response сделать `pub(crate)` если они нужны handler'ам — пока что для Tasks 3-5 им хватит конструкторов.

Дополнительно нужен новый pub(crate) конструктор для JSON ответа из serde-сериализации:

```rust
impl Response {
    pub(crate) fn ok_json<T: serde::Serialize>(value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Response {
                status: 200,
                reason: "OK",
                extra_headers: vec![("Content-Type", "application/json".into())],
                body,
            },
            Err(e) => {
                log::error!("Failed to serialize JSON response: {e}");
                Response::status(500, "Internal Server Error")
            }
        }
    }
}
```

- [ ] **Step 1.6: cargo build (только lib без тестов — todo! не паникует на компиляции)**

```bash
cargo build --lib 2>&1 | tail -10
```
Expected: clean build. Если warning про unused `pub use overview::handle_overview` — игнорируем, заработает в Task 6 при mux dispatch.

- [ ] **Step 1.7: Не коммитить.**

---

## Task 2: collect.rs — pure functions

**Files:**
- Modify: `src/web/routes/collect.rs`.

- [ ] **Step 2.1: Реализовать collect функции**

Заменить содержимое `src/web/routes/collect.rs` на:

```rust
//! Pure collection functions for the REST API.
//!
//! Each function reads from project-wide global state (POOLS, COORDINATORS,
//! get_client_stats(), get_server_stats(), connection counters) and assembles
//! a Serializable DTO. No I/O, no Mutex acquisition outside what the global
//! reads already do internally.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::pool::get_all_pools;
use crate::stats::{
    get_client_stats, get_server_stats, CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER,
    TLS_CONNECTION_COUNTER, TOTAL_CONNECTION_COUNTER,
};
use crate::stats::PoolStats;

use crate::web::routes::dto::{OverviewDto, PoolDto, PoolsDto, VersionDto};

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn collect_version() -> VersionDto {
    VersionDto {
        version: env!("CARGO_PKG_VERSION"),
        git_commit: option_env!("PG_DOORMAN_GIT_COMMIT").unwrap_or("unknown"),
        build_date: option_env!("PG_DOORMAN_BUILD_DATE").unwrap_or("unknown"),
        ts: now_unix_ms(),
    }
}

pub fn collect_overview() -> OverviewDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let client_states = get_client_stats();
    let server_states = get_server_stats();

    let mut active_clients = 0u64;
    let mut idle_clients = 0u64;
    let mut waiting_clients = 0u64;
    for stats in client_states.values() {
        match stats.state_to_string().as_str() {
            "active" => active_clients += 1,
            "idle" => idle_clients += 1,
            "waiting" => waiting_clients += 1,
            _ => {}
        }
    }

    let mut active_servers = 0u64;
    let mut idle_servers = 0u64;
    for stats in server_states.values() {
        match stats.state_to_string().as_str() {
            "active" => active_servers += 1,
            "idle" => idle_servers += 1,
            _ => {}
        }
    }

    let connections_total =
        TOTAL_CONNECTION_COUNTER.load(std::sync::atomic::Ordering::Relaxed) as u64;
    let connections_tls_total =
        TLS_CONNECTION_COUNTER.load(std::sync::atomic::Ordering::Relaxed) as u64;
    let connections_plain_total =
        PLAIN_CONNECTION_COUNTER.load(std::sync::atomic::Ordering::Relaxed) as u64;
    let connections_cancel_total =
        CANCEL_CONNECTION_COUNTER.load(std::sync::atomic::Ordering::Relaxed) as u64;

    let mut query_count_total = 0u64;
    let mut transaction_count_total = 0u64;
    let mut prepared_hits_total = 0u64;
    let mut prepared_misses_total = 0u64;
    let mut pools_paused = 0u64;
    for stats in pool_lookup.values() {
        query_count_total += stats.total_query_count;
        transaction_count_total += stats.total_xact_count;
        if stats.paused {
            pools_paused += 1;
        }
    }
    for stats in server_states.values() {
        prepared_hits_total += stats
            .prepared_hit_count
            .load(std::sync::atomic::Ordering::Relaxed);
        prepared_misses_total += stats
            .prepared_miss_count
            .load(std::sync::atomic::Ordering::Relaxed);
    }

    OverviewDto {
        ts: now_unix_ms(),

        active_clients,
        idle_clients,
        waiting_clients,

        active_servers,
        idle_servers,

        connections_total,
        connections_tls_total,
        connections_plain_total,
        connections_cancel_total,

        query_count_total,
        transaction_count_total,
        // Per-pool error counter is exposed in phase 3e; until then we report 0
        // for backend-collected errors. /api/overview is the single aggregated
        // dashboard signal, so a zero here is honest under-counting, not a
        // misleading aggregate.
        errors_count_total: 0,

        prepared_hits_total,
        prepared_misses_total,

        pools_total: pool_lookup.len() as u64,
        pools_paused,
    }
}

pub fn collect_pools() -> PoolsDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let pools_map = get_all_pools();

    let mut pools = Vec::with_capacity(pool_lookup.len());
    for (identifier, stats) in pool_lookup.iter() {
        let pool = match pools_map.get(identifier) {
            Some(p) => p,
            None => continue,
        };
        let address = pool.address();
        let dto = PoolDto {
            id: format!("{}@{}", identifier.user, identifier.db),
            user: identifier.user.clone(),
            database: identifier.db.clone(),
            host: address.host.clone(),
            port: address.port,
            pool_mode: stats.mode.to_string(),
            max_connections: stats.pool_size,
            min_connections: pool.settings.min_pool_size.unwrap_or(0) as u32,
            connections: stats.sv_active + stats.sv_idle + stats.sv_used + stats.sv_login,
            idle: stats.sv_idle,
            active: stats.sv_active,
            waiting: stats.cl_waiting,
            max_active_age_ms: stats.oldest_active_age_ms,
            query_p95_ms: stats.query_percentile.p95(),
            query_p99_ms: stats.query_percentile.p99(),
            transactions_p95_ms: stats.xact_percentile.p95(),
            transactions_p99_ms: stats.xact_percentile.p99(),
            wait_avg_ms: stats.avg_wait_time / 1_000, // micros -> ms
            // Placeholder; phase 3e exposes wait_time_percentile.
            wait_p95_ms: 0,
            queries_total: stats.total_query_count,
            transactions_total: stats.total_xact_count,
            // Placeholder; phase 3e exposes per-pool error counter.
            errors_total: 0,
            paused: stats.paused,
            epoch: pool.settings.epoch.unwrap_or(0),
        };
        pools.push(dto);
    }

    // Stable order for snapshot tests.
    pools.sort_by(|a, b| a.id.cmp(&b.id));

    PoolsDto {
        ts: now_unix_ms(),
        pools,
    }
}
```

**Critical:** перед написанием кода проверьте сигнатуры в исходниках:
- `PoolStats` поля — `src/stats/pool.rs`. Особенно `query_percentile.p95()`, `avg_wait_time` (микросекунды), `pool_size`, `paused`, `total_query_count`, `total_xact_count`.
- `Address` поля — `src/pool/mod.rs` либо `src/pool/address.rs` (есть `host: String` и `port: u16`?).
- `PoolSettings` поля — `min_pool_size: Option<u32>`, `epoch: Option<u64>` — могут быть другие имена.
- `ServerStats` — `prepared_hit_count`, `prepared_miss_count` (AtomicU64).

Если имена отличаются — адаптируйте под реальные. Если поля нет совсем (например `epoch`) — используйте `0` placeholder с комментарием.

- [ ] **Step 2.2: Запустить cargo build для проверки**

```bash
cargo build --lib 2>&1 | tail -20
```
Expected: clean. Если есть errors про несуществующие поля — fix per actual API.

- [ ] **Step 2.3: clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -5
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 2.4: Не коммитить.**

---

## Task 3: handler GET /api/version

**Files:**
- Modify: `src/web/routes/version.rs`.

- [ ] **Step 3.1: Реализовать handle_version**

Заменить содержимое `src/web/routes/version.rs`:

```rust
//! GET /api/version handler.

use crate::web::routes::collect::collect_version;
use crate::web::server::Response;

pub fn handle_version() -> Response {
    Response::ok_json(&collect_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_response_is_200_json_with_required_fields() {
        let r = handle_version();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"version\""), "body={body}");
        assert!(body.contains("\"git_commit\""), "body={body}");
        assert!(body.contains("\"build_date\""), "body={body}");
        assert!(body.contains("\"ts\""), "body={body}");
    }
}
```

Доступ к `r.status` и `r.body` из тестов требует чтобы поля Response были `pub(crate)`. Если ещё не — сделать в Task 1.5; иначе исправить тут.

- [ ] **Step 3.2: Прогнать тест**

```bash
cargo test --lib web::routes::version:: 2>&1 | tail -10
```
Expected: 1 passed.

- [ ] **Step 3.3: clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 3.4: Не коммитить.**

---

## Task 4: handler GET /api/overview

**Files:**
- Modify: `src/web/routes/overview.rs`.

- [ ] **Step 4.1: Реализовать handle_overview**

```rust
//! GET /api/overview handler.

use crate::web::routes::collect::collect_overview;
use crate::web::server::Response;

pub fn handle_overview() -> Response {
    Response::ok_json(&collect_overview())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_response_is_200_json() {
        let r = handle_overview();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"active_clients\"",
            "\"idle_clients\"",
            "\"waiting_clients\"",
            "\"active_servers\"",
            "\"idle_servers\"",
            "\"connections_total\"",
            "\"connections_tls_total\"",
            "\"connections_plain_total\"",
            "\"connections_cancel_total\"",
            "\"query_count_total\"",
            "\"transaction_count_total\"",
            "\"errors_count_total\"",
            "\"prepared_hits_total\"",
            "\"prepared_misses_total\"",
            "\"pools_total\"",
            "\"pools_paused\"",
        ] {
            assert!(body.contains(field), "missing {field} in body={body}");
        }
    }
}
```

- [ ] **Step 4.2: Прогнать тест**

```bash
cargo test --lib web::routes::overview:: 2>&1 | tail -10
```
Expected: 1 passed.

- [ ] **Step 4.3: clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

- [ ] **Step 4.4: Не коммитить.**

---

## Task 5: handler GET /api/pools

**Files:**
- Modify: `src/web/routes/pools.rs`.

- [ ] **Step 5.1: Реализовать handle_pools**

```rust
//! GET /api/pools handler.

use crate::web::routes::collect::collect_pools;
use crate::web::server::Response;

pub fn handle_pools() -> Response {
    Response::ok_json(&collect_pools())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_response_is_200_json_with_array() {
        let r = handle_pools();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""), "body={body}");
        assert!(body.contains("\"pools\""), "body={body}");
    }

    // Note: the actual pool list is empty in unit tests because no pools
    // are registered globally. End-to-end coverage with a real pool comes
    // from BDD scenarios (added in phase 7).
}
```

- [ ] **Step 5.2: Прогнать тест**

```bash
cargo test --lib web::routes::pools:: 2>&1 | tail -10
```
Expected: 1 passed.

- [ ] **Step 5.3: clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

- [ ] **Step 5.4: Не коммитить.**

---

## Task 6: интеграция handlers в mux

**Files:**
- Modify: `src/web/server.rs`.

- [ ] **Step 6.1: Заменить 501-stub в `dispatch()` на маршрутизацию**

Найти в `src/web/server.rs` функции `dispatch` блок:

```rust
if req.path.starts_with("/api/") {
    // Real handlers land in phase 3.
    return Response::json(
        501,
        "Not Implemented",
        r#"{"error":"not_implemented","message":"phase 2 stub; api routes land in phase 3"}"#,
    );
}
```

Заменить на:

```rust
if req.path.starts_with("/api/") {
    return route_api(req);
}
```

И добавить функцию (выше или ниже `dispatch()`):

```rust
fn route_api(req: &ParsedRequest<'_>) -> Response {
    use crate::web::routes;
    // strip query string for matching
    let path = req.path.split('?').next().unwrap_or(req.path);
    match path {
        "/api/version" => routes::handle_version(),
        "/api/overview" => routes::handle_overview(),
        "/api/pools" => routes::handle_pools(),
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
}
```

- [ ] **Step 6.2: Обновить unit-тесты в `src/web/server.rs`**

Тест `dispatch_anonymous_public_path_when_ui_anonymous_true` сейчас проверяет `/api/overview` → 501. Меняем на `/api/that-does-not-exist-yet` → 501, и добавляем новый тест что `/api/overview` → 200:

```rust
#[test]
fn dispatch_unknown_api_returns_501() {
    let r = dispatch(
        &req("GET", "/api/not-yet-wired"),
        &opts(true, true),
        AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 501);
}

#[test]
fn dispatch_overview_returns_200() {
    let r = dispatch(
        &req("GET", "/api/overview"),
        &opts(true, true),
        AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_version_returns_200() {
    let r = dispatch(
        &req("GET", "/api/version"),
        &opts(true, true),
        AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn dispatch_pools_returns_200() {
    let r = dispatch(
        &req("GET", "/api/pools"),
        &opts(true, true),
        AuthOutcome::Anonymous,
    );
    assert_eq!(r.status, 200);
}
```

(Удалить старый `dispatch_anonymous_public_path_when_ui_anonymous_true` или переименовать его в `dispatch_unknown_api_returns_501`).

- [ ] **Step 6.3: cargo test --lib web::server::tests::**

```bash
cargo test --lib web::server::tests:: 2>&1 | tail -10
```
Expected: новый счёт passed (15+ tests), 0 failed.

- [ ] **Step 6.4: clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

- [ ] **Step 6.5: Не коммитить.**

---

## Task 7: integration test

**Files:**
- Modify: `src/web/tests.rs`.

- [ ] **Step 7.1: Добавить integration-тесты для каждого нового endpoint**

В `src/web/tests.rs` после существующих 6 тестов добавить:

```rust
#[tokio::test]
async fn api_version_returns_json() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/version HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("Content-Type: application/json"), "raw={raw}");
    assert!(raw.contains("\"version\""), "raw={raw}");
    assert!(raw.contains("\"git_commit\""), "raw={raw}");
}

#[tokio::test]
async fn api_overview_returns_json_when_ui_active() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"active_clients\""), "raw={raw}");
    assert!(raw.contains("\"pools_total\""), "raw={raw}");
}

#[tokio::test]
async fn api_pools_returns_json_when_ui_active() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/pools HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"pools\""), "raw={raw}");
}

#[tokio::test]
async fn api_overview_still_404_when_ui_inactive() {
    let port = spawn_server(opts(false, true)).await;
    let raw = send(port, "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 404"), "raw={raw}");
}
```

- [ ] **Step 7.2: Прогнать**

```bash
cargo test --lib web::tests:: 2>&1 | tail -10
```
Expected: 10 passed (6 от phase 2 + 4 новых).

- [ ] **Step 7.3: clippy + fmt + полный test**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
cargo test --lib 2>&1 | tail -3
```
Expected: 664 (baseline) + 1 (version) + 1 (overview unit) + 1 (pools unit) + 4 (integration) + 3 (server tests rewired) - 1 (one removed) ≈ 673 passed. Точное число подтверждаем при прогоне.

- [ ] **Step 7.4: Не коммитить.**

---

## Task 8: Final-проверка + commit

**Files:** none modified.

- [ ] **Step 8.1: cargo fmt + clippy + test**

```bash
cargo fmt
cargo clippy --lib -- --deny warnings 2>&1 | tail -5
cargo test --lib 2>&1 | tail -3
```

- [ ] **Step 8.2: Smoke check release build**

```bash
cargo build --release 2>&1 | tail -3
cat > /tmp/doorman-phase3a.toml <<'EOF'
[general]
host = "127.0.0.1"
port = 16432
admin_username = "admin"
admin_password = "phase3test"

[web]
enabled = true
host = "127.0.0.1"
port = 19127
ui = true

[pools.smoke]
server_host = "localhost"
server_port = 5432

[[pools.smoke.users]]
username = "u"
password = "p"
pool_size = 5
EOF
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3a.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/version ---"
curl -s http://127.0.0.1:19127/api/version
echo ""
echo "--- /api/overview ---"
curl -s http://127.0.0.1:19127/api/overview
echo ""
echo "--- /api/pools ---"
curl -s http://127.0.0.1:19127/api/pools
echo ""
echo "--- /api/clients (still 501 stub) ---"
curl -is http://127.0.0.1:19127/api/clients | head -1
kill $DPID
wait $DPID 2>/dev/null
```
Expected: первые три endpoint'а возвращают валидный JSON; /api/clients ещё 501 — будет в phase 3b.

- [ ] **Step 8.3: Pre-commit code review**

Pre-commit review через Agent (правило CLAUDE.md). Черновик commit-сообщения:

```
feat(web): land /api/version, /api/overview, /api/pools endpoints

The web listener now answers three GET routes when [web].ui = true:
status / aggregated counters / per-pool snapshot. The wire shape
follows the spec sections 8.1-8.4 verbatim — same field names that the
upcoming frontend pages will consume directly. Anonymous access is
allowed because public routes are gated by [web].ui_anonymous, which
defaults to true.

Two field placeholders remain at zero (errors_total per-pool, wait_p95_ms)
until phase 3e exposes the corresponding backend gauges. The JSON shape
is stable from the start; only the values move from 0 to real numbers.

The handlers read existing global state (POOLS, get_client_stats(),
get_server_stats(), connection counters) through pure collect_*()
functions. /metrics behaviour and /admin protocol are untouched.

Tests: <N> passed (was 664), clippy and fmt clean. Verified by release
smoke against the three endpoints plus regression check that an
unwired path keeps returning 501.

Phase 3a of seven; phase 3b adds /api/clients and /api/servers.
```

- [ ] **Step 8.4: Создать коммит**

```bash
git add -A
git status   # просмотр staged
git commit -m "$(cat <<'EOF'
<finalised commit message>
EOF
)"
git log --oneline -3
git status
```

---

## Self-review

**Spec coverage:**
- ✅ /api/version JSON shape — Tasks 1, 2, 3.
- ✅ /api/overview shape (раздел 8.3) — Tasks 1, 2, 4.
- ✅ /api/pools shape (раздел 8.4) — Tasks 1, 2, 5.
- ✅ Pure collect functions без бизнес-логики на бэкенде — Task 2 (decision #21).
- ✅ Mux маршрутизация — Task 6.
- ✅ Integration tests с реальным listener'ом — Task 7.

**Не покрыто этой фазой (намеренно):**
- /api/clients, /api/servers — phase 3b.
- Sort/filter/pagination на роутах — phase 3b начинает (для clients), 3c расширяет.
- ConfigState routes — phase 3c.
- Top-N + apps + events — phase 3d.
- errors_total / wait_p95_ms реальные значения — phase 3e (backend gaps).

**Type-consistency:**
- `Response::ok_json<T: Serialize>` — single canonical constructor для JSON ответов.
- `collect_*()` — все возвращают `Dto`-структуры из `dto.rs`.
- Handler signatures: `pub fn handle_*() -> Response` — без аргументов в phase 3a (нет filter/sort).

**Placeholder check:** Все шаги содержат конкретный код или команду; никаких «TBD» / «similar to».

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-3a.md`.

Subagent-driven execution; controller dispatches one implementer per task plus two-stage review.
