# Web UI — Phase 3c-1 Implementation Plan: 4 ConfigState list endpoints

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Поднять `GET /api/connections`, `/api/stats`, `/api/databases`, `/api/users` — четыре простых list endpoint'а для ConfigState-страницы. Никаких filter/sort/pagination: данные тут — конфигурационные либо агрегатные счётчики, объёмы небольшие (пулов десятки, не тысячи).

**Architecture:** Каждый endpoint читает глобальное состояние (atomic counters либо `get_all_pools()` / `PoolStats::construct_pool_lookup()`) и собирает плоский DTO. Нет фильтров, нет передачи query string в handler. Все handlers — тривиальные обёртки над `collect_*()` в `collect.rs` (как `handle_overview`).

**Field naming:** Мы зеркалим существующие admin SHOW-команды (`show_connections`, `show_stats`, `show_databases`, `show_users`) один-в-один по составу полей. Это сознательный выбор:
- Операторы знакомы со SHOW-выводами PgBouncer/pg_doorman.
- Нет необходимости держать два имени для одного поля.
- frontend на странице ConfigState может рендерить DTO через generic table component без mapping-слоя.

Один отступ от mirroring: в `/api/stats` добавляем поле `id` (`<user>@<database>`) для удобной корреляции с `/api/pools` (где `id` уже есть). В остальном — те же имена и единицы что в `SHOW STATS`. Time-поля в `/api/stats` — микросекунды (как в backend); это документируется в doc-комментарии DTO.

**Tech Stack:** Тот же что в phase 3a/3b. Никаких новых deps.

**Reference:**
- Spec: `2026-05-06-web-ui-design.md`, раздел 8.2 (full endpoint list, semantics), раздел 8.1 (общие принципы — flat JSON, `ts: u64`, нет envelope wrapper'а), декрет #21 (thin server).
- Phase 3b commit: `e1999c2`.
- Существующие admin SHOW: `src/admin/show.rs:381` (databases), `:466` (stats), `:532` (connections), `:632` (users).

**Не входит в фазу 3c-1:**
- `/api/config` (с маскированием секретов), `/api/log_level`, `/api/auth_query`, `/api/pool_scaling`, `/api/pool_coordinator`, `/api/sockets` — это phase 3c-2.
- `/api/prepared`, `/api/interner`, admin stubs `/api/prepared/text/{hash}`, `/api/interner/top` — phase 3c-3.
- Top-N (`/api/top/*`), `/api/apps`, `/api/events` — phase 3d.

---

## File Structure

**Новые файлы:**
- `src/web/routes/connections.rs` — handler `GET /api/connections`.
- `src/web/routes/stats.rs` — handler `GET /api/stats`.
- `src/web/routes/databases.rs` — handler `GET /api/databases`.
- `src/web/routes/users.rs` — handler `GET /api/users`.

**Модифицируемые файлы:**
- `src/web/routes/dto.rs` — `ConnectionsDto`, `StatsDto`, `StatsRowDto`, `DatabasesDto`, `DatabaseDto`, `UsersDto`, `UserDto`.
- `src/web/routes/collect.rs` — `collect_connections`, `collect_stats`, `collect_databases`, `collect_users`. Pure helper `connections_from_raw` для unit-теста.
- `src/web/routes/mod.rs` — `pub(crate) mod {connections, stats, databases, users};`.
- `src/web/server.rs` — четыре arm'а в `route_api` + четыре dispatch-теста.
- `src/web/tests.rs` — четыре integration-теста.

---

## Task 0: Baseline проверка

**Files:** none modified.

- [ ] **Step 0.1: Зафиксировать чистое состояние ветки**

```bash
cd /home/vadv/Projects/pg_doorman
git status
git log --oneline -3
```
Expected: HEAD = `e1999c2`, working tree clean (untracked files допустимы — `.local/`, `Dockerfile.ubuntu22-tls`, `INCIDENT_*.md` существовали до фазы 3b и не относятся к плану).

- [ ] **Step 0.2: Зафиксировать baseline тестов**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 715 passed, clippy clean, fmt clean.

---

## Task 1: добавить DTOs в `dto.rs`

**Files:**
- Modify: `src/web/routes/dto.rs`.

- [ ] **Step 1.1: Добавить `ConnectionsDto`**

Положить **после** `ServerSort` enum (в конец файла, либо рядом с другими `*Dto`):

```rust
/// `GET /api/connections` — cumulative connection counters.
///
/// `errors` is derived as `total - tls - plain - cancel` to mirror the
/// existing `SHOW CONNECTIONS` admin output exactly. Operators reading the
/// REST API see the same values they saw via the admin protocol.
#[derive(Debug, Serialize)]
pub struct ConnectionsDto {
    pub ts: u64,
    pub total: u64,
    pub tls: u64,
    pub plain: u64,
    pub cancel: u64,
    pub errors: u64,
}
```

- [ ] **Step 1.2: Добавить `StatsDto` + `StatsRowDto`**

```rust
/// `GET /api/stats` — per-pool aggregated counters.
///
/// Field names mirror `SHOW STATS` columns. Time fields (`*_xact_time`,
/// `*_query_time`, `*_wait_time`) are microseconds, matching the units stored
/// in `PoolStats`. Frontend converts to milliseconds for display.
#[derive(Debug, Serialize)]
pub struct StatsDto {
    pub ts: u64,
    pub stats: Vec<StatsRowDto>,
}

#[derive(Debug, Serialize)]
pub struct StatsRowDto {
    /// Stable identifier `<user>@<database>`, matches `PoolDto.id`.
    pub id: String,
    pub database: String,
    pub user: String,
    pub total_xact_count: u64,
    pub total_query_count: u64,
    pub total_received: u64,
    pub total_sent: u64,
    pub total_xact_time: u64,
    pub total_query_time: u64,
    pub total_wait_time: u64,
    pub total_errors: u64,
    pub avg_xact_count: u64,
    pub avg_query_count: u64,
    pub avg_recv: u64,
    pub avg_sent: u64,
    pub avg_errors: u64,
    pub avg_xact_time: u64,
    pub avg_query_time: u64,
    pub avg_wait_time: u64,
}
```

- [ ] **Step 1.3: Добавить `DatabasesDto` + `DatabaseDto`**

```rust
/// `GET /api/databases` — configured database/pool entries.
/// Field names mirror `SHOW DATABASES` columns.
#[derive(Debug, Serialize)]
pub struct DatabasesDto {
    pub ts: u64,
    pub databases: Vec<DatabaseDto>,
}

#[derive(Debug, Serialize)]
pub struct DatabaseDto {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub force_user: String,
    pub pool_size: u32,
    pub min_pool_size: u32,
    /// Hardcoded zero — pg_doorman does not implement reserve_pool but the
    /// column exists in `SHOW DATABASES` for protocol compatibility. Kept
    /// here so consumers parsing the REST API see the same shape.
    pub reserve_pool: u32,
    pub pool_mode: String,
    pub max_connections: u32,
    pub current_connections: u32,
}
```

- [ ] **Step 1.4: Добавить `UsersDto` + `UserDto`**

```rust
/// `GET /api/users` — list of configured users.
///
/// One row per `(user, database)` pair from the pool registry. Mirrors
/// `SHOW USERS`: same user appearing in multiple databases yields multiple
/// rows (the admin command did not deduplicate).
#[derive(Debug, Serialize)]
pub struct UsersDto {
    pub ts: u64,
    pub users: Vec<UserDto>,
}

#[derive(Debug, Serialize)]
pub struct UserDto {
    pub name: String,
    pub pool_mode: String,
}
```

- [ ] **Step 1.5: cargo build + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 1.6: Не коммитить.**

---

## Task 2: collect-функции + unit-тесты

**Files:**
- Modify: `src/stats/pool.rs` (visibility bump for 6 fields).
- Modify: `src/web/routes/collect.rs`.

- [ ] **Step 2.0: Поднять видимость 6 полей `PoolStats`**

В `src/stats/pool.rs` следующие поля сейчас `pub(self)` (без модификатора). `collect_stats()` живёт в другом модуле и не сможет их читать. Меняем на `pub`, чтобы соответствовать остальным `total_*` / `avg_*` полям в этой же структуре:

```rust
    /// Total bytes received from clients
    pub total_received: u64,           // line 131

    /// Total bytes sent to clients
    pub total_sent: u64,               // line 134

    /// Average bytes received per second
    pub avg_recv: u64,                 // line 172

    /// Average bytes sent per second
    pub avg_sent: u64,                 // line 175

    /// Average transaction processing time (microseconds)
    pub avg_xact_time_microsecons: u64,    // line 178 — typo "microsecons" сохраняется как было

    /// Average query processing time (microseconds)
    pub avg_query_time_microseconds: u64,  // line 181
```

**Это единственное изменение в `src/stats/pool.rs`.** Все остальные правки — внутри `src/web/`.

- [ ] **Step 2.1: Расширить use-импорты**

В верхней use-секции добавить:

```rust
use crate::web::routes::dto::{
    ConnectionsDto, DatabaseDto, DatabasesDto, StatsDto, StatsRowDto, UserDto, UsersDto,
    // ... existing entries
};
```

- [ ] **Step 2.2: `collect_connections` + pure helper**

Добавить в `collect.rs` (после `collect_overview` либо в конец, перед `#[cfg(test)]`):

```rust
pub fn collect_connections() -> ConnectionsDto {
    connections_from_raw(
        cnt(&TOTAL_CONNECTION_COUNTER),
        cnt(&TLS_CONNECTION_COUNTER),
        cnt(&PLAIN_CONNECTION_COUNTER),
        cnt(&CANCEL_CONNECTION_COUNTER),
    )
}

/// Builds a `ConnectionsDto` from raw counter values. Pure function — exists
/// so the `errors = total - tls - plain - cancel` derivation is exercised by
/// unit tests without touching the global atomics.
fn connections_from_raw(total: u64, tls: u64, plain: u64, cancel: u64) -> ConnectionsDto {
    ConnectionsDto {
        ts: now_unix_ms(),
        total,
        tls,
        plain,
        cancel,
        // `errors` mirrors `SHOW CONNECTIONS`: it is whatever is left after
        // subtracting the categorised counters from the total. May be zero or
        // positive in normal operation.
        errors: total.saturating_sub(tls).saturating_sub(plain).saturating_sub(cancel),
    }
}
```

**Note:** существующий `show_connections` использует `total - tls - plain - cancel` без `saturating_sub`. Мы используем `saturating_sub` как защиту от теоретического race (counter'ы инкрементируются независимо; в очень редкие моменты сумма категорий может на 1-2 опередить `total`). Без этого guard'а возможен underflow в `u64`, который покажется огромным числом и испугает оператора. Это единственное намеренное расхождение с admin-вариантом.

- [ ] **Step 2.3: `collect_stats`**

```rust
pub fn collect_stats() -> StatsDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let mut stats: Vec<StatsRowDto> = pool_lookup
        .iter()
        .map(|(identifier, s)| StatsRowDto {
            id: format!("{}@{}", identifier.user, identifier.db),
            database: identifier.db.clone(),
            user: identifier.user.clone(),
            total_xact_count: s.total_xact_count,
            total_query_count: s.total_query_count,
            total_received: s.total_received,
            total_sent: s.total_sent,
            total_xact_time: s.total_xact_time_microseconds,
            total_query_time: s.total_query_time_microseconds,
            total_wait_time: s.wait_time,
            total_errors: s.errors,
            avg_xact_count: s.avg_xact_count,
            avg_query_count: s.avg_query_count,
            avg_recv: s.avg_recv,
            avg_sent: s.avg_sent,
            avg_errors: s.errors,
            avg_xact_time: s.avg_xact_time_microsecons,
            avg_query_time: s.avg_query_time_microseconds,
            avg_wait_time: s.avg_wait_time,
        })
        .collect();

    // Stable order: same `id` ordering as `/api/pools` for deterministic UI.
    stats.sort_by(|a, b| a.id.cmp(&b.id));

    StatsDto {
        ts: now_unix_ms(),
        stats,
    }
}
```

**Verify:** `avg_errors` в существующем `generate_show_stats_row` — это `self.errors`, не `self.avg_errors`. Это **известная странность** (`PoolStats` не хранит per-window error rate). Зеркалим как есть; добавление настоящего avg-errors counter'а — отдельная задача (см. spec section 16 nice-to-have).

- [ ] **Step 2.4: `collect_databases`**

```rust
pub fn collect_databases() -> DatabasesDto {
    let pools_map = get_all_pools();
    let mut databases: Vec<DatabaseDto> = pools_map
        .iter()
        .map(|(_identifier, pool)| {
            let address = pool.address();
            let settings = &pool.settings;
            DatabaseDto {
                name: address.name(),
                host: address.host.clone(),
                port: address.port,
                database: address.database.clone(),
                force_user: settings.user.username.clone(),
                pool_size: settings.user.pool_size,
                min_pool_size: settings.user.min_pool_size.unwrap_or(0),
                // pg_doorman does not honour reserve_pool yet — see DTO doc.
                reserve_pool: 0,
                pool_mode: settings.pool_mode.to_string(),
                max_connections: settings.user.pool_size,
                // pool_state().size is `usize`; safe `as u32` cast since pool_size is u32 anyway.
                current_connections: pool.pool_state().size as u32,
            }
        })
        .collect();

    // Deterministic order using the `<user>@<db>` composite key.
    databases.sort_by(|a, b| a.name.cmp(&b.name));

    DatabasesDto {
        ts: now_unix_ms(),
        databases,
    }
}
```

**Verify type compatibility:** `address.name()` возвращает `String` (probably uses `cached_name`). Если `&str` — добавить `.to_string()`.

**Verify** что `pool.pool_state().size` возвращает целое число (`u32` либо `usize`). Если разный тип — приведение `as u32`.

- [ ] **Step 2.5: `collect_users`**

```rust
pub fn collect_users() -> UsersDto {
    let pools_map = get_all_pools();
    let mut users: Vec<UserDto> = pools_map
        .iter()
        .map(|(identifier, pool)| UserDto {
            name: identifier.user.clone(),
            pool_mode: pool.settings.pool_mode.to_string(),
        })
        .collect();

    users.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.pool_mode.cmp(&b.pool_mode)));

    UsersDto {
        ts: now_unix_ms(),
        users,
    }
}
```

**Note:** sort by `name` then `pool_mode` — даёт deterministic order при одинаковом user в разных пулах с разными pool_mode'ами (редкий, но валидный сценарий).

- [ ] **Step 2.6: Unit-тест на `connections_from_raw`**

В существующий `#[cfg(test)] mod tests` (он уже есть в `collect.rs`, line ~373) добавить:

```rust
    // ---------------------------------------------------------------------------
    // ConnectionsDto math
    // ---------------------------------------------------------------------------

    #[test]
    fn connections_errors_derive_from_total_minus_categorised() {
        let dto = super::connections_from_raw(100, 60, 30, 5);
        assert_eq!(dto.total, 100);
        assert_eq!(dto.tls, 60);
        assert_eq!(dto.plain, 30);
        assert_eq!(dto.cancel, 5);
        assert_eq!(dto.errors, 5);
    }

    #[test]
    fn connections_errors_zero_when_categories_cover_total() {
        let dto = super::connections_from_raw(50, 30, 15, 5);
        assert_eq!(dto.errors, 0);
    }

    #[test]
    fn connections_errors_saturate_when_categories_exceed_total() {
        // Race: categorised counters momentarily ahead of total.
        // Without saturating_sub this would underflow into u64::MAX.
        let dto = super::connections_from_raw(10, 8, 5, 0);
        assert_eq!(dto.errors, 0);
    }
```

- [ ] **Step 2.7: cargo build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib web::routes::collect:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 27 collect tests passing (24 baseline + 3 connections_*), clean.

- [ ] **Step 2.8: Не коммитить.**

---

## Task 3: handlers (по одному файлу на endpoint)

**Files:**
- Create: `src/web/routes/connections.rs`, `stats.rs`, `databases.rs`, `users.rs`.
- Modify: `src/web/routes/mod.rs`.

- [ ] **Step 3.1: `connections.rs`**

```rust
//! GET /api/connections handler.

use crate::web::routes::collect::collect_connections;
use crate::web::server::Response;

pub(crate) fn handle_connections() -> Response {
    Response::ok_json(&collect_connections())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_response_is_200_with_envelope() {
        let r = handle_connections();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        for field in [
            "\"ts\"",
            "\"total\"",
            "\"tls\"",
            "\"plain\"",
            "\"cancel\"",
            "\"errors\"",
        ] {
            assert!(body.contains(field), "missing {field} in {body}");
        }
    }
}
```

- [ ] **Step 3.2: `stats.rs`**

```rust
//! GET /api/stats handler.

use crate::web::routes::collect::collect_stats;
use crate::web::server::Response;

pub(crate) fn handle_stats() -> Response {
    Response::ok_json(&collect_stats())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_response_is_200_with_envelope() {
        let r = handle_stats();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"stats\""));
    }
}
```

- [ ] **Step 3.3: `databases.rs`**

```rust
//! GET /api/databases handler.

use crate::web::routes::collect::collect_databases;
use crate::web::server::Response;

pub(crate) fn handle_databases() -> Response {
    Response::ok_json(&collect_databases())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databases_response_is_200_with_envelope() {
        let r = handle_databases();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"databases\""));
    }
}
```

- [ ] **Step 3.4: `users.rs`**

```rust
//! GET /api/users handler.

use crate::web::routes::collect::collect_users;
use crate::web::server::Response;

pub(crate) fn handle_users() -> Response {
    Response::ok_json(&collect_users())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_response_is_200_with_envelope() {
        let r = handle_users();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"users\""));
    }
}
```

- [ ] **Step 3.5: `mod.rs` — register modules**

В `src/web/routes/mod.rs` обновить (alphabetical order):

```rust
pub mod collect;
pub mod dto;

pub(crate) mod clients;
pub(crate) mod connections;
pub(crate) mod databases;
pub(crate) mod overview;
pub(crate) mod pools;
pub(crate) mod query;
pub(crate) mod servers;
pub(crate) mod stats;
pub(crate) mod users;
pub(crate) mod version;
```

- [ ] **Step 3.6: cargo build + test для новых модулей**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib web::routes::connections:: web::routes::stats:: web::routes::databases:: web::routes::users:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 4 handler-тестa passing, clippy/fmt clean.

- [ ] **Step 3.7: Не коммитить.**

---

## Task 4: интеграция в mux + dispatch-тесты

**Files:**
- Modify: `src/web/server.rs`.

- [ ] **Step 4.1: Расширить `route_api`**

В функции `route_api` добавить четыре arm'а (alphabetical после `/api/clients`):

```rust
    match path {
        "/api/version" => routes::version::handle_version(),
        "/api/overview" => routes::overview::handle_overview(),
        "/api/pools" => routes::pools::handle_pools(),
        "/api/clients" => routes::clients::handle_clients(&query),
        "/api/connections" => routes::connections::handle_connections(),
        "/api/databases" => routes::databases::handle_databases(),
        "/api/servers" => routes::servers::handle_servers(&query),
        "/api/stats" => routes::stats::handle_stats(),
        "/api/users" => routes::users::handle_users(),
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
```

- [ ] **Step 4.2: Добавить dispatch-тесты в `src/web/server.rs::tests`**

В существующий `mod tests` добавить четыре теста (после `dispatch_servers_returns_200`):

```rust
    #[test]
    fn dispatch_connections_returns_200() {
        let r = dispatch(
            &req("GET", "/api/connections"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_stats_returns_200() {
        let r = dispatch(
            &req("GET", "/api/stats"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_databases_returns_200() {
        let r = dispatch(
            &req("GET", "/api/databases"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_users_returns_200() {
        let r = dispatch(
            &req("GET", "/api/users"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }
```

- [ ] **Step 4.3: Прогон тестов**

```bash
cargo test --lib web::server:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 25 server-тестов (21 baseline + 4 new), clippy/fmt clean.

- [ ] **Step 4.4: Не коммитить.**

---

## Task 5: integration tests

**Files:**
- Modify: `src/web/tests.rs`.

- [ ] **Step 5.1: Добавить четыре integration-теста**

В конец `src/web/tests.rs` добавить:

```rust
#[tokio::test]
async fn api_connections_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/connections HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    for field in ["\"ts\"", "\"total\"", "\"tls\"", "\"plain\"", "\"cancel\"", "\"errors\""] {
        assert!(raw.contains(field), "missing {field} in {raw}");
    }
}

#[tokio::test]
async fn api_stats_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/stats HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"stats\""), "raw={raw}");
}

#[tokio::test]
async fn api_databases_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/databases HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"databases\""), "raw={raw}");
}

#[tokio::test]
async fn api_users_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/users HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"users\""), "raw={raw}");
}
```

- [ ] **Step 5.2: Полный прогон**

```bash
cargo test --lib 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~727 passed (715 baseline + 4 handler unit + 4 dispatch + 3 connections_from_raw + 1 ignored). Точное число подтверждаем при прогоне.

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

Используем тот же config что в фазах 3a/3b (`/tmp/doorman-phase3a.toml`).

```bash
cargo build --release 2>&1 | tail -3
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3c1.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/connections ---"
curl -s 'http://127.0.0.1:19127/api/connections' | head -c 400
echo ""
echo "--- /api/stats ---"
curl -s 'http://127.0.0.1:19127/api/stats' | head -c 600
echo ""
echo "--- /api/databases ---"
curl -s 'http://127.0.0.1:19127/api/databases' | head -c 600
echo ""
echo "--- /api/users ---"
curl -s 'http://127.0.0.1:19127/api/users' | head -c 400
echo ""
kill $DPID
wait $DPID 2>/dev/null
```
Expected: каждый endpoint отвечает `200`, JSON содержит ожидаемые поля. Если pg_doorman не успел установить connection'ы — `total/tls/plain/cancel/errors` могут быть нулями, это нормально.

- [ ] **Step 6.3: Pre-commit code review**

Через Agent (правило CLAUDE.md). Черновик commit-сообщения:

```
feat(web): land /api/connections /api/stats /api/databases /api/users

ConfigState page in the upcoming Web UI needs a per-tab data source for
connection counters, per-pool stats, configured databases, and configured
users. This commit wires the four list endpoints. Field names mirror the
SHOW CONNECTIONS / SHOW STATS / SHOW DATABASES / SHOW USERS admin
columns one-to-one so operators recognise the values.

The shapes are flat lists with a `ts` timestamp; no filter, sort, or
pagination — these are configuration and aggregate views, not the
client/server lists where volume justified server-side query handling
in phase 3b.

The only intentional deviation from SHOW CONNECTIONS is `errors` being
computed via `saturating_sub` rather than wrapping subtraction. The
counters update independently and the categorised sum can momentarily
exceed `total`; saturating arithmetic prevents a transient u64 underflow
from surfacing as a frighteningly large number on the dashboard.

Tests: <N> passed (was 715), clippy and fmt clean. Verified by release
binary smoke-tests against all four endpoints.

Phase 3c-1 of seven; phase 3c-2 lands the remaining ConfigState routes
(config with masking, log_level, auth_query, pool_scaling,
pool_coordinator, sockets).
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
- ✅ `/api/connections` shape (раздел 8.2 «Cumulative counters total/tls/plain/cancel/errors») — Tasks 1, 2, 3.
- ✅ `/api/stats` shape (раздел 8.2 «Per-pool xact/query/wait counters») — Tasks 1, 2, 3.
- ✅ `/api/databases` shape (раздел 8.2 «Конфиг database entries») — Tasks 1, 2, 3.
- ✅ `/api/users` shape (раздел 8.2 «Список пользователей») — Tasks 1, 2, 3.
- ✅ `ts: u64` обязательное поле — все DTO.
- ✅ flat JSON без envelope wrapper'а — все DTO напрямую сериализуются (нет wrapping struct'а).
- ✅ Public access (decision #21 + auth matrix) — handlers не требуют admin-auth, mux уже обрабатывает `ui_anonymous`.

**Не покрыто этой фазой:**
- `/api/config`, `/api/log_level`, `/api/auth_query`, `/api/pool_scaling`, `/api/pool_coordinator`, `/api/sockets` — phase 3c-2.
- Frontend ConfigState page — phase 6.
- BDD scenarios для public ConfigState routes — phase 7.

**Type-consistency check:**
- Все `*Dto` поля — `String`, `u64`, `u32`, `u16`, `bool` (стандартные serde-типы).
- Sort order стабилен и detereministic — `databases` по `name`, `stats` по `id`, `users` по `(name, pool_mode)`.
- Field имена в DTO совпадают с admin SHOW columns (за исключением `id` в `StatsRowDto` — добавлен для UI correlation).

**Placeholder check:** Нет полей с заглушками типа `epoch: 0` или `errors: 0`. Все значения берутся из реальных backend источников. Единственное hardcoded поле — `reserve_pool: 0` в `DatabaseDto`, что зеркалит существующую `show_databases` поведение и документировано в DTO doc-комментарии.

**Consistency со спекой:**
- Decision #21 (thin server, fat client) соблюдён: backend отдаёт raw counters/config; никаких computed fields, rate'ов, severity, threshold-классификаций.
- Раздел 8.1 общие принципы: flat JSON, `ts: <unix_ms>`, нет error envelope для 200 — соблюдено.
- Раздел 8.2 access matrix: все четыре endpoint'а в группе public — handlers без auth-проверок (mux обрабатывает `ui_anonymous`).

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-3c-1.md`.

Subagent-driven execution; controller dispatches one implementer per task plus two-stage review.
