# Web UI — Phase 3d-1 Implementation Plan: top-clients + apps aggregation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Поднять `GET /api/top/clients?by=qps|errors|age&n=20&pool=` (Top-N клиентов) и `GET /api/apps?sort=&order=` (aggregation по `application_name`). Это первая часть phase 3d (killer-feature triage); никаких backend changes — данные уже есть в `ClientStats`.

**Architecture:** Чистый read-only поверх существующих global-state structures. `collect_top_clients(filters)` берёт snapshot всех клиентов, фильтрует по pool, сортирует по выбранному by, обрезает по n. `collect_apps(sort, order)` группирует клиентов по `application_name`, суммирует counters. Handlers — тривиальные обёртки.

**Sort semantics:**
- `/api/top/clients`: `by=qps` сортирует по `queries_total / age_seconds.max(1)` (serverside computed). `by=errors` сортирует по `error_count`. `by=age` сортирует по `age_seconds`. Default `qps`. Все sorts descending — мы хотим самых нагруженных сверху.
- `/api/apps`: `sort=clients|queries|transactions|errors` (все cumulative, frontend считает rate'ы делением). `order=asc|desc`, default `desc`. Default `sort=clients`.

**Decision #21 deviation:** `/api/top/clients` единственное место где server вычисляет производное (qps) — потому что sort_by требует значения. Альтернатива была бы выдавать raw counters и сортировать на frontend, что плохо когда total списка тысячи клиентов. Это контролируемое исключение, согласованное со spec section 8.2 («Top-N как core feature triage»).

**Reference:**
- Spec section 8.2 (полный endpoint list + query params).
- Phase 3c-3 commit: `a18b8da`.
- Phase 3b commit: `e1999c2` (паттерн filter+sort+pagination для clients).

**Не входит в фазу 3d-1:**
- `/api/top/queries`, `/api/top/prepared`, `/api/events` — следующие sub-фазы 3d-2/3d-3/3d-4.

---

## File Structure

**Новые файлы:**
- `src/web/routes/top_clients.rs` — handler `GET /api/top/clients`.
- `src/web/routes/apps.rs` — handler `GET /api/apps`.

**Модифицируемые файлы:**
- `src/web/routes/dto.rs` — `TopClientsDto`, `TopClientRowDto`, `TopClientBy`, `AppsDto`, `AppRowDto`, `AppSort`.
- `src/web/routes/collect.rs` — `collect_top_clients(filters)`, `collect_apps(filters)`, плюс pure helpers `top_clients_from(snapshot, filters)` и `apps_from(snapshot, filters)` для unit tests.
- `src/web/routes/mod.rs` — register 2 modules.
- `src/web/server.rs` — 2 arm'а в `route_api` + 2 dispatch tests.
- `src/web/tests.rs` — 2 integration tests.

---

## Task 0: Baseline проверка

- [ ] **Step 0.1**

```bash
cd /home/vadv/Projects/pg_doorman
git status
git log --oneline -3
```
Expected: HEAD = `a18b8da`, working tree clean.

- [ ] **Step 0.2**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 778 passed, clean.

---

## Task 1: DTOs

**Files:** Modify `src/web/routes/dto.rs`.

- [ ] **Step 1.1: TopClients**

```rust
/// `GET /api/top/clients` — Top-N clients by qps / errors / age.
#[derive(Debug, Serialize)]
pub struct TopClientsDto {
    pub ts: u64,
    /// The sort dimension actually used: `"qps"`, `"errors"`, `"age"`.
    pub by: String,
    /// The clamped value of `n` actually used (1..=MAX_TOP_N).
    pub n: u64,
    pub clients: Vec<TopClientRowDto>,
}

#[derive(Debug, Serialize)]
pub struct TopClientRowDto {
    /// `"#cN"` form — matches `ClientDto.client_id`.
    pub client_id: String,
    pub application_name: String,
    pub user: String,
    pub database: String,
    pub addr: String,
    pub age_seconds: u64,
    pub queries_total: u64,
    pub errors_total: u64,
    /// Server-side computed `queries_total / age_seconds.max(1)`, exposed
    /// for parity with the `by=qps` sort dimension and so the frontend
    /// does not have to recompute when rendering the table column.
    pub qps: f64,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum TopClientBy {
    #[default]
    Qps,
    Errors,
    Age,
}

impl TopClientBy {
    pub fn as_str(self) -> &'static str {
        match self {
            TopClientBy::Qps => "qps",
            TopClientBy::Errors => "errors",
            TopClientBy::Age => "age",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TopClientFilters {
    pub by: TopClientBy,
    pub n: u64,
    pub pool: Option<String>,
}
```

- [ ] **Step 1.2: Apps**

```rust
/// `GET /api/apps` — per-application_name aggregate of client counters.
#[derive(Debug, Serialize)]
pub struct AppsDto {
    pub ts: u64,
    pub apps: Vec<AppRowDto>,
}

#[derive(Debug, Serialize)]
pub struct AppRowDto {
    pub application_name: String,
    /// Number of currently-connected clients reporting this application_name.
    pub clients: u64,
    /// Cumulative counters; frontend computes rates from successive snapshots.
    pub queries_total: u64,
    pub transactions_total: u64,
    pub errors_total: u64,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum AppSort {
    #[default]
    Clients,
    Queries,
    Transactions,
    Errors,
}

impl AppSort {
    pub fn as_str(self) -> &'static str {
        match self {
            AppSort::Clients => "clients",
            AppSort::Queries => "queries",
            AppSort::Transactions => "transactions",
            AppSort::Errors => "errors",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct AppFilters {
    pub sort: AppSort,
    pub order: SortOrder,
}
```

- [ ] **Step 1.3: cargo build + clippy + fmt**

Expected: clean.

- [ ] **Step 1.4: Не коммитить.**

---

## Task 2: collect-функции + unit-тесты

**Files:** Modify `src/web/routes/collect.rs`.

- [ ] **Step 2.1: Расширить use-импорты**

```rust
use crate::web::routes::dto::{
    AppFilters, AppRowDto, AppSort, AppsDto, TopClientBy, TopClientFilters, TopClientRowDto,
    TopClientsDto,
    // existing entries
};
```

- [ ] **Step 2.2: pure helper `clamp_top_n_value`**

В collect.rs ниже существующего `clamp_top_n` (для interner) добавить:

```rust
/// Clamps `?n=` for the Top-N client/apps endpoints. Same shape as
/// `clamp_top_n` for interner top, kept as a separate function so changing
/// the interner cap doesn't affect these page-sized lists.
pub(crate) fn clamp_top_clients_n(requested: u64) -> u64 {
    const DEFAULT: u64 = 20;
    const MAX: u64 = 200;
    match requested {
        0 => DEFAULT,
        n if n > MAX => MAX,
        n => n,
    }
}
```

- [ ] **Step 2.3: `collect_top_clients`**

```rust
pub fn collect_top_clients(filters: &TopClientFilters) -> TopClientsDto {
    let snapshot: Vec<_> = get_client_stats().values().cloned().collect();
    top_clients_from(snapshot, filters)
}

fn top_clients_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ClientStats>>,
    filters: &TopClientFilters,
) -> TopClientsDto {
    let n = clamp_top_clients_n(filters.n);

    let mut rows: Vec<TopClientRowDto> = snapshot
        .iter()
        .filter(|s| {
            if let Some(p) = &filters.pool {
                let id = format!("{}@{}", s.username(), s.pool_name());
                if id != *p {
                    return false;
                }
            }
            true
        })
        .map(|s| {
            let age_seconds = s.connect_time().elapsed().as_secs();
            let queries_total = s.query_count.load(std::sync::atomic::Ordering::Relaxed);
            let errors_total = s.error_count.load(std::sync::atomic::Ordering::Relaxed);
            let qps = queries_total as f64 / age_seconds.max(1) as f64;
            TopClientRowDto {
                client_id: format!("#c{}", s.connection_id()),
                application_name: s.application_name(),
                user: s.username(),
                database: s.pool_name(),
                addr: s.ipaddr(),
                age_seconds,
                queries_total,
                errors_total,
                qps,
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        // All Top-N sorts are descending — operators want busiest first.
        match filters.by {
            TopClientBy::Qps => b
                .qps
                .partial_cmp(&a.qps)
                .unwrap_or(std::cmp::Ordering::Equal),
            TopClientBy::Errors => b.errors_total.cmp(&a.errors_total),
            TopClientBy::Age => b.age_seconds.cmp(&a.age_seconds),
        }
    });

    let clients: Vec<_> = rows.into_iter().take(n as usize).collect();

    TopClientsDto {
        ts: now_unix_ms(),
        by: filters.by.as_str().to_string(),
        n,
        clients,
    }
}
```

**Verify:** `connection_id()` accessor существует в `ClientStats`. Если только поле приватное — использовать `format!("#c{}", s.connection_id)` или искать аналог в `ClientDto` builder из collect.rs.

- [ ] **Step 2.4: `collect_apps`**

```rust
pub fn collect_apps(filters: &AppFilters) -> AppsDto {
    let snapshot: Vec<_> = get_client_stats().values().cloned().collect();
    apps_from(snapshot, filters)
}

fn apps_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ClientStats>>,
    filters: &AppFilters,
) -> AppsDto {
    use std::collections::HashMap;

    let mut acc: HashMap<String, AppRowDto> = HashMap::new();
    for s in &snapshot {
        let app = s.application_name();
        let entry = acc.entry(app.clone()).or_insert_with(|| AppRowDto {
            application_name: app,
            clients: 0,
            queries_total: 0,
            transactions_total: 0,
            errors_total: 0,
        });
        entry.clients += 1;
        entry.queries_total += s.query_count.load(std::sync::atomic::Ordering::Relaxed);
        entry.transactions_total += s
            .transaction_count
            .load(std::sync::atomic::Ordering::Relaxed);
        entry.errors_total += s.error_count.load(std::sync::atomic::Ordering::Relaxed);
    }

    let mut apps: Vec<AppRowDto> = acc.into_values().collect();
    apps.sort_by(|a, b| {
        let ord = match filters.sort {
            AppSort::Clients => a.clients.cmp(&b.clients),
            AppSort::Queries => a.queries_total.cmp(&b.queries_total),
            AppSort::Transactions => a.transactions_total.cmp(&b.transactions_total),
            AppSort::Errors => a.errors_total.cmp(&b.errors_total),
        };
        match filters.order {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        }
    });

    AppsDto {
        ts: now_unix_ms(),
        apps,
    }
}
```

- [ ] **Step 2.5: Unit-тесты на `clamp_top_clients_n` + `top_clients_from` + `apps_from`**

В существующий `mod tests` добавить тесты:

```rust
    // -------------------------------------------------------------------------
    // Top-clients clamp helper
    // -------------------------------------------------------------------------

    #[test]
    fn clamp_top_clients_n_zero_returns_default() {
        assert_eq!(super::clamp_top_clients_n(0), 20);
    }

    #[test]
    fn clamp_top_clients_n_keeps_in_range() {
        assert_eq!(super::clamp_top_clients_n(1), 1);
        assert_eq!(super::clamp_top_clients_n(50), 50);
        assert_eq!(super::clamp_top_clients_n(200), 200);
    }

    #[test]
    fn clamp_top_clients_n_caps_above_max() {
        assert_eq!(super::clamp_top_clients_n(201), 200);
        assert_eq!(super::clamp_top_clients_n(u64::MAX), 200);
    }

    // -------------------------------------------------------------------------
    // Top-clients sort
    // -------------------------------------------------------------------------

    #[test]
    fn top_clients_sort_by_errors_desc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 5),
            make_client(2, "db", "u", "a", 0, 1),
            make_client(3, "db", "u", "a", 0, 3),
        ];
        let f = TopClientFilters {
            by: TopClientBy::Errors,
            n: 10,
            pool: None,
        };
        let result = super::top_clients_from(clients, &f);
        let errs: Vec<u64> = result.clients.iter().map(|c| c.errors_total).collect();
        assert_eq!(errs, vec![5, 3, 1]);
        assert_eq!(result.by, "errors");
    }

    #[test]
    fn top_clients_n_default_when_zero() {
        let clients: Vec<_> = (0..5).map(|i| make_client(i, "db", "u", "a", 0, 0)).collect();
        let f = TopClientFilters {
            by: TopClientBy::Qps,
            n: 0,
            pool: None,
        };
        let result = super::top_clients_from(clients, &f);
        assert_eq!(result.n, 20);
    }

    #[test]
    fn top_clients_pool_filter_excludes_others() {
        let clients = vec![
            make_client(1, "db1", "alice", "a", 0, 0),
            make_client(2, "db2", "bob", "a", 0, 0),
        ];
        let f = TopClientFilters {
            by: TopClientBy::Qps,
            n: 10,
            pool: Some("alice@db1".to_string()),
        };
        let result = super::top_clients_from(clients, &f);
        assert_eq!(result.clients.len(), 1);
        assert_eq!(result.clients[0].user, "alice");
    }

    // -------------------------------------------------------------------------
    // Apps aggregation
    // -------------------------------------------------------------------------

    #[test]
    fn apps_aggregate_counts_clients_per_application_name() {
        let clients = vec![
            make_client(1, "db", "u", "appA", 10, 0),
            make_client(2, "db", "u", "appA", 20, 0),
            make_client(3, "db", "u", "appB", 5, 0),
        ];
        let f = AppFilters {
            sort: AppSort::Clients,
            order: SortOrder::Desc,
        };
        let result = super::apps_from(clients, &f);
        let appA = result.apps.iter().find(|a| a.application_name == "appA").unwrap();
        let appB = result.apps.iter().find(|a| a.application_name == "appB").unwrap();
        assert_eq!(appA.clients, 2);
        assert_eq!(appA.queries_total, 30);
        assert_eq!(appB.clients, 1);
        assert_eq!(appB.queries_total, 5);
    }

    #[test]
    fn apps_sort_by_queries_desc() {
        let clients = vec![
            make_client(1, "db", "u", "appA", 10, 0),
            make_client(2, "db", "u", "appB", 100, 0),
            make_client(3, "db", "u", "appC", 50, 0),
        ];
        let f = AppFilters {
            sort: AppSort::Queries,
            order: SortOrder::Desc,
        };
        let result = super::apps_from(clients, &f);
        let names: Vec<_> = result.apps.iter().map(|a| a.application_name.clone()).collect();
        assert_eq!(names, vec!["appB", "appC", "appA"]);
    }
```

- [ ] **Step 2.6: cargo build + test + clippy + fmt**

Expected: 41 collect tests passing (36 baseline + 3 clamp + 2 top + 2 apps), clean.

- [ ] **Step 2.7: Не коммитить.**

---

## Task 3: handlers

**Files:**
- Create: `src/web/routes/top_clients.rs`, `src/web/routes/apps.rs`.
- Modify: `src/web/routes/mod.rs`.

- [ ] **Step 3.1: `top_clients.rs`**

```rust
//! GET /api/top/clients?by=qps|errors|age&n=20&pool= handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_top_clients;
use crate::web::routes::dto::{TopClientBy, TopClientFilters};
use crate::web::routes::query::{first, parse_u64};
use crate::web::server::Response;

pub(crate) fn handle_top_clients(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = TopClientFilters {
        by: match first(query, "by").as_deref() {
            Some("errors") => TopClientBy::Errors,
            Some("age") => TopClientBy::Age,
            _ => TopClientBy::Qps,
        },
        n: parse_u64(query, "n", 0),
        pool: first(query, "pool"),
    };
    Response::ok_json(&collect_top_clients(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_clients_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_top_clients(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"qps\""));
        assert!(body.contains("\"n\":20"));
        assert!(body.contains("\"clients\""));
    }

    #[test]
    fn top_clients_by_errors_param() {
        let mut q = BTreeMap::new();
        q.insert("by".into(), vec!["errors".into()]);
        let r = handle_top_clients(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"by\":\"errors\""));
    }
}
```

- [ ] **Step 3.2: `apps.rs`**

```rust
//! GET /api/apps?sort=&order= handler.

use std::collections::BTreeMap;

use crate::web::routes::collect::collect_apps;
use crate::web::routes::dto::{AppFilters, AppSort, SortOrder};
use crate::web::routes::query::first;
use crate::web::server::Response;

pub(crate) fn handle_apps(query: &BTreeMap<String, Vec<String>>) -> Response {
    let filters = AppFilters {
        sort: match first(query, "sort").as_deref() {
            Some("queries") => AppSort::Queries,
            Some("transactions") => AppSort::Transactions,
            Some("errors") => AppSort::Errors,
            _ => AppSort::Clients,
        },
        order: match first(query, "order").as_deref() {
            Some("asc") => SortOrder::Asc,
            _ => SortOrder::Desc,
        },
    };
    Response::ok_json(&collect_apps(&filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_returns_200_with_envelope() {
        let q = BTreeMap::new();
        let r = handle_apps(&q);
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"apps\""));
    }
}
```

- [ ] **Step 3.3: `mod.rs` register**

```rust
pub(crate) mod apps;
pub(crate) mod top_clients;
```

(в alphabetical position)

- [ ] **Step 3.4: cargo build + test**

Expected: 3 handler tests passing.

- [ ] **Step 3.5: Не коммитить.**

---

## Task 4: mux integration

**Files:** Modify `src/web/server.rs`.

- [ ] **Step 4.1: 2 arm'а в `route_api`**

```rust
        "/api/top/clients" => routes::top_clients::handle_top_clients(&query),
        "/api/apps" => routes::apps::handle_apps(&query),
```

(в существующий match block)

- [ ] **Step 4.2: 2 dispatch tests**

```rust
    #[test]
    fn dispatch_top_clients_returns_200() {
        let r = dispatch(
            &req("GET", "/api/top/clients"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_apps_returns_200() {
        let r = dispatch(
            &req("GET", "/api/apps"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }
```

- [ ] **Step 4.3: Прогон тестов**

Expected: 39 server tests (37 baseline + 2 new), clean.

- [ ] **Step 4.4: Не коммитить.**

---

## Task 5: integration tests

**Files:** Modify `src/web/tests.rs`.

- [ ] **Step 5.1: 2 integration tests**

```rust
#[tokio::test]
async fn api_top_clients_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/top/clients HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"by\":\"qps\""), "raw={raw}");
    assert!(raw.contains("\"n\":20"), "raw={raw}");
    assert!(raw.contains("\"clients\""), "raw={raw}");
}

#[tokio::test]
async fn api_apps_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/apps HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"apps\""), "raw={raw}");
}
```

- [ ] **Step 5.2: Полный прогон**

Expected: ~789 passed (778 + 7 collect + 3 handler + 2 dispatch + 2 integration). Уточнить при прогоне.

- [ ] **Step 5.3: Не коммитить.**

---

## Task 6: Final-проверка + commit

- [ ] **Step 6.1: cargo fmt + clippy + test**

- [ ] **Step 6.2: Smoke release**

```bash
cargo build --release 2>&1 | tail -3
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3d1.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/top/clients ---"
curl -s 'http://127.0.0.1:19127/api/top/clients?by=errors&n=10' | head -c 500
echo ""
echo "--- /api/apps ---"
curl -s 'http://127.0.0.1:19127/api/apps?sort=clients&order=desc' | head -c 500
echo ""
kill $DPID 2>/dev/null
wait $DPID 2>/dev/null
```

- [ ] **Step 6.3: Pre-commit code review**

Через Agent (правило CLAUDE.md). Черновик:

```
feat(web): land /api/top/clients and /api/apps

The Web UI's killer-feature triage data is now backed by two new
endpoints. /api/top/clients answers "which connection is hammering
the pooler right now" by sorting clients server-side by qps,
errors, or age, optionally narrowed to a single pool. /api/apps
gives the per-application_name aggregate (clients, queries_total,
transactions_total, errors_total) so an operator can spot the
service that is opening too many connections or generating too
many errors at a glance.

Sort dimensions and the n cap (default 20, max 200) are documented
on the DTOs. /api/top/clients computes qps server-side as
queries_total / max(age_seconds, 1) — that is the one server-side
derivation in the rollout, justified because a Top-N sort by qps
needs the value to compare. Other counters stay raw, frontend
computes rates.

No backend instrumentation added — both endpoints read existing
ClientStats counters. SQL hot path is untouched.

Tests: <N> lib tests passed (was 778); cargo clippy --lib and
cargo fmt --check clean. Verified by release smoke on both
endpoints with the by= and sort= parameters.

Phase 3d-1 of seven; phase 3d-2 lands /api/top/queries together
with the per-interner-entry count + duration instrumentation.
```

- [ ] **Step 6.4: commit**

---

## Self-review

**Spec coverage check:**
- ✅ `/api/top/clients` (раздел 8.2 «Top-N клиентов по QPS / errors / connection age») — Tasks 1, 2, 3.
- ✅ `/api/apps` (раздел 8.2 «Aggregation по application_name: clients, qps, tps, errors per app») — Tasks 1, 2, 3.

**Hot path constraint:** ZERO new atomic operations in SQL hot path. Оба endpoint'а только читают ClientStats атомики, которые уже инкрементятся на существующих путях. Сборка снапшота происходит ON DEMAND при HTTP-запросе.

**Не покрыто этой фазой:**
- `/api/top/queries` — phase 3d-2 (requires per-interner-entry count + duration instrumentation, добавит 2 fetch_add per query).
- `/api/top/prepared` — phase 3d-3 (requires per-CacheEntry hit/miss instrumentation).
- `/api/events` — phase 3d-4 (admin events ring buffer, без hot path).

**Type-consistency check:** Поля DTO согласованы с phase 3b ClientDto (`client_id`, `application_name`, `user`, `database`, `addr`). Sort dimensions consistent (`asc/desc`).

**Placeholder check:** Нет placeholder-полей. Все значения из реальных источников.

---

## Execution Handoff

Plan complete. Subagent-driven execution.
