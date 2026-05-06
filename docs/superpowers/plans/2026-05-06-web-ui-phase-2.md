# Web UI — Phase 2 Implementation Plan: Listener Mux + Basic-Auth + Default-Password Gate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Расширить текущий `/metrics`-only listener до mux'а, который под `[web].ui = true` и не-дефолтным `admin_password` начинает отдавать `/api/*` (пока заглушка 501) и SPA-пути (`/`, `/assets/*` — заглушка 404 до фазы 7), под basic-auth-правилами раздела 6.1 спеки. `/metrics` продолжает отдавать ровно те же байты, что и сегодня. Фаза 2 не вводит ни одного реального API-handler'а — только auth-scaffolding и dispatch.

**Architecture:** Новый `src/web/server.rs` берёт на себя listener и mux. Старый `src/web/metrics/server.rs` распиливается: accept loop переезжает в `web/server.rs`, а собственно генерация `/metrics` body остаётся в `web/metrics/handler.rs`. `web/auth.rs` — чистый парсер `Authorization: Basic <b64>` + constant-time compare через крейт `subtle`. Default-password gate добавляется в `src/app/server.rs` перед спауном listener'а: если `ui = true` и `admin_password ∈ {"", "admin"}`, listener поднимается без `/api/*` и SPA, а `log::warn!` объясняет почему.

**Tech Stack:** Rust + tokio + base64 (уже в Cargo.toml 0.22) + subtle (новая dep, ≈500 LOC, без транзитива). Тесты — `tokio::test` + `reqwest` (если уже в dev-deps; иначе ручной `TcpStream`).

**Reference:**
- Spec: `docs/superpowers/specs/2026-05-06-web-ui-design.md`, разделы 4.1 (топология listener'а), 4.2 (file structure), 5.4 (default-password safety), 6 (auth & access matrix), 13.5 (release checklist).
- Decision log: #4 (config flags), #11 ([web] / alias prometheus), #13 (default-password — UI off + warn), #18 (freshness + health pill — но это frontend, не backend).
- Phase 1 commit: `b24082f`.

**Не входит в фазу 2** (отдельные планы для последующих фаз):
- Реальные `/api/*` handlers и `collect_*()` рефакторинг `admin/show.rs` — фаза 3.
- LogTap — фаза 4.
- Frontend skeleton/pages — фазы 5-6.
- `include_dir!` + SPA-эфирация — фаза 7.

---

## File Structure

**Новые файлы:**
- `src/web/server.rs` — accept loop + mux + dispatch.
- `src/web/auth.rs` — парсер `Authorization: Basic` + constant-time compare.
- `src/web/metrics/handler.rs` — то, что сейчас в `server.rs:1-101` (`handle_metrics_request`); accept-loop вырезан.

**Удаляемые файлы:**
- `src/web/metrics/server.rs` — распиливается; через `git mv` файл переименовываем в `handler.rs`, accept-loop переезжает в новый `web/server.rs` отдельным diff'ом.

**Модифицируемые файлы:**
- `Cargo.toml` — добавить `subtle = "2"`.
- `src/web/mod.rs` — добавить `pub mod auth; pub mod server;` и `pub use server::start_web_server;`.
- `src/web/metrics/mod.rs` — `mod server;` → `mod handler;`, убрать `pub use server::start_prometheus_server`, добавить `pub(crate) use handler::handle_metrics_request`.
- `src/app/server.rs` (line 360-368) — заменить вызов на `start_web_server` + default-password gate перед ним.

**Тесты:**
- `src/web/auth.rs` — unit tests внутри файла (`#[cfg(test)]`).
- `src/web/server.rs` — unit tests на mux dispatch (taking parsed request, returning Response).
- `src/web/tests.rs` (новый) — integration tests с реальным listener'ом (через `TcpStream`-клиента, без `reqwest`, чтобы не вводить новые dev-deps без необходимости).
- `src/web/metrics/tests.rs` — обновить вызов `start_prometheus_server` → `start_web_server`, проверить что /metrics всё ещё отдаёт `pg_doorman_*`.

---

## Task 0: Baseline проверка

**Files:** none modified.

- [ ] **Step 0.1: Подтвердить чистое состояние ветки**

```bash
cd /home/vadv/Projects/pg_doorman
git status
git log --oneline -3
```
Expected: HEAD = `34bd426 docs(spec): frontend dist commits to git, no npm in release pipeline` или новее, working tree clean (untracked `.local/`, `Dockerfile.ubuntu22-tls`, `INCIDENT_*.md` — не наше).

- [ ] **Step 0.2: Зафиксировать baseline**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 634 passed, clippy clean, fmt clean.

---

## Task 1: Добавить `subtle` dep + TDD-тесты на `src/web/auth.rs`

**Files:**
- Modify: `Cargo.toml`.
- Create: `src/web/auth.rs` (со скелетом + failing tests, без реализации).

- [ ] **Step 1.1: Добавить `subtle` в `[dependencies]`**

В `Cargo.toml` найти секцию `[dependencies]`. Добавить (alphabetical position):

```toml
subtle = "2"
```

- [ ] **Step 1.2: Создать `src/web/auth.rs` со скелетом и tests**

Создать файл `src/web/auth.rs` с:

```rust
//! Basic-auth parser for the web mux.
//!
//! HTTP/1.1 `Authorization: Basic <base64(user:pass)>` header parsing
//! plus constant-time credential comparison.

use base64::Engine;
use subtle::ConstantTimeEq;

/// Authentication outcome for an inbound request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// Request carries no Authorization header.
    Anonymous,
    /// Authorization header present and matched the configured admin credentials.
    Admin,
    /// Authorization header present but malformed or did not match.
    Rejected,
}

/// Inspect the value of an HTTP `Authorization` header (or `None` if absent),
/// compare against `admin_username`/`admin_password` in constant time, and
/// classify the outcome.
///
/// The comparison runs in constant time relative to the configured credentials
/// to deny timing oracles. We do **not** offer a way to learn whether the
/// username matched but the password didn't — both legs are checked together
/// without short-circuit.
pub fn classify(
    authorization_header: Option<&str>,
    admin_username: &str,
    admin_password: &str,
) -> AuthOutcome {
    todo!("implemented in Task 2")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn anonymous_when_header_missing() {
        assert_eq!(classify(None, "admin", "secret"), AuthOutcome::Anonymous);
    }

    #[test]
    fn admin_when_credentials_match() {
        let header = format!("Basic {}", b64("admin:secret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Admin
        );
    }

    #[test]
    fn rejected_when_password_wrong() {
        let header = format!("Basic {}", b64("admin:wrong"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_username_wrong() {
        let header = format!("Basic {}", b64("evil:secret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_scheme_not_basic() {
        let header = format!("Bearer {}", b64("admin:secret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_base64_invalid() {
        assert_eq!(
            classify(Some("Basic !!!not-base64!!!"), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_decoded_has_no_colon() {
        let header = format!("Basic {}", b64("adminsecret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_decoded_is_invalid_utf8() {
        let raw = base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe, 0xfd]);
        let header = format!("Basic {}", raw);
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn admin_when_password_contains_colon() {
        // Per RFC 7617 only the FIRST colon is the separator.
        let header = format!("Basic {}", b64("admin:p:a:s:s"));
        assert_eq!(
            classify(Some(&header), "admin", "p:a:s:s"),
            AuthOutcome::Admin
        );
    }
}
```

- [ ] **Step 1.3: Подключить `pub mod auth;` в `src/web/mod.rs`**

В файле `src/web/mod.rs` добавить строку (после `pub mod metrics;` либо альфавитно):

```rust
pub mod auth;
```

- [ ] **Step 1.4: Запустить тесты — должны падать с `unimplemented!`/`todo!`**

```bash
cargo test --lib web::auth:: 2>&1 | tail -20
```
Expected: 9 тестов запускаются, всё падают по `not yet implemented` от `todo!()`. Это правильное состояние перед TDD-implementation в Task 2.

- [ ] **Step 1.5: Не коммитить.** Phase 2 — один коммит в Task 8.

---

## Task 2: Реализовать `classify()` в `src/web/auth.rs`

**Files:**
- Modify: `src/web/auth.rs` (replace the `todo!()` body).

- [ ] **Step 2.1: Заменить тело `classify`**

В `src/web/auth.rs` заменить тело функции на:

```rust
pub fn classify(
    authorization_header: Option<&str>,
    admin_username: &str,
    admin_password: &str,
) -> AuthOutcome {
    let Some(header) = authorization_header else {
        return AuthOutcome::Anonymous;
    };
    let Some(b64) = header.strip_prefix("Basic ") else {
        return AuthOutcome::Rejected;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return AuthOutcome::Rejected;
    };
    let Ok(decoded_str) = std::str::from_utf8(&decoded) else {
        return AuthOutcome::Rejected;
    };
    let Some((user, pass)) = decoded_str.split_once(':') else {
        return AuthOutcome::Rejected;
    };
    // `&` instead of `&&`: avoids short-circuit, both legs always evaluated
    // so timing depends only on configured credential lengths.
    let matches = bool::from(user.as_bytes().ct_eq(admin_username.as_bytes()))
        & bool::from(pass.as_bytes().ct_eq(admin_password.as_bytes()));
    if matches {
        AuthOutcome::Admin
    } else {
        AuthOutcome::Rejected
    }
}
```

- [ ] **Step 2.2: Запустить тесты — все должны проходить**

```bash
cargo test --lib web::auth:: 2>&1 | tail -10
```
Expected: 9 passed, 0 failed.

- [ ] **Step 2.3: Прогнать clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -5
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 2.4: Не коммитить.**

---

## Task 3: Распилить `src/web/metrics/server.rs` на `handler.rs` + accept-loop удаляется

**Files:**
- Move: `src/web/metrics/server.rs` → `src/web/metrics/handler.rs`.
- Modify: `src/web/metrics/handler.rs` — оставить только `handle_metrics_request`, удалить `start_prometheus_server`.
- Modify: `src/web/metrics/mod.rs` — `mod server;` → `mod handler;`, обновить re-exports.

- [ ] **Step 3.1: git mv**

```bash
cd /home/vadv/Projects/pg_doorman
git mv src/web/metrics/server.rs src/web/metrics/handler.rs
```

- [ ] **Step 3.2: Удалить `start_prometheus_server` из `handler.rs`**

В `src/web/metrics/handler.rs` удалить функцию `start_prometheus_server` (это lines 103-159 в исходнике). Также удалить теперь-неиспользуемые import'ы в начале файла:
- `use std::net::SocketAddr;`
- `use tokio::net::TcpSocket;`

Оставить:
- `use flate2::write::GzEncoder; use flate2::Compression;`
- `use log::error;` (вместо `log::{error, info}` — `info!` использовался только в `start_prometheus_server`).
- `use prometheus::{Encoder, TextEncoder}; use std::io::Write;`
- `use super::metrics::update_metrics; use super::REGISTRY;`

Также сменить doc-комментарий в начале файла:

```rust
//! Handler that builds the /metrics body and writes it onto a TcpStream.
//! The accept loop and HTTP routing live in `crate::web::server`.
```

- [ ] **Step 3.3: Обновить `src/web/metrics/mod.rs`**

Найти строку `mod server;` и заменить на `mod handler;`.
Найти строку `pub use server::start_prometheus_server;` и заменить на:

```rust
pub(crate) use handler::handle_metrics_request;
```

Public scope сужен до `pub(crate)` — функция нужна только внутри `web::server` и тестов того же crate, наружу не торчит.

- [ ] **Step 3.4: cargo build падает (ожидаемо)**

```bash
cargo build --lib 2>&1 | tail -10
```
Expected: ошибка `cannot find function start_prometheus_server in module crate::web::metrics` — в `src/app/server.rs:361-368` и в `src/web/metrics/tests.rs`. Чиним в Task 5 и Task 7 соответственно.

- [ ] **Step 3.5: Не коммитить.**

---

## Task 4: Создать `src/web/server.rs` с listener + mux

**Files:**
- Create: `src/web/server.rs`.

- [ ] **Step 4.1: Создать `src/web/server.rs`**

Содержимое (~250 строк; полностью приведено ниже):

```rust
//! HTTP listener + path mux for the web subsystem.
//!
//! Routes:
//! - `GET /metrics` → Prometheus exporter, no auth.
//! - `GET /api/*`   → public or admin per spec section 6, returns 501 in phase 2.
//! - `GET /` | `GET /assets/*` → SPA placeholder, returns 404 in phase 2 (filled in phase 7).
//! - everything else → 404.
//!
//! Phase 2 ships only the dispatch + auth gating; real `/api/*` handlers and
//! the SPA bundle are added in phases 3 and 7.

use std::net::SocketAddr;

use log::{error, info};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpSocket, TcpStream};

use crate::web::auth::{classify, AuthOutcome};
use crate::web::metrics::handle_metrics_request;

/// Runtime state needed by the mux on every request.
#[derive(Clone)]
pub struct WebServerOptions {
    /// `true` when `[web].ui = true` AND admin_password is non-default.
    /// When `false`, the listener serves only `/metrics`; everything else → 404.
    pub ui_active: bool,
    /// `[web].ui_anonymous` (gates public `/api/*` and SPA paths when ui_active).
    pub ui_anonymous: bool,
    pub admin_username: String,
    pub admin_password: String,
}

/// Admin-only path prefixes (require `Admin` auth regardless of `ui_anonymous`).
/// Spec section 6.1.
const ADMIN_ONLY_PREFIXES: &[&str] = &[
    "/api/logs",
    "/api/prepared/text/",
    "/api/interner/top",
];

/// Spawns the HTTP listener for the given address.
pub async fn start_web_server(host: &str, opts: WebServerOptions) {
    info!("starting web listener on {host}");
    let addr: SocketAddr = match host.parse() {
        Ok(addr) => addr,
        Err(e) => panic!("Failed to parse socket address '{host}': {e}"),
    };

    let listen_socket = if addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .unwrap_or_else(|e| panic!("Failed to create socket: {e}"));

    listen_socket
        .set_reuseaddr(true)
        .unwrap_or_else(|e| panic!("Failed to set SO_REUSEADDR: {e}"));
    listen_socket
        .set_reuseport(true)
        .unwrap_or_else(|e| panic!("Failed to set SO_REUSEPORT: {e}"));
    listen_socket
        .bind(addr)
        .unwrap_or_else(|e| panic!("Failed to bind to address {addr}: {e}"));

    let listener = listen_socket
        .listen(1024)
        .unwrap_or_else(|e| panic!("Failed to listen on {addr}: {e}"));
    info!("web listener bound on {addr}");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let opts = opts.clone();
                tokio::spawn(async move {
                    handle_connection(stream, opts).await;
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

async fn handle_connection(stream: TcpStream, opts: WebServerOptions) {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut writer = BufWriter::new(write_half);

    let mut buf = [0u8; 4096];
    let n = match reader.read(&mut buf).await {
        Ok(0) => return,
        Ok(n) => n,
        Err(e) => {
            error!("Failed to read HTTP request: {e}");
            return;
        }
    };

    let raw = match std::str::from_utf8(&buf[..n]) {
        Ok(s) => s,
        Err(_) => {
            let _ = write_simple(&mut writer, 400, "Bad Request").await;
            return;
        }
    };

    let Some(parsed) = ParsedRequest::parse(raw) else {
        let _ = write_simple(&mut writer, 400, "Bad Request").await;
        return;
    };

    // /metrics is always served, regardless of ui_active or auth.
    if parsed.method == "GET" && parsed.path == "/metrics" {
        // Reassemble: handle_metrics_request expects the full TcpStream and
        // does its own read+gzip pipeline. We've already consumed the request
        // header — reroute by writing /metrics output directly here.
        let inner = reader.into_inner().reunite(writer.into_inner()).unwrap();
        handle_metrics_request(inner).await;
        return;
    }

    let auth = classify(
        parsed.authorization,
        &opts.admin_username,
        &opts.admin_password,
    );

    let response = dispatch(&parsed, &opts, auth);
    let _ = response.write(&mut writer).await;
}

#[derive(Debug)]
struct ParsedRequest<'a> {
    method: &'a str,
    path: &'a str,
    authorization: Option<&'a str>,
}

impl<'a> ParsedRequest<'a> {
    fn parse(raw: &'a str) -> Option<Self> {
        let mut lines = raw.split("\r\n");
        let request_line = lines.next()?;
        let mut parts = request_line.splitn(3, ' ');
        let method = parts.next()?;
        let path = parts.next()?;
        let _http_version = parts.next()?;

        let mut authorization = None;
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Authorization: ") {
                authorization = Some(value);
            } else if let Some(value) = line.strip_prefix("authorization: ") {
                authorization = Some(value);
            }
        }
        Some(ParsedRequest {
            method,
            path,
            authorization,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Response {
    status: u16,
    reason: &'static str,
    extra_headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Response {
    fn status(status: u16, reason: &'static str) -> Self {
        Response {
            status,
            reason,
            extra_headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn json(status: u16, reason: &'static str, body: &str) -> Self {
        Response {
            status,
            reason,
            extra_headers: vec![("Content-Type", "application/json".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    fn unauthorized() -> Self {
        let mut r = Response::status(401, "Unauthorized");
        r.extra_headers.push((
            "WWW-Authenticate",
            "Basic realm=\"pg_doorman admin\"".into(),
        ));
        r
    }

    async fn write(self, writer: &mut BufWriter<tokio::net::tcp::OwnedWriteHalf>) -> std::io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n",
            self.status,
            self.reason,
            self.body.len()
        );
        for (k, v) in &self.extra_headers {
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        writer.write_all(head.as_bytes()).await?;
        if !self.body.is_empty() {
            writer.write_all(&self.body).await?;
        }
        writer.flush().await
    }
}

async fn write_simple(
    writer: &mut BufWriter<tokio::net::tcp::OwnedWriteHalf>,
    status: u16,
    reason: &'static str,
) -> std::io::Result<()> {
    Response::status(status, reason).write(writer).await
}

fn is_admin_only(path: &str) -> bool {
    ADMIN_ONLY_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

fn dispatch(req: &ParsedRequest<'_>, opts: &WebServerOptions, auth: AuthOutcome) -> Response {
    if req.method != "GET" && req.method != "HEAD" {
        return Response::status(405, "Method Not Allowed");
    }

    if !opts.ui_active {
        // /metrics already handled before dispatch().
        return Response::status(404, "Not Found");
    }

    let admin_only = req.path.starts_with("/api/") && is_admin_only(req.path);

    let needs_admin = admin_only || (!opts.ui_anonymous);
    if needs_admin && auth != AuthOutcome::Admin {
        return Response::unauthorized();
    }

    if req.path.starts_with("/api/") {
        // Real handlers land in phase 3.
        return Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"phase 2 stub; api routes land in phase 3"}"#,
        );
    }

    if req.path == "/" || req.path.starts_with("/assets/") {
        // SPA bundle is wired in phase 7.
        return Response::status(404, "Not Found");
    }

    Response::status(404, "Not Found")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(ui_active: bool, ui_anonymous: bool) -> WebServerOptions {
        WebServerOptions {
            ui_active,
            ui_anonymous,
            admin_username: "admin".into(),
            admin_password: "secret".into(),
        }
    }

    fn req<'a>(method: &'a str, path: &'a str) -> ParsedRequest<'a> {
        ParsedRequest {
            method,
            path,
            authorization: None,
        }
    }

    #[test]
    fn parse_minimal_get() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.method, "GET");
        assert_eq!(p.path, "/api/foo");
        assert_eq!(p.authorization, None);
    }

    #[test]
    fn parse_with_authorization_header() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nAuthorization: Basic abc\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.authorization, Some("Basic abc"));
    }

    #[test]
    fn parse_with_lowercase_authorization() {
        let raw = "GET /api/foo HTTP/1.1\r\nHost: x\r\nauthorization: Basic abc\r\n\r\n";
        let p = ParsedRequest::parse(raw).unwrap();
        assert_eq!(p.authorization, Some("Basic abc"));
    }

    #[test]
    fn parse_rejects_malformed_request_line() {
        assert!(ParsedRequest::parse("garbage").is_none());
    }

    #[test]
    fn dispatch_rejects_post() {
        let r = dispatch(&req("POST", "/api/foo"), &opts(true, true), AuthOutcome::Anonymous);
        assert_eq!(r.status, 405);
    }

    #[test]
    fn dispatch_404_when_ui_inactive() {
        let r = dispatch(&req("GET", "/api/foo"), &opts(false, true), AuthOutcome::Anonymous);
        assert_eq!(r.status, 404);
    }

    #[test]
    fn dispatch_anonymous_public_path_when_ui_anonymous_true() {
        let r = dispatch(&req("GET", "/api/overview"), &opts(true, true), AuthOutcome::Anonymous);
        assert_eq!(r.status, 501);
    }

    #[test]
    fn dispatch_401_on_anonymous_admin_path() {
        let r = dispatch(&req("GET", "/api/logs"), &opts(true, true), AuthOutcome::Anonymous);
        assert_eq!(r.status, 401);
        assert!(r.extra_headers.iter().any(|(k, _)| *k == "WWW-Authenticate"));
    }

    #[test]
    fn dispatch_admin_path_with_admin_auth_returns_501() {
        let r = dispatch(&req("GET", "/api/logs"), &opts(true, true), AuthOutcome::Admin);
        assert_eq!(r.status, 501);
    }

    #[test]
    fn dispatch_401_on_anonymous_public_when_ui_anonymous_false() {
        let r = dispatch(&req("GET", "/api/overview"), &opts(true, false), AuthOutcome::Anonymous);
        assert_eq!(r.status, 401);
    }

    #[test]
    fn dispatch_404_for_root_in_phase_2() {
        let r = dispatch(&req("GET", "/"), &opts(true, true), AuthOutcome::Admin);
        assert_eq!(r.status, 404);
    }

    #[test]
    fn dispatch_404_for_assets_in_phase_2() {
        let r = dispatch(&req("GET", "/assets/main.js"), &opts(true, true), AuthOutcome::Admin);
        assert_eq!(r.status, 404);
    }

    #[test]
    fn is_admin_only_recognises_logs() {
        assert!(is_admin_only("/api/logs"));
        assert!(is_admin_only("/api/logs?since=10"));
        assert!(is_admin_only("/api/prepared/text/abc"));
        assert!(is_admin_only("/api/interner/top"));
    }

    #[test]
    fn is_admin_only_does_not_match_public() {
        assert!(!is_admin_only("/api/overview"));
        assert!(!is_admin_only("/api/pools"));
        assert!(!is_admin_only("/api/prepared"));
    }
}
```

- [ ] **Step 4.2: Подключить `pub mod server;` в `src/web/mod.rs`**

В файле `src/web/mod.rs` добавить (после `pub mod auth;`):

```rust
pub mod server;

pub use server::{start_web_server, WebServerOptions};
```

- [ ] **Step 4.3: cargo build** (всё ещё падает в `src/app/server.rs` и `src/web/metrics/tests.rs` — Task 5/7)

```bash
cargo build --lib 2>&1 | tail -10
```
Expected: ошибки только в `src/app/server.rs:361` (call site) и `src/web/metrics/tests.rs` (старое имя функции).

- [ ] **Step 4.4: cargo test --lib web::server::tests** должен пройти

```bash
cargo test --lib web::server::tests 2>&1 | tail -20
```
Expected: 13 тестов passed.

- [ ] **Step 4.5: Не коммитить.**

---

## Task 5: Обновить `src/app/server.rs` — call site + default-password gate

**Files:**
- Modify: `src/app/server.rs:1` (use crate::web::start_web_server, удалить старый use из crate::web::metrics).
- Modify: `src/app/server.rs:360-368` (default-password gate + новый вызов).

- [ ] **Step 5.1: Прочитать текущий blob и обновить imports**

Найти в `src/app/server.rs` строку:
```rust
use crate::web::metrics::{record_interner_gc, start_prometheus_server};
```
Заменить на:
```rust
use crate::web::metrics::record_interner_gc;
use crate::web::{start_web_server, WebServerOptions};
```

- [ ] **Step 5.2: Обновить блок спауна**

Найти блок (around lines 358-368):
```rust
// Prometheus metrics exporter
if config.web.enabled {
    tokio::task::spawn(async move {
        start_prometheus_server(
            format!("{}:{}", config.web.host, config.web.port).as_str(),
        )
        .await;
    });
}
```

Заменить на:
```rust
// Web listener (Prometheus exporter + optional UI)
if config.web.enabled {
    let admin_password_is_default =
        config.general.admin_password.is_empty() || config.general.admin_password == "admin";
    let ui_active = if config.web.ui {
        if admin_password_is_default {
            log::warn!(
                "web.ui = true ignored: admin_password is default/empty. \
                 Set a real admin_password to enable the UI; /metrics keeps working."
            );
            false
        } else {
            true
        }
    } else {
        false
    };
    let host = format!("{}:{}", config.web.host, config.web.port);
    let opts = WebServerOptions {
        ui_active,
        ui_anonymous: config.web.ui_anonymous,
        admin_username: config.general.admin_username.clone(),
        admin_password: config.general.admin_password.clone(),
    };
    tokio::task::spawn(async move {
        start_web_server(&host, opts).await;
    });
}
```

- [ ] **Step 5.3: cargo build — должен скомпилироваться, но `src/web/metrics/tests.rs` всё ещё падает**

```bash
cargo build --lib 2>&1 | tail -10
```
Expected: оставшаяся ошибка только в `src/web/metrics/tests.rs` (старый вызов `start_prometheus_server`). Чиним в Task 7.

- [ ] **Step 5.4: Не коммитить.**

---

## Task 6: Integration test для real listener

**Files:**
- Create: `src/web/tests.rs` (новый файл с integration tests).
- Modify: `src/web/mod.rs` (`#[cfg(test)] mod tests;`).

- [ ] **Step 6.1: Создать `src/web/tests.rs`**

```rust
//! End-to-end smoke tests for the web listener mux.
//!
//! Each test spawns a real listener on `127.0.0.1:<random port>` (chosen via
//! `portpicker`, already in dev-dependencies), opens a TcpStream and sends a
//! hand-rolled HTTP/1.1 request line + headers. We avoid pulling reqwest into
//! dev-deps for these few cases.

use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::web::{start_web_server, WebServerOptions};

fn opts(ui_active: bool, ui_anonymous: bool) -> WebServerOptions {
    WebServerOptions {
        ui_active,
        ui_anonymous,
        admin_username: "admin".into(),
        admin_password: "secret".into(),
    }
}

async fn spawn_server(opts: WebServerOptions) -> u16 {
    let port = portpicker::pick_unused_port().expect("free port");
    let host = format!("127.0.0.1:{port}");
    tokio::spawn(async move {
        start_web_server(&host, opts).await;
    });
    // small wait for bind; matches existing test pattern
    tokio::time::sleep(Duration::from_millis(150)).await;
    port
}

async fn send(port: u16, request: &str) -> String {
    let mut stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(("127.0.0.1", port)))
        .await
        .expect("connect timeout")
        .expect("connect");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await;
    String::from_utf8_lossy(&response).into_owned()
}

#[tokio::test]
async fn metrics_endpoint_serves_prometheus_body_when_ui_inactive() {
    let port = spawn_server(opts(false, true)).await;
    let raw = send(port, "GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.contains("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("pg_doorman_"), "raw={raw}");
}

#[tokio::test]
async fn api_returns_404_when_ui_inactive() {
    let port = spawn_server(opts(false, true)).await;
    let raw = send(port, "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 404"), "raw={raw}");
}

#[tokio::test]
async fn api_public_route_returns_501_when_ui_active_anonymous() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 501"), "raw={raw}");
}

#[tokio::test]
async fn api_admin_route_returns_401_without_auth() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/logs HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
    assert!(raw.contains("WWW-Authenticate: Basic"), "raw={raw}");
}

#[tokio::test]
async fn api_admin_route_with_auth_returns_501() {
    let port = spawn_server(opts(true, true)).await;
    let creds = base64::engine::general_purpose::STANDARD.encode("admin:secret");
    let req = format!(
        "GET /api/logs HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {creds}\r\n\r\n"
    );
    let raw = send(port, &req).await;
    assert!(raw.starts_with("HTTP/1.1 501"), "raw={raw}");
}

#[tokio::test]
async fn api_public_route_returns_401_when_ui_anonymous_false() {
    let port = spawn_server(opts(true, false)).await;
    let raw = send(port, "GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 401"), "raw={raw}");
}
```

- [ ] **Step 6.2: Подключить tests-модуль в `src/web/mod.rs`**

Добавить в `src/web/mod.rs` (в самом конце):

```rust
#[cfg(test)]
mod tests;
```

- [ ] **Step 6.3: cargo test --lib web::tests** должен пройти

```bash
cargo test --lib web::tests:: 2>&1 | tail -20
```
Expected: 6 passed.

- [ ] **Step 6.4: Не коммитить.**

---

## Task 7: Обновить `src/web/metrics/tests.rs`

**Files:**
- Modify: `src/web/metrics/tests.rs`.

- [ ] **Step 7.1: Прочитать текущие тесты**

Открыть `src/web/metrics/tests.rs`. Тест `test_prometheus_server_basic` сейчас вызывает `start_prometheus_server(server_addr)` — функция удалена в Task 3. Заменить вызов на новую сигнатуру:

```rust
use crate::web::{start_web_server, WebServerOptions};

let opts = WebServerOptions {
    ui_active: false,
    ui_anonymous: true,
    admin_username: "admin".into(),
    admin_password: "secret".into(),
};
let server_handle = tokio::spawn(async move {
    start_web_server(server_addr, opts).await;
});
```

(Сохранить остальной код теста: счётчики, sleep, HTTP-запрос — без изменений.)

То же самое для `test_prometheus_server_integration` (если использует тот же паттерн).

- [ ] **Step 7.2: cargo test --lib web::metrics:: должен пройти**

```bash
cargo test --lib web::metrics::tests:: 2>&1 | tail -10
```
Expected: один или два теста проходят (один из них помечен `#[ignore]`).

- [ ] **Step 7.3: Полный прогон тестов**

```bash
cargo test --lib 2>&1 | tail -3
```
Expected: 634 (baseline) + 9 (auth) + 13 (server unit) + 6 (web integration) = **662 passed**, 0 failed, 1 ignored. Если число другое — найти расхождение.

- [ ] **Step 7.4: clippy + fmt**

```bash
cargo clippy --lib -- --deny warnings 2>&1 | tail -5
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 7.5: Не коммитить.**

---

## Task 8: Final-проверка + commit

**Files:** none modified.

- [ ] **Step 8.1: cargo fmt + clippy + test (полный)**

```bash
cargo fmt
cargo clippy --lib -- --deny warnings 2>&1 | tail -5
cargo test --lib 2>&1 | tail -3
```
Expected: clean, 662 passed.

- [ ] **Step 8.2: Smoke check `/metrics` (как в фазе 1)**

```bash
cargo build --release 2>&1 | tail -3
cat > /tmp/doorman-phase2-web.toml <<'EOF'
[general]
host = "127.0.0.1"
port = 16432
admin_username = "admin"
admin_password = "phase2test"

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
./target/release/pg_doorman /tmp/doorman-phase2-web.toml > /tmp/doorman-phase2-web.log 2>&1 &
DPID=$!
sleep 2
echo "--- /metrics smoke ---"
curl -s http://127.0.0.1:19127/metrics | head -3
echo "--- /api/overview without auth (ui_anonymous=true default) ---"
curl -is http://127.0.0.1:19127/api/overview | head -3
echo "--- /api/logs without auth (admin-only) ---"
curl -is http://127.0.0.1:19127/api/logs | head -3
echo "--- /api/logs with auth ---"
curl -is -u admin:phase2test http://127.0.0.1:19127/api/logs | head -3
kill $DPID
wait $DPID 2>/dev/null
```
Expected:
- `/metrics` → 200 OK + `pg_doorman_*` метрики.
- `/api/overview` без auth → 501 Not Implemented (public path, ui_anonymous=true default).
- `/api/logs` без auth → 401 + `WWW-Authenticate: Basic realm="pg_doorman admin"`.
- `/api/logs` с правильным auth → 501.

- [ ] **Step 8.3: Smoke check default-password gate**

Поднять doorman с `admin_password = "admin"` и `ui = true` — UI должен быть отключён, /api/* отдавать 404, /metrics работать:

```bash
cat > /tmp/doorman-phase2-default-pass.toml <<'EOF'
[general]
host = "127.0.0.1"
port = 16432
admin_username = "admin"
admin_password = "admin"

[web]
enabled = true
host = "127.0.0.1"
port = 19128
ui = true

[pools.smoke]
server_host = "localhost"
server_port = 5432

[[pools.smoke.users]]
username = "u"
password = "p"
pool_size = 5
EOF
./target/release/pg_doorman /tmp/doorman-phase2-default-pass.toml > /tmp/doorman-phase2-defp.log 2>&1 &
DPID=$!
sleep 2
echo "--- expect warn line in log ---"
grep "web.ui = true ignored" /tmp/doorman-phase2-defp.log
echo "--- /metrics still 200 ---"
curl -is http://127.0.0.1:19128/metrics | head -1
echo "--- /api/overview should 404 (ui inactive) ---"
curl -is http://127.0.0.1:19128/api/overview | head -1
kill $DPID
wait $DPID 2>/dev/null
```
Expected:
- log содержит строку "web.ui = true ignored: admin_password is default/empty. ...".
- `/metrics` → 200 OK.
- `/api/overview` → 404 Not Found (ui inactive).

- [ ] **Step 8.4: Pre-commit code review**

Согласно `~/.claude/CLAUDE.md` правилу — диспатч Agent (`subagent_type: general-purpose`, `model: opus`) с черновиком commit-сообщения и инструкциями из CLAUDE.md.

Черновик commit-сообщения:

```
feat(web): add HTTP mux with basic-auth and default-password gate

The web listener now serves more than /metrics: when [web].ui = true and
admin_password is non-default, GET /api/* and the SPA paths participate
in dispatch. /metrics behaviour is byte-identical to before — the same
listener routes it. Operators with default or empty admin_password see a
single warning line and the UI stays off; /metrics still works for them.

Public /api/* routes are gated by [web].ui_anonymous; admin-only paths
(/api/logs, /api/prepared/text/, /api/interner/top) always require
basic-auth. Phase 2 ships only the gating — every /api/* request that
makes it through auth returns 501 with a stub body. Real handlers land
in phase 3.

The auth check uses constant-time credential comparison via the subtle
crate to deny timing oracles.

Tests: 662 passed (was 634 baseline + 28 new), clippy/fmt clean.
Verified by release-build smoke against /metrics, /api/overview,
/api/logs (anonymous and authenticated), and the default-password
configuration.

Phase 2 of seven; phase 3 fills /api/* with real handlers.
```

Если ревью даёт блокеры — fix, repeat.

- [ ] **Step 8.5: Создать единый коммит фазы 2**

```bash
git add -A
git status
git commit -m "$(cat <<'EOF'
<finalised commit message from review>
EOF
)"
git log --oneline -3
git status
```

Expected: working tree clean, новый коммит в head.

---

## Self-review

**Spec coverage check:**
- ✅ Mux dispatch (раздел 4.1) — Task 4.
- ✅ Admin-only paths гарантированно требуют auth (раздел 6.1) — Task 4 (ADMIN_ONLY_PREFIXES + dispatch).
- ✅ Public paths гейтятся ui_anonymous (раздел 6.1) — Task 4.
- ✅ Basic-auth парсер с constant-time compare (раздел 6.2) — Tasks 1-2.
- ✅ Default-password gate (раздел 5.4) — Task 5.
- ✅ /metrics не меняется — Task 4 (ранний return до auth/dispatch), Task 8.2 (smoke).
- ✅ Backwards compat smoke (Task 8.3 — default password leaves /metrics intact).

**Не покрыто этой фазой (намеренно):**
- Реальные /api/* handlers — фаза 3.
- LogTap — фаза 4.
- Frontend / SPA / static_assets — фазы 5-7.
- BDD-сценарии (web_anonymous, web_admin, web_default_password, web_log_tap из спеки 12.3) — будут добавляться по ходу фаз 3-4.
- Полный аудит-чеклист WCAG / keyboard / freshness — это frontend, фазы 5-6.

**Type-consistency check:**
- `WebServerOptions` consistent everywhere (Tasks 4, 5, 6, 7).
- `AuthOutcome` enum referenced from server.rs and tests.
- `start_web_server(host: &str, opts: WebServerOptions)` сигнатура одинакова в Tasks 4, 5, 6.
- `classify(authorization: Option<&str>, user: &str, pass: &str) -> AuthOutcome` — fixed.

**Placeholder check:** обыскал — нет «TBD» / «implement later» / «similar to Task N». Все шаги содержат конкретный код или команду.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-2.md`. Same execution choice as phase 1: subagent-driven recommended; controller dispatches one implementer per task plus two-stage review.
