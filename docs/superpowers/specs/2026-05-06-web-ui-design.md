# pg_doorman Web UI — Design

Дата: 2026-05-06.
Ветка: `feat/client-cache-anonymous-lru` (на момент брейншторма; имплементация пойдёт отдельной веткой).
Целевой релиз: следующий минор (3.8.0 либо ближайший по календарю).

## 1. Контекст и цель

В pg_doorman есть `/metrics` (Prometheus exporter) и админ-консоль через `psql` к admin-порту. Этого достаточно для долговременного мониторинга (Grafana поверх метрик) и точечных операций, но плохо подходит для двух сценариев:

1. Оперативный обзор «что прямо сейчас происходит в пулере» — список клиентов, серверов, состояний пулов, потоки ошибок. Метрики не показывают индивидуальные соединения; psql admin требует знать команды и форматирует таблицы под терминал.
2. Live-tail логов на инциденте — сейчас оператор должен иметь shell-доступ к машине и `tail -F` соответствующий файл. Для удалённых инсталляций это лишнее препятствие.

Web UI закрывает оба сценария, не вводя новых сетевых сущностей: переиспользует тот же listener, что обслуживает `/metrics`.

## 2. Scope MVP / non-goals

**В MVP входит:**
- Observability-страницы: Overview (live counters + 7 sparkline-графиков), Pools, Clients, Servers, Prepared, Interner, Config.
- Live-tail логов в браузере (admin-only).
- Конфиг-флаги, anonymous-режим, basic-auth для admin-путей.
- React + TypeScript SPA, embedded в бинарь.

**Явно не в MVP:**
- Никаких write-команд через UI: `PAUSE / RESUME / RECONNECT / RELOAD / SHUTDOWN / UPGRADE / SET log_level / RESET INTERNER` — только psql admin.
- Никакого WebSocket/SSE — push добавим, если polling окажется слабым местом.
- Никакого серверного хранилища истории — графики живут в JS-памяти браузера (sessionStorage).
- Никакой темизации, i18n, мобильной адаптации — оставлено на follow-up.
- Никакого per-pool breakdown на графиках в Overview — только агрегаты.

## 3. Decision log

| # | Вопрос | Выбор |
|---|---|---|
| 1 | MVP scope | Observability + live-tail логов |
| 2 | Доставка логов | Polling `/api/logs?since=…` |
| 3 | HTTP-роутинг и зависимости | Руками на tokio (расширяем существующий код) |
| 4 | Конфиг-флаги | Один флаг `ui` + `ui_anonymous` + реюз `admin_username/password` |
| 5 | LogTap lifecycle | Lazy refcount (включается при первом запросе, reaper выключает по таймауту) |
| 6 | Набор графиков | 7 базовых, без per-pool breakdown |
| 7 | UI-стек | Полная сборка (Vite + npm) |
| 8 | Фреймворк | React + TypeScript |
| 9 | Каркас навигации | Sidebar nav |
| 10 | Расположение модуля | Перенести `src/prometheus/` → `src/web/` |
| 11 | Имя config-секции | `[web]` с `serde(alias = "prometheus")` |
| 12 | Refactoring `admin/show.rs` | Extract pure `collect_*()` functions, две сериализации (pg / json) |
| 13 | Дефолтный пароль + UI | Не поднимать UI с warning'ом, listener для `/metrics` продолжает работать |

## 4. Архитектура

### 4.1 Топология listener'а на `:9127`

```
GET /metrics              → web::metrics::handle (no auth, как сегодня)
GET /api/<resource>       → web::routes::* (auth по правилам ниже)
GET / | /assets/...       → web::static_assets (отдача SPA)
```

Один listener, один порт. Если `web.ui = false` (дефолт) — mux отдаёт всё кроме `/metrics` как 404, поведение текущего prometheus-эндпоинта не меняется.

### 4.2 Реорганизация модулей

`src/prometheus/` упраздняется как самостоятельный crate-модуль; его содержимое переезжает в `src/web/`.

```
src/web/
├── mod.rs                  # pub mod {server, auth, routes, log_tap, static_assets, metrics};
├── server.rs               # listener + mux (бывший prometheus/server.rs, расширенный)
├── auth.rs                 # парсер basic-auth, constant-time compare
├── log_tap.rs              # ring buffer + lazy refcount + reaper
├── static_assets.rs        # отдача SPA через include_dir!
├── metrics/
│   ├── mod.rs              # REGISTRY, update_metrics (бывший prometheus/metrics.rs)
│   ├── system.rs           # бывший prometheus/system.rs
│   └── tests.rs            # бывший prometheus/tests.rs
└── routes/
    ├── mod.rs              # mux dispatch
    ├── overview.rs         # GET /api/overview
    ├── pools.rs            # GET /api/pools
    ├── clients.rs          # GET /api/clients
    ├── servers.rs          # GET /api/servers
    ├── connections.rs      # GET /api/connections
    ├── stats.rs            # GET /api/stats
    ├── databases.rs        # GET /api/databases
    ├── users.rs            # GET /api/users
    ├── config.rs           # GET /api/config (с маскингом секретов)
    ├── auth_query.rs       # GET /api/auth_query
    ├── log_level.rs        # GET /api/log_level
    ├── pool_scaling.rs     # GET /api/pool_scaling
    ├── pool_coordinator.rs # GET /api/pool_coordinator
    ├── sockets.rs          # GET /api/sockets (linux-only)
    ├── prepared.rs         # GET /api/prepared (агрегаты, без текстов)
    ├── prepared_text.rs    # GET /api/prepared/text/{hash} (admin)
    ├── interner.rs         # GET /api/interner (агрегаты)
    ├── interner_top.rs     # GET /api/interner/top (admin)
    ├── logs.rs             # GET /api/logs (admin)
    └── version.rs          # GET /api/version
```

`use crate::prometheus::*` по проекту → `use crate::web::metrics::*`. Замена тривиальная.

### 4.3 Frontend проект

```
frontend/
├── package.json, package-lock.json, tsconfig.json
├── vite.config.ts            # dev proxy /api/* → :9127
├── index.html
├── src/
│   ├── main.tsx, App.tsx
│   ├── api.ts, types.ts
│   ├── pages/  (Overview, Pools, Clients, Servers, Prepared, Interner, Logs, Config)
│   ├── components/  (Sidebar, Chart, Table, LogStream, AuthGate)
│   ├── hooks/  (usePoll, useHistory, useAdminAuth)
│   └── styles/tailwind.css
└── public/favicon.ico
```

Стек: React 18 + TS 5 + Vite 5 + react-router 6 + uPlot 1.6 + Tailwind v3.

### 4.4 Embedding в бинарь

`src/web/static_assets.rs` использует `include_dir!` macro:

```rust
use include_dir::{include_dir, Dir};
static SPA: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/frontend/dist");
```

CI билдит фронт перед `cargo build`. В dev — `vite dev` отдельно с proxy на `:9127`.

## 5. Конфиг

### 5.1 TOML

```toml
[web]
enabled = true        # поднимать listener (бывший [prometheus] enabled)
host = "0.0.0.0"
port = 9127
ui = false            # NEW: дать UI и /api/*
ui_anonymous = true   # NEW: public-пути без auth
log_tap_kb = 64       # NEW: capacity ring buffer'а логов
```

### 5.2 Rust-структура и backwards compat

```rust
#[derive(Serialize, Deserialize, ...)]
pub struct Web {
    #[serde(default = "Web::default_enabled")]
    pub enabled: bool,
    #[serde(default = "Web::default_host")]
    pub host: String,
    #[serde(default = "Web::default_port")]
    pub port: u16,
    #[serde(default = "Web::default_ui")]
    pub ui: bool,
    #[serde(default = "Web::default_ui_anonymous")]
    pub ui_anonymous: bool,
    #[serde(default = "Web::default_log_tap_kb")]
    pub log_tap_kb: u32,
}

// в Config:
#[serde(alias = "prometheus")]
pub web: Web,
```

`serde(alias)` позволяет старому конфигу с `[prometheus] enabled = true` работать без изменений. Документацию обновляем под новое имя; alias упоминаем сноской «`[prometheus]` остаётся валидным алиасом для обратной совместимости».

### 5.3 Дефолты

| Ключ | Дефолт | Обоснование |
|---|---|---|
| `enabled` | `false` | Совпадает с текущим `prometheus.enabled` |
| `host` | `"0.0.0.0"` | Совпадает с текущим |
| `port` | `9127` | Совпадает с текущим |
| `ui` | `false` | Opt-in, чтобы UI не появлялся у обновляющихся юзеров |
| `ui_anonymous` | `true` | Когда юзер opt-in'ил UI, default'но видно как `/metrics` |
| `log_tap_kb` | `64` | Достаточно для нескольких минут активного лога; маленький RAM-footprint |

### 5.4 Startup safety: дефолтный пароль + ui

```rust
let ui_active = if cfg.web.ui {
    if cfg.general.admin_password == "admin" || cfg.general.admin_password.is_empty() {
        log::warn!(
            "web.ui = true ignored: admin_password is default/empty. \
             Set a real admin_password to enable the UI. /metrics continues to work."
        );
        false
    } else {
        true
    }
} else {
    false
};
```

`ui_active` определяет, поднимать ли SPA-handler, `/api/*`, reaper LogTap'а. `/metrics` от этого не зависит — он живёт всегда при `web.enabled=true`.

## 6. Auth & access matrix

### 6.1 Dispatch

```
GET /metrics                  → no auth, всегда 200
GET /api/<admin-only>         → require basic-auth(admin), иначе 401
GET /api/<public> | / | /assets/*
                              → if ui_anonymous=true: pass
                                 else: require basic-auth(admin), иначе 401
```

Admin-only пути (фиксированный set, проверяется по startsWith):
- `/api/logs`
- `/api/prepared/text/`
- `/api/interner/top`

### 6.2 Парсер basic-auth

```rust
fn check_basic_auth(header: Option<&str>, user: &str, pass: &str) -> bool {
    let Some(value) = header.and_then(|h| h.strip_prefix("Basic ")) else { return false; };
    let Ok(decoded) = base64::decode(value.trim()) else { return false; };
    let Ok(s) = std::str::from_utf8(&decoded) else { return false; };
    let Some((u, p)) = s.split_once(':') else { return false; };
    use subtle::ConstantTimeEq;
    bool::from(u.as_bytes().ct_eq(user.as_bytes()))
      & bool::from(p.as_bytes().ct_eq(pass.as_bytes()))     // & вместо &&: без short-circuit
}
```

Новые deps: `base64` (~5k LOC) и `subtle` (~500 LOC, constant-time primitives). Обе крошечные, без транзитивных зависимостей.

При неуспешной auth: `401 Unauthorized` + `WWW-Authenticate: Basic realm="pg_doorman admin"` — браузер сам показывает login dialog.

## 7. Backend компоненты

### 7.1 `src/web/server.rs`

Owns `TcpListener`, accept loop, парсит request line + минимальные headers (`Authorization`, `Accept-Encoding`), вызывает `auth::check`, диспатчит в `routes::*`. Spawn-per-connection (как сейчас в `prometheus::server.rs`). Запускает reaper-task для LogTap при старте. ~200 строк.

### 7.2 `src/web/auth.rs`

Парсер `Authorization: Basic <b64>`, `check_basic_auth(...)`, `Authenticated { admin: bool }`.

### 7.3 `src/web/log_tap.rs`

```rust
pub struct LogEntry {
    pub seq: u64,
    pub ts_ms: u64,
    pub level: log::Level,
    pub target: String,
    pub message: String,
}

pub struct LogTap {
    entries: parking_lot::Mutex<VecDeque<LogEntry>>,
    current_bytes: AtomicUsize,
    max_bytes: usize,
    next_seq: AtomicU64,
    dropped_total: AtomicU64,
    last_request_at: AtomicU64,    // monotonic ms
}
```

Per-entry лимит — **4 KB** на одну строку (длинные обрезаются с маркером `…<truncated>`), чтобы один debug-лог с большим SQL не вытеснил хвост.

### 7.4 Доработка `src/app/log_level.rs`

Поле `tap: ArcSwap<Option<Arc<LogTap>>>`. В `Log::log()`:

```rust
fn log(&self, record: &Record) {
    if self.enabled(record.metadata()) {
        self.inner.log(record);
        if let Some(t) = self.tap.load().as_ref() {
            t.push(record.level(), record.target(), record.args());
        }
    }
}
```

API:
- `enable_log_tap(cap_bytes) -> Arc<LogTap>` (idempotent, CAS на `ArcSwap`)
- `disable_log_tap()`
- `log_tap() -> Option<Arc<LogTap>>`

Hot path при `tap=None`: один `ArcSwap::load` + `Option::is_none` ≈ ~5 ns. При `Some`: format + Mutex lock + `VecDeque::push_back` + eviction ≈ единицы µs.

### 7.5 Refactoring `src/admin/show.rs`

Существующие функции делают collection + serialization за один проход (под Postgres-протокол). Чтобы не дублировать логику для JSON, выносим collection в чистые функции:

```rust
// src/admin/show.rs (рефакторинг)
pub fn collect_pools() -> Vec<PoolRow> { ... }
pub fn collect_clients() -> Vec<ClientRow> { ... }
// ...

#[derive(Serialize, Clone)]
pub struct PoolRow { /* поля */ }
```

Старые `show_pools(stream)` теперь вызывают `collect_pools()` + `render_pg_rows(...)`. Новые `routes::pools::handle()` вызывают `collect_pools()` + `serde_json::to_vec(...)`. Pure functions тестируются изолированно.

### 7.6 `src/web/static_assets.rs`

```rust
pub fn resolve(path: &str) -> Option<(&'static str, &'static [u8])> {
    let normalized = if path == "/" { "index.html" } else { path.trim_start_matches('/') };
    if let Some(file) = SPA.get_file(normalized) {
        return Some((content_type_for(normalized), file.contents()));
    }
    // SPA fallback для client-side routing: путь без расширения → index.html
    if !normalized.contains('.') {
        return SPA.get_file("index.html").map(|f| ("text/html", f.contents()));
    }
    None
}
```

`include_dir` — compile-time, ~1 KSLoc. Bundle добавит ~150–250 KB к бинарнику (ожидается).

## 8. API endpoints

### 8.1 Общий принцип

- Плоский JSON, без wrapper-конверта.
- У объектных response'ов поле `ts: <unix_ms>` для синхронизации графиков.
- Pagination через `?limit=&offset=` (clients, servers, prepared).
- Все timestamps — unix milliseconds.
- HTTP-коды: `200 / 401 / 404 / 500 / 503`.
- Error body: `{"error": "<code>", "message": "<human>"}`.

### 8.2 Полный список

| Путь | Доступ | Назначение |
|---|---|---|
| `/metrics` | always public | Prometheus exporter, не трогаем |
| `/api/version` | public | `{version, build_date, git_commit, ts}` |
| `/api/overview` | public | Композит для главной (см. 8.3) |
| `/api/pools` | public | Per-pool строки |
| `/api/clients` | public | Active client connections (с pagination) |
| `/api/servers` | public | Backend connections (с pagination) |
| `/api/connections` | public | Cumulative counters (total/tls/plain/cancel/errors) |
| `/api/stats` | public | Per-pool xact/query/wait counters |
| `/api/databases` | public | Конфиг database entries |
| `/api/users` | public | Список пользователей |
| `/api/config` | public | Ключ-значение, **секреты маскированы** |
| `/api/auth_query` | public | Auth-query cache stats per pool |
| `/api/log_level` | public | Текущий filter (RUST_LOG-формат) |
| `/api/pool_scaling` | public | Anticipation/burst-gate counters per pool |
| `/api/pool_coordinator` | public | Coordinator limits/usage per database |
| `/api/sockets` | public, linux-only | TCP socket states |
| `/api/prepared` | public | Агрегат без текстов |
| `/api/interner` | public | Агрегат без preview |
| `/api/prepared/text/{hash}` | **admin** | Тело конкретного prepared statement |
| `/api/interner/top?n=N` | **admin** | Top-N интернированных запросов с 120-char preview |
| `/api/logs?since=&max=` | **admin** | Live-tail (см. секция 9) |

Маскирование секретов в `/api/config`: значение `"***"` для любого поля, чьё имя в TOML — точно `password`, `secret`, либо имеет суффикс `_password`/`_secret`/`_token`/`_key`. Покрывает `admin_password`, `talos_jwt_secret`, per-user `[user] password`, потенциальные `*_token` и `*_key`. Конкретный whitelist полей фиксируется в `routes::config` тестом, чтобы добавление нового секрета в config'е сразу провалило тест без апдейта маскера.

### 8.3 Shape `/api/overview`

```json
{
  "ts": 1714752000123,
  "active_clients": 1247, "idle_clients": 312,
  "active_servers": 78,   "idle_servers": 22,
  "connections_total": 18934, "connections_tls": 18012, "connections_plain": 922,
  "connections_cancel": 41, "connections_errors": 14,
  "query_count_total": 9871234,
  "transaction_count_total": 4123456,
  "prepared_hit_count": 88123,
  "prepared_miss_count": 412,
  "pool_size_sum": 100, "pool_current_sum": 78,
  "wait_queue_depth_sum": 0,
  "pools_total": 12, "pools_paused": 0
}
```

Все cumulative-счётчики передаются как есть; клиент сам считает дельты для tps/qps/hit_rate/conn_rate за окно.

### 8.4 Shape `/api/pools`

```json
{ "ts": ..., "pools": [
  { "id":"main@db1", "user":"app", "database":"db1", "host":"pg1", "port":5432,
    "pool_mode":"transaction", "pool_size":50, "min_pool_size":5,
    "current":42, "idle":8, "active":34, "waiting":0,
    "paused":false, "epoch":3 },
  ...
] }
```

### 8.5 Shape `/api/clients?limit=100&offset=0`

```json
{ "ts": ..., "total": 1247, "limit": 100, "offset": 0, "clients": [
  { "client_id":"#c12345", "database":"db1", "user":"app",
    "application_name":"myservice@v3", "addr":"10.1.2.3:54321",
    "tls":true, "state":"active", "wait":"none",
    "transaction_count":4123, "query_count":18421, "error_count":2,
    "age_seconds":1842 },
  ...
] }
```

### 8.6 Shape `/api/logs?since=<seq>&max=<200>`

```json
{ "ts": ...,
  "tap_active": true,
  "tap_capacity_bytes": 65536,
  "tap_used_bytes": 18024,
  "next_seq": 10421,
  "dropped_before": 0,
  "entries": [
    { "seq":10401, "ts_ms":1714752000098, "level":"INFO",
      "target":"pg_doorman::pool",
      "message":"server #s12 returned to pool main@db1 (idle)" },
    ...
  ]
}
```

При `web.log_tap_kb = 0` — handler возвращает 503 + `{"error":"log_tap_disabled","message":"log_tap_kb is 0 in config"}`.

## 9. LogTap lifecycle

### 9.1 State machine

```
Off ──first GET /api/logs──▶ Active ──no requests for 30s──▶ Off
                              │
                              └── push() из Log::log() пока Active
```

`Off` = `controller.tap = None` (нулевой оверхед в hot path).
`Active` = `controller.tap = Some(Arc<LogTap>)`.

### 9.2 Activation handler

```rust
async fn handle_logs(query: LogsQuery, auth: Authenticated) -> Response {
    if !auth.admin { return Response::status(401); }
    if config.web.log_tap_kb == 0 {
        return Response::json_status(503, Error { code: "log_tap_disabled", ... });
    }
    let tap = log_level::log_tap()
        .unwrap_or_else(|| log_level::enable_log_tap(config.web.log_tap_kb as usize * 1024));
    let DrainResult { entries, next_seq, dropped_before }
        = tap.drain_since(query.since, query.max.unwrap_or(200));
    Response::json(/* ... */)
}
```

### 9.3 Reaper

```rust
async fn reaper() {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Some(tap) = log_level::log_tap() {
            let last = tap.last_request_at.load(Relaxed);
            if now_monotonic_ms().saturating_sub(last) > 30_000 {
                log_level::disable_log_tap();
                log::debug!("LogTap disabled (no consumers for 30s)");
            }
        }
    }
}
```

Reaper-task запускается один раз в `web::server::start` при `ui_active = true && log_tap_kb > 0`.

### 9.4 Гонки и инварианты

1. **Activation race** (T1, T2 одновременно делают первый GET). `enable_log_tap(cap)` идемпотентна — внутри CAS на `ArcSwap` с условием `None`. Проигравший получает существующий Arc.
2. **Reap race** (reaper выключает в момент полла). Reaper делает store None, через ms T2 enable'ит новый. Дыра в истории ровно 0–5 секунд в худшем случае.
3. **Push-during-reap**. Push идёт в старый Arc; запись копится; когда последний `Arc::strong_count` дропается — весь ring освобождается через Drop.
4. **Seq monotonicity**. `fetch_add` гарантирует уникальность. Порядок в `VecDeque` может слегка отличаться от seq при контенции (T1 fetch_add'нул раньше, lock взял позже). UI sort'ит по seq при render'е.

### 9.5 Format и truncation

Каждая запись в ring — структура `LogEntry` (см. 7.3). Сериализация в JSON делается в handler'е, не при push'е. `message` обрезается до 4 KB при push с маркером `…<truncated>` если длиннее. `target` — путь модуля Rust (например, `"pg_doorman::pool::server_pool"`), обрезке не подвергается.

### 9.6 Filter (level/target)

На клиенте, не на сервере. Сервер всегда отдаёт полный stream; UI фильтрует через React state. Если ring заполняется debug-сообщениями быстрее, чем оператор успевает их читать — добавим server-side filter `?level=&target=` без breaking change.

## 10. Frontend

### 10.1 Стек (фиксируем)

| Компонент | Версия | Размер |
|---|---|---|
| React | 18.x | ~45 KB gzipped |
| TypeScript | 5.x | (compile only) |
| Vite | 5.x | (build only) |
| react-router | 6.x | ~10 KB |
| uPlot | 1.6.x | ~40 KB, ноль deps |
| Tailwind | 3.x | tree-shaken до ~10 KB |

Никаких UI-китов (MUI/Chakra/shadcn) в MVP. Никакого state-менеджера (Redux/Zustand) — `useState` + custom hooks.

### 10.2 Polling и история

`usePoll(fetcher, intervalMs)` — обёртка `useEffect`, очищает interval при unmount. Дефолтный interval — 1500 мс.

`useHistory<T>(key, maxPoints)` — rolling window 120 точек (≈ 3 минуты при 1.5s polling), персистит в `sessionStorage` под `key`. Кнопка «Reset history» в Overview очищает sessionStorage.

### 10.3 Auth flow в браузере

При `ui_anonymous = true`: фронт делает запросы без `Authorization`. Если 401 на admin-only endpoint — `AuthGate` показывает modal с input для credentials, сохраняет в memory (React state), повторяет запрос с `Authorization: Basic <b64>`.

При `ui_anonymous = false`: первый запрос (`GET /api/version`) возвращает 401 → `AuthGate` перехватывает, дальше всё под basic-auth.

Credentials хранятся **только в memory** (React state). При F5 нужно перевводить.

### 10.4 Build/CI

В `release`-pipeline шаг перед `cargo build`:

```yaml
- name: Build frontend
  run: |
    cd frontend
    npm ci
    npm run build
```

В dev: `vite dev` на :5173, proxy `/api/*` → :9127. Hot reload работает.

В CI checks: `npm run typecheck` + `npm run lint`.

## 11. Error handling

### 11.1 HTTP-коды

| Код | Когда |
|---|---|
| 200 | OK |
| 401 | Auth required (включая admin-only без creds, и любой путь при `ui_anonymous=false` без creds) |
| 404 | Путь не найден; `/api/*` при `ui_active=false` |
| 500 | Handler поймал `Result::Err` (логируется через `log::error!`) |
| 503 | `/api/logs` при `log_tap_kb=0` |

### 11.2 Backend handlers

Ловим `Result` и логируем через `log::error!` + возвращаем 500 с body `{"error":"internal","message":"<short>"}`.

`tokio::spawn` per-connection изолирует panic — паника в одном handler'е роняет одно соединение, listener продолжает accept'ить.

### 11.3 Frontend

- 401 → `AuthGate` показывает modal, повтор запроса.
- 5xx → inline-баннер «Backend error: <msg>», polling продолжается.
- Network error / connection refused → «Backend unreachable» баннер.
- Дроп логов → `LogStream` показывает строку `<N> lines dropped` серой плашкой.

## 12. Testing

### 12.1 Rust unit tests

- `src/web/auth.rs` — base64 decode, malformed headers, constant-time compare, default-password rejection.
- `src/web/log_tap.rs` — push с overflow и eviction, drain_since (старее ring → `dropped_before`, в ring → корректный slice, после tail → пусто), seq monotonicity при concurrent push, idempotent enable.
- `src/web/routes/*.rs` — каждый handler проверяется на JSON shape (snapshot-тесты на serde output). Чистые `collect_*()` функции тестируются отдельно от serializers.

### 12.2 Rust integration tests

Поднимаем настоящий HTTP listener в test, делаем `reqwest`:
- public path при `ui_anonymous=true` → 200 без auth.
- admin-only без creds → 401 + WWW-Authenticate.
- admin-only с правильными creds → 200.
- `ui=false` → `/metrics` 200, `/` 404.
- `ui=true && admin_password="admin"` → warn в логах, `/api/*` 404.

### 12.3 BDD (`tests/bdd/`)

- `web_anonymous.feature` — read-only сценарии без auth.
- `web_admin.feature` — auth flow, доступ к логам.
- `web_default_password.feature` — UI отключается при дефолтном пароле.
- `web_log_tap.feature` — lazy activation и reaper deactivation.

### 12.4 Frontend tests

- `vitest` для hooks (`usePoll`, `useHistory`).
- React Testing Library для `AuthGate` (рендер + 401 → modal).
- Smoke build: `npm run build` без warnings (CI gate).

## 13. Migration

### 13.1 Конфиг

Backwards-compatible через `#[serde(alias = "prometheus")]`. Старые `pg_doorman.toml` с `[prometheus] enabled = true / host / port` продолжат работать без изменений. Дефолты новых ключей (`ui = false`, `ui_anonymous = true`, `log_tap_kb = 64`) безопасны: ничего не появляется само.

Ограничение alias-подхода: нельзя одновременно держать в одном TOML и `[prometheus]`, и `[web]` — это будет ошибка парсинга. Документируем явно, в `pg_doorman.example.toml` показываем только новое имя.

### 13.2 Код

- `git mv src/prometheus src/web/metrics`
- `git mv src/web/metrics/server.rs src/web/server.rs`
- Глобальная замена `crate::prometheus::*` → `crate::web::metrics::*` (sanity: `cargo build && cargo clippy && cargo test`).
- Создаются: `src/web/{auth,log_tap,static_assets,routes/}.rs`.
- Обновляется: `src/app/log_level.rs` (поле `tap`, методы `enable/disable/log_tap`).

### 13.3 Документация

- `documentation/{en,ru}/src/configuration*.md` — обновить секцию `[web]`, добавить feature-флаги, упомянуть deprecated alias `[prometheus]`.
- `changelog.md` — entry под следующий минор.
- `README.md` — короткая ссылка на UI feature.

### 13.4 Версионирование

Следующий релиз — minor (3.8.0 либо ближайший по календарю), не major. Переименование секции через alias — не breaking change.

### 13.5 Релиз-чеклист

- `cargo fmt && cargo clippy -- --deny warnings && cargo test` — чисто
- BDD пройден
- Frontend `npm run build` без warnings
- Размер бинарника проверен (ожидание +200–400 KB)
- Hot path логов: бенчмарк на info-уровне до/после с `tap=None` (должен быть в шуме измерений)

## 14. Implementation phases

Будет уточнено в `writing-plans`. Высокоуровневое разбиение:

1. **Reorg + config** — переезд `prometheus → web/metrics`, новая `Web` структура с alias, defaults. Без изменения поведения наружу.
2. **Listener + mux + auth** — расширение `web/server.rs` до mux'а, `web/auth.rs`, отказ при дефолтном пароле. `/metrics` продолжает работать.
3. **Backend routes** — `routes/*.rs` плюс рефакторинг `admin/show.rs` (`collect_*` функции), все public endpoint'ы.
4. **LogTap + admin endpoint'ы** — `log_tap.rs`, `enable_log_tap` в `LogLevelController`, `routes/logs.rs`, reaper-task.
5. **Frontend skeleton** — `frontend/` с Vite, sidebar layout, AuthGate, react-router. Все страницы — заглушки.
6. **Frontend pages** — Overview (с uPlot), Pools, Clients, Servers, Prepared, Interner, Logs, Config.
7. **Embedding + CI + docs** — `include_dir!`, build-pipeline шаг, документация, BDD-сценарии.

## 15. Open follow-ups (не входит в MVP, явно отложено)

- Per-pool breakdown на графиках Overview (легенда, цвета, persistance выбора).
- Server-side filter для `/api/logs?level=&target=` если ring заполняется быстрее, чем оператор читает.
- SSE / WebSocket для логов и графиков, если polling окажется слабым местом.
- Темизация (light/dark, system-preference).
- i18n.
- Мобильная адаптация sidebar nav.
- Admin-команды через UI: PAUSE / RESUME / RECONNECT / RELOAD / SET log_level — после security-обзора (CSRF, audit log, идемпотентность кнопок).
- Серверное хранение коротких rolling-окон (например, 24 часа в SQLite/parquet) для bookmarkable URL с конкретным временным окном.
