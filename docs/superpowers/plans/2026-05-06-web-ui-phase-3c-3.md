# Web UI — Phase 3c-3 Implementation Plan: prepared / interner caches + admin stubs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Поднять последние 4 endpoint'а ConfigState/Caches: `/api/prepared` (public, агрегат без текстов), `/api/interner` (public, агрегат без preview), `/api/prepared/text/{hash}` (admin-only, тело конкретного prepared statement по hash), `/api/interner/top?n=N` (admin-only, Top-N интернированных запросов с 120-char preview).

**Architecture:**
- Public endpoints (`/api/prepared`, `/api/interner`) — те же тривиальные обёртки над `collect_*()` что и в фазе 3c-2.
- Admin-only endpoints (`/api/prepared/text/{hash}`, `/api/interner/top`) уже автоматически защищены mux'ом: `ADMIN_ONLY_PREFIXES` в `src/web/server.rs:35` уже содержит `/api/prepared/text/` и `/api/interner/top`. Никаких изменений в auth-логике не требуется.
- `/api/prepared/text/{hash}` — path parameter. Mux парсит prefix в `route_api`; handler извлекает hex-hash, пробегает по `get_all_pools()`, возвращает первое совпадение. Если не найдено — 404 c body `{"error":"not_found","message":"prepared statement not found for hash <hex>"}`.
- `/api/interner/top` — query parameter `?n=N`, default 20, clamp [1, 200].

**Privacy для public agg endpoints:** spec section 8.2 требует чтобы public `/api/prepared` и `/api/interner` НЕ содержали текстов SQL — операторы без admin-прав не должны видеть содержимое запросов (потенциальная утечка данных через debug deparse). DTO для `/api/prepared` ОТСУТСТВУЕТ поле `query`. DTO для `/api/interner` показывает агрегаты (entries+bytes) без какого-либо preview. Только admin-only `/api/prepared/text/{hash}` и `/api/interner/top` отдают контент.

**Reference:**
- Spec section 8.2 (full endpoint list, public/admin matrix).
- Phase 3c-2 commit: `5b31a69`.
- Existing admin SHOW: `src/admin/show.rs:160` (prepared_statements), `:201` (interner), `:242` (interner_top).

**Не входит в фазу 3c-3:**
- Top-N (`/api/top/queries|clients|prepared`), `/api/apps`, `/api/events` — phase 3d.
- LogTap (`/api/logs`) — phase 4.

---

## File Structure

**Новые файлы:**
- `src/web/routes/prepared.rs` — handler `GET /api/prepared`.
- `src/web/routes/prepared_text.rs` — handler `GET /api/prepared/text/{hash}`.
- `src/web/routes/interner.rs` — handler `GET /api/interner`.
- `src/web/routes/interner_top.rs` — handler `GET /api/interner/top`.

**Модифицируемые файлы:**
- `src/web/routes/dto.rs` — `PreparedDto`, `PreparedRowDto`, `InternerDto`, `InternerKindDto`, `InternerTopDto`, `InternerTopRowDto`, `PreparedTextDto`.
- `src/web/routes/collect.rs` — `collect_prepared`, `collect_interner`, `collect_prepared_text(hash)`, `collect_interner_top(n)`.
- `src/web/routes/mod.rs` — register 4 new modules.
- `src/web/server.rs` — `route_api` arms + admin-prefix routing для `/api/prepared/text/`.
- `src/web/tests.rs` — integration tests (public 200, admin 401 без auth, admin 200 с auth, admin 404 для несуществующего hash).

---

## Task 0: Baseline проверка

**Files:** none modified.

- [ ] **Step 0.1**

```bash
cd /home/vadv/Projects/pg_doorman
git status
git log --oneline -3
```
Expected: HEAD = `5b31a69`, working tree clean (untracked `.local/`, `Dockerfile.ubuntu22-tls`, `INCIDENT_*.md` допустимы).

- [ ] **Step 0.2**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 754 passed, clean.

---

## Task 1: DTOs

**Files:** Modify `src/web/routes/dto.rs`.

- [ ] **Step 1.1: Добавить DTOs в конец файла**

```rust
/// `GET /api/prepared` — aggregate of pool-level prepared-statement caches.
///
/// Public endpoint. The `query` text is intentionally NOT included here to
/// avoid leaking SQL bodies to anonymous Web UI viewers; the admin-only
/// `/api/prepared/text/{hash}` endpoint returns the text on demand.
#[derive(Debug, Serialize)]
pub struct PreparedDto {
    pub ts: u64,
    pub prepared: Vec<PreparedRowDto>,
}

#[derive(Debug, Serialize)]
pub struct PreparedRowDto {
    /// Pool identifier in the form rendered by `PoolIdentifier::Display`.
    pub pool: String,
    /// 64-bit FxHash, formatted as decimal to mirror SHOW PREPARED STATEMENTS.
    pub hash: String,
    pub name: String,
    pub count_used: u64,
    /// One of "named", "anonymous", "mixed" — `CacheEntryKind::as_str`.
    pub kind: String,
}

/// `GET /api/interner` — global query interner aggregate.
/// Public; no SQL preview.
#[derive(Debug, Serialize)]
pub struct InternerDto {
    pub ts: u64,
    pub named: InternerKindDto,
    pub anonymous: InternerKindDto,
}

#[derive(Debug, Serialize)]
pub struct InternerKindDto {
    pub entries: u64,
    pub bytes: u64,
}

/// `GET /api/interner/top?n=N` — admin-only Top-N interner entries by
/// interned-text byte length, with a 120-character SQL preview.
#[derive(Debug, Serialize)]
pub struct InternerTopDto {
    pub ts: u64,
    /// The clamped value of `n` actually used (1..=MAX).
    pub n: u64,
    pub entries: Vec<InternerTopRowDto>,
}

#[derive(Debug, Serialize)]
pub struct InternerTopRowDto {
    /// `0x<hex>` form of the FxHash, matching SHOW INTERNER TOP.
    pub hash: String,
    /// `"named"` or `"anonymous"`.
    pub kind: String,
    pub bytes: u64,
    /// Idle milliseconds for anonymous entries; `-1` for named (named tracks
    /// GC state instead of last-used).
    pub idle_ms: i64,
    /// First 120 characters of the interned text (truncated by chars, not
    /// bytes — keeps multi-byte UTF-8 sequences whole).
    pub preview: String,
}

/// `GET /api/prepared/text/{hash}` — admin-only body of a single prepared
/// statement. Returns 404 when the hash is not present in any pool's cache.
#[derive(Debug, Serialize)]
pub struct PreparedTextDto {
    pub ts: u64,
    pub hash: String,
    pub pool: String,
    pub name: String,
    pub query: String,
    pub kind: String,
}
```

- [ ] **Step 1.2: cargo build + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 1.3: Не коммитить.**

---

## Task 2: collect-функции + unit-тесты на parse_n

**Files:** Modify `src/web/routes/collect.rs`.

- [ ] **Step 2.1: Расширить use-импорты**

```rust
use crate::server::{anon_snapshot, named_snapshot, now_monotonic_ms};
use crate::web::routes::dto::{
    InternerDto, InternerKindDto, InternerTopDto, InternerTopRowDto, PreparedDto, PreparedRowDto,
    PreparedTextDto,
    // ... existing entries
};
```

- [ ] **Step 2.2: `collect_prepared`**

```rust
pub fn collect_prepared() -> PreparedDto {
    let mut prepared: Vec<PreparedRowDto> = Vec::new();
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        for (hash, parse, count_used, kind) in cache.get_entries() {
            prepared.push(PreparedRowDto {
                pool: identifier.to_string(),
                hash: hash.to_string(),
                name: parse.name.clone(),
                count_used,
                kind: kind.as_str().to_string(),
            });
        }
    }

    // Stable order: pool first, then hash, for deterministic UI display.
    prepared.sort_by(|a, b| (a.pool.as_str(), a.hash.as_str()).cmp(&(b.pool.as_str(), b.hash.as_str())));

    PreparedDto {
        ts: now_unix_ms(),
        prepared,
    }
}
```

**Verify:** `PoolIdentifier` имеет `Display` impl (используется в `show_prepared_statements:180`). Если нет — заменить на `format!("{}@{}", identifier.user, identifier.db)`.

- [ ] **Step 2.3: `collect_interner`**

```rust
pub fn collect_interner() -> InternerDto {
    let named = named_snapshot();
    let anon = anon_snapshot();
    let named_bytes: u64 = named.iter().map(|(_, e)| e.text().len() as u64).sum();
    let anon_bytes: u64 = anon.iter().map(|(_, e)| e.text().len() as u64).sum();

    InternerDto {
        ts: now_unix_ms(),
        named: InternerKindDto {
            entries: named.len() as u64,
            bytes: named_bytes,
        },
        anonymous: InternerKindDto {
            entries: anon.len() as u64,
            bytes: anon_bytes,
        },
    }
}
```

- [ ] **Step 2.4: `collect_prepared_text`**

```rust
pub fn collect_prepared_text(hash: u64) -> Option<PreparedTextDto> {
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        for (h, parse, _count, kind) in cache.get_entries() {
            if h == hash {
                return Some(PreparedTextDto {
                    ts: now_unix_ms(),
                    hash: format!("{:#x}", hash),
                    pool: identifier.to_string(),
                    name: parse.name.clone(),
                    query: parse.query().to_string(),
                    kind: kind.as_str().to_string(),
                });
            }
        }
    }
    None
}
```

- [ ] **Step 2.5: `collect_interner_top` + pure helper `clamp_top_n`**

```rust
/// Clamps the user-supplied `?n=` parameter to a sensible range.
///
/// `0` and missing → default 20 (matches SHOW INTERNER TOP convention).
/// Values above 200 are capped — the page would be unusable beyond that
/// and a 100k-entry interner shouldn't materialise an unbounded preview list.
pub(crate) fn clamp_top_n(requested: u64) -> u64 {
    const DEFAULT: u64 = 20;
    const MAX: u64 = 200;
    match requested {
        0 => DEFAULT,
        n if n > MAX => MAX,
        n => n,
    }
}

pub fn collect_interner_top(n: u64) -> InternerTopDto {
    let n = clamp_top_n(n);
    let now = now_monotonic_ms();

    enum Handle {
        Named(std::sync::Arc<crate::server::NamedEntry>),
        Anon(std::sync::Arc<crate::server::AnonEntry>),
    }

    let mut combined: Vec<(u64, &'static str, usize, i64, Handle)> = Vec::new();
    for (hash, entry) in named_snapshot() {
        let bytes = entry.text().len();
        combined.push((hash, "named", bytes, -1, Handle::Named(entry)));
    }
    for (hash, entry) in anon_snapshot() {
        let idle = entry.idle_ms(now) as i64;
        let bytes = entry.text().len();
        combined.push((hash, "anonymous", bytes, idle, Handle::Anon(entry)));
    }
    combined.sort_by_key(|r| std::cmp::Reverse(r.2));

    let entries = combined
        .into_iter()
        .take(n as usize)
        .map(|(hash, kind, bytes, idle_ms, handle)| {
            let text = match handle {
                Handle::Named(e) => e.text().clone(),
                Handle::Anon(e) => e.text().clone(),
            };
            let preview: String = text.chars().take(120).collect();
            InternerTopRowDto {
                hash: format!("{:#x}", hash),
                kind: kind.to_string(),
                bytes: bytes as u64,
                idle_ms,
                preview,
            }
        })
        .collect();

    InternerTopDto {
        ts: now_unix_ms(),
        n,
        entries,
    }
}
```

- [ ] **Step 2.6: Unit-тесты на `clamp_top_n`**

В `#[cfg(test)] mod tests` добавить:

```rust
    #[test]
    fn clamp_top_n_zero_returns_default() {
        assert_eq!(super::clamp_top_n(0), 20);
    }

    #[test]
    fn clamp_top_n_keeps_in_range() {
        assert_eq!(super::clamp_top_n(1), 1);
        assert_eq!(super::clamp_top_n(50), 50);
        assert_eq!(super::clamp_top_n(200), 200);
    }

    #[test]
    fn clamp_top_n_caps_above_max() {
        assert_eq!(super::clamp_top_n(201), 200);
        assert_eq!(super::clamp_top_n(u64::MAX), 200);
    }
```

- [ ] **Step 2.7: cargo build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib web::routes::collect:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 36 collect tests passing (33 baseline + 3 clamp_top_n), clean.

- [ ] **Step 2.8: Не коммитить.**

---

## Task 3: handlers (4 файла)

**Files:**
- Create: `src/web/routes/{prepared,interner,prepared_text,interner_top}.rs`.
- Modify: `src/web/routes/mod.rs`.

- [ ] **Step 3.1: `prepared.rs`**

```rust
//! GET /api/prepared handler. Public — aggregate without SQL text.

use crate::web::routes::collect::collect_prepared;
use crate::web::server::Response;

pub(crate) fn handle_prepared() -> Response {
    Response::ok_json(&collect_prepared())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_response_is_200_with_envelope() {
        let r = handle_prepared();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"prepared\""));
    }
}
```

- [ ] **Step 3.2: `interner.rs`**

```rust
//! GET /api/interner handler. Public — aggregate without SQL preview.

use crate::web::routes::collect::collect_interner;
use crate::web::server::Response;

pub(crate) fn handle_interner() -> Response {
    Response::ok_json(&collect_interner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interner_response_is_200_with_envelope() {
        let r = handle_interner();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"named\""));
        assert!(body.contains("\"anonymous\""));
    }
}
```

- [ ] **Step 3.3: `prepared_text.rs`**

```rust
//! GET /api/prepared/text/{hash} handler. Admin-only (mux gates the prefix).

use crate::web::routes::collect::collect_prepared_text;
use crate::web::server::Response;

pub(crate) fn handle_prepared_text(hash_str: &str) -> Response {
    let Some(hash) = parse_hash(hash_str) else {
        return Response::json(
            400,
            "Bad Request",
            r#"{"error":"bad_hash","message":"hash must be decimal or 0x-prefixed hex u64"}"#,
        );
    };
    match collect_prepared_text(hash) {
        Some(dto) => Response::ok_json(&dto),
        None => Response::json(
            404,
            "Not Found",
            r#"{"error":"not_found","message":"prepared statement not found for hash"}"#,
        ),
    }
}

fn parse_hash(s: &str) -> Option<u64> {
    if let Some(stripped) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(stripped, 16).ok();
    }
    s.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_text_returns_404_on_unknown_hash() {
        let r = handle_prepared_text("0xdeadbeef");
        assert_eq!(r.status, 404);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("not_found"));
    }

    #[test]
    fn prepared_text_returns_400_on_malformed_hash() {
        let r = handle_prepared_text("not-a-hash");
        assert_eq!(r.status, 400);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("bad_hash"));
    }

    #[test]
    fn parse_hash_decimal() {
        assert_eq!(parse_hash("12345"), Some(12345));
    }

    #[test]
    fn parse_hash_hex_prefix() {
        assert_eq!(parse_hash("0xff"), Some(255));
        assert_eq!(parse_hash("0XFF"), Some(255));
    }

    #[test]
    fn parse_hash_invalid() {
        assert_eq!(parse_hash(""), None);
        assert_eq!(parse_hash("xyz"), None);
        assert_eq!(parse_hash("0xZZ"), None);
    }
}
```

- [ ] **Step 3.4: `interner_top.rs`**

```rust
//! GET /api/interner/top?n=N handler. Admin-only (mux gates the prefix).

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_interner_top;
use crate::web::routes::query::parse_u64;
use crate::web::server::Response;

pub(crate) fn handle_interner_top(query: &BTreeMap<String, Vec<String>>) -> Response {
    let n = parse_u64(query, "n", 0); // 0 → default in clamp_top_n
    Response::ok_json(&collect_interner_top(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interner_top_response_is_200() {
        let q = BTreeMap::new();
        let r = handle_interner_top(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"entries\""));
        assert!(body.contains("\"n\":20"));
    }

    #[test]
    fn interner_top_honours_n_query_param() {
        let mut q = BTreeMap::new();
        q.insert("n".into(), vec!["50".into()]);
        let r = handle_interner_top(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"n\":50"));
    }
}
```

- [ ] **Step 3.5: `mod.rs` — register modules**

В `src/web/routes/mod.rs` добавить (alphabetical):

```rust
pub(crate) mod interner;
pub(crate) mod interner_top;
pub(crate) mod prepared;
pub(crate) mod prepared_text;
```

- [ ] **Step 3.6: cargo build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib web::routes::prepared:: web::routes::prepared_text:: web::routes::interner:: web::routes::interner_top:: 2>&1 | tail -15
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 9 handler-тестов passing (1 prepared + 1 interner + 5 prepared_text + 2 interner_top), clean.

- [ ] **Step 3.7: Не коммитить.**

---

## Task 4: интеграция в mux + dispatch tests

**Files:** Modify `src/web/server.rs`.

- [ ] **Step 4.1: Расширить `route_api`**

Сейчас `route_api` использует `match path` для exact-string match. Для `/api/prepared/text/<hash>` нужен prefix-match. Меняем структуру:

```rust
fn route_api(req: &ParsedRequest<'_>) -> Response {
    use crate::web::routes;
    use crate::web::routes::query::parse_query;

    let (path, query_str) = match req.path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (req.path, ""),
    };
    let query = parse_query(query_str);

    // Prefix-routed paths first (admin-only; mux already gated auth).
    if let Some(hash) = path.strip_prefix("/api/prepared/text/") {
        return routes::prepared_text::handle_prepared_text(hash);
    }

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
        "/api/auth_query" => routes::auth_query::handle_auth_query(),
        "/api/config" => routes::config::handle_config(),
        "/api/log_level" => routes::log_level::handle_log_level(),
        "/api/pool_coordinator" => routes::pool_coordinator::handle_pool_coordinator(),
        "/api/pool_scaling" => routes::pool_scaling::handle_pool_scaling(),
        "/api/sockets" => routes::sockets::handle_sockets(),
        "/api/prepared" => routes::prepared::handle_prepared(),
        "/api/interner" => routes::interner::handle_interner(),
        "/api/interner/top" => routes::interner_top::handle_interner_top(&query),
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
}
```

- [ ] **Step 4.2: 6 dispatch-тестов**

Добавить:

```rust
    #[test]
    fn dispatch_prepared_returns_200() {
        let r = dispatch(
            &req("GET", "/api/prepared"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_interner_returns_200() {
        let r = dispatch(
            &req("GET", "/api/interner"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_interner_top_anonymous_returns_401() {
        let r = dispatch(
            &req("GET", "/api/interner/top"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_interner_top_admin_returns_200() {
        let r = dispatch(
            &req("GET", "/api/interner/top?n=10"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_prepared_text_anonymous_returns_401() {
        let r = dispatch(
            &req("GET", "/api/prepared/text/0x123"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_prepared_text_admin_unknown_hash_returns_404() {
        let r = dispatch(
            &req("GET", "/api/prepared/text/0xdeadbeef"),
            &opts(true, true),
            AuthOutcome::Admin,
        );
        assert_eq!(r.status, 404);
    }
```

- [ ] **Step 4.3: Прогон тестов**

```bash
cargo test --lib web::server:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 37 server-тестов (31 baseline + 6 new), clippy/fmt clean.

- [ ] **Step 4.4: Не коммитить.**

---

## Task 5: integration tests

**Files:** Modify `src/web/tests.rs`.

- [ ] **Step 5.1: Добавить integration-тесты**

В конец `src/web/tests.rs`:

```rust
#[tokio::test]
async fn api_prepared_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/prepared HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"prepared\""), "raw={raw}");
}

#[tokio::test]
async fn api_interner_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/interner HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"named\""), "raw={raw}");
    assert!(raw.contains("\"anonymous\""), "raw={raw}");
}

#[tokio::test]
async fn api_interner_top_anonymous_returns_401() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/interner/top HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}

#[tokio::test]
async fn api_interner_top_admin_returns_200() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/interner/top?n=5 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"n\":5"), "raw={raw}");
}

#[tokio::test]
async fn api_prepared_text_anonymous_returns_401() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/prepared/text/0x123 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}

#[tokio::test]
async fn api_prepared_text_admin_unknown_hash_returns_404() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/prepared/text/0xdeadbeef HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    assert!(raw.starts_with("HTTP/1.1 404"), "raw={raw}");
}
```

- [ ] **Step 5.2: Полный прогон**

```bash
cargo test --lib 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~778 passed (754 baseline + 3 clamp_top_n + 9 handler unit + 6 dispatch + 6 integration). Точное число подтверждаем при прогоне.

- [ ] **Step 5.3: Не коммитить.**

---

## Task 6: Final-проверка + commit

- [ ] **Step 6.1: cargo fmt + clippy + test**

- [ ] **Step 6.2: Smoke check release build**

```bash
cargo build --release 2>&1 | tail -3
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3c3.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/prepared (public) ---"
curl -s 'http://127.0.0.1:19127/api/prepared' | head -c 400
echo ""
echo "--- /api/interner (public) ---"
curl -s 'http://127.0.0.1:19127/api/interner'
echo ""
echo "--- /api/interner/top анон → 401 ---"
curl -s -o /dev/null -w "%{http_code}\n" 'http://127.0.0.1:19127/api/interner/top'
echo "--- /api/interner/top admin → 200 ---"
curl -s --user 'admin:smoke-secret-do-not-use' 'http://127.0.0.1:19127/api/interner/top?n=5' | head -c 400
echo ""
echo "--- /api/prepared/text/0xdeadbeef admin → 404 ---"
curl -s -o /dev/null -w "%{http_code}\n" --user 'admin:smoke-secret-do-not-use' 'http://127.0.0.1:19127/api/prepared/text/0xdeadbeef'
kill $DPID 2>/dev/null
wait $DPID 2>/dev/null
```

Expected: public endpoints отвечают `200`, anonymous admin path → 401, admin auth → 200/404 в зависимости от наличия hash.

- [ ] **Step 6.3: Pre-commit code review**

Через Agent (правило CLAUDE.md). Черновик:

```
feat(web): land /api/prepared /api/interner + admin /api/prepared/text /api/interner/top

The Caches page in the upcoming Web UI gets its public aggregates plus
the two admin-only endpoints for actually inspecting query bodies.

/api/prepared is the per-pool prepared-statement summary; SQL bodies
are intentionally absent from this public response — the admin-only
/api/prepared/text/{hash} endpoint serves the body on demand. Likewise
/api/interner gives the global named/anonymous interner counts and
byte totals, and the admin-only /api/interner/top?n=N returns the
heaviest entries with a 120-character preview, capped at n=200 so a
100k-entry interner does not turn into an unbounded preview list.

Tests: <N> passed (was 754), clippy and fmt clean. Verified by release
binary smoke-tests against all four endpoints, including the 401
anonymous gate on the admin paths and the 404 path for an unknown
hash.

Phase 3c-3 of seven; phase 3d lands the killer-feature top-N triage
endpoints, /api/apps, and /api/events.
```

- [ ] **Step 6.4: commit**

---

## Self-review

**Spec coverage check:**
- ✅ `/api/prepared` (раздел 8.2 «Агрегат prepared statements без текстов») — text excluded from `PreparedRowDto`.
- ✅ `/api/interner` (раздел 8.2 «Агрегат query interner без preview») — counts only, no preview.
- ✅ `/api/prepared/text/{hash}` admin-only (раздел 8.2) — гарантировано через `ADMIN_ONLY_PREFIXES` (server.rs:35).
- ✅ `/api/interner/top?n=N` admin-only с 120-char preview (раздел 8.2) — гарантировано через `ADMIN_ONLY_PREFIXES`.
- ✅ Public access для `/api/prepared`, `/api/interner` — handlers без auth-проверок.

**Privacy guarantee:** `/api/prepared` намеренно не содержит поля `query`. `/api/interner` намеренно не содержит preview. Test суиты в фазе 3c-3 не assert'ят ОТСУТСТВИЕ этих полей — в DTO их просто нет, что эквивалентно. Фронтенд phase 6 предполагает что для просмотра текстов оператору нужно админ-логиниться.

**Не покрыто этой фазой:**
- `/api/top/*`, `/api/apps`, `/api/events` — phase 3d.
- LogTap (`/api/logs`) — phase 4.

**Type-consistency check:**
- `hash: String` в DTO (формат "decimal" для PreparedRowDto, "0x<hex>" для InternerTopRowDto + PreparedTextDto) — соответствует существующему admin SHOW.
- `n: u64` clamped to [1, 200].
- `idle_ms: i64` — `-1` сентинел для named (нет idle-tracking).

**Placeholder check:** Нет полей со заглушкой 0 или TODO. Все значения из реальных источников.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-3c-3.md`.
Subagent-driven execution; controller dispatches one implementer per task plus two-stage review.
