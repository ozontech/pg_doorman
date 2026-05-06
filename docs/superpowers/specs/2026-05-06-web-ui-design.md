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
| 14 | LogTap concurrency | Lock-free producer (AtomicBool gate + `try_send` в bounded MPSC) + single consumer task. Точность лога жертвуется (drop-new при burst) ради zero contention в hot path SQL. |
| 15 | Page count | 6 страниц вместо 8 (UX-ревью). Pools поглощает Servers через drawer drill-down. Caches объединяет Prepared+Interner вкладками. ConfigState консолидирует config/auth_query/log_level/databases/users/sockets/pool_scaling/pool_coordinator. |
| 16 | Top-N как core feature | Endpoints `/api/top/{queries,clients,prepared}` и `/api/apps` — не nice-to-have, а killer feature для триажа («service is slow»). Без них UI = `SHOW POOLS` viewer (DBA-ревью). |
| 17 | Sort/filter/url-state | Все таблицы >50 строк — sortable headers + server-side filter через query params + URL-state (`?sort=&order=&pool=&user=…`). Bookmarkable URLs обязательны для pair-debug. |
| 18 | Freshness + health pill | В top bar постоянно видимые: health-pill (OK/degraded/critical, считается из threshold-таблицы) и freshness indicator (Updated Xs ago). Без них polling-UI молча врёт оператору при зависании. |
| 19 | log_tap_max_entries default | Default bump 1024 → 8192 (DBA-ревью). На info-уровне при 5kTPS 1024 строк = субсекунда истории; 8192 даёт ~1.5–2 минуты, сохраняя ≤2 MB RSS. |
| 20 | Thresholds — formula-based | Все warning/critical пороги в раздел 15 — формулы, не абсолютные числа. `0.7×pool_size`, не `35`. Это нужно, чтобы пороги работали и для pool_size=10, и для pool_size=500. |
| 21 | Thin server, fat client | Backend отдаёт ТОЛЬКО raw counters / gauges / percentiles. Никаких computed `health.state`, `errors_per_sec`, `worst_pool`, aggregations. Threshold engine — чистая функция во frontend (`src/lib/thresholds.ts`), применяется к raw shape. Перенос: (1) backend не дублирует UI бизнес-логику; (2) поменять порог = PR во frontend без backend rebuild и rollout; (3) `cargo build` и frontend deployment слабо связаны. |
| 22 | Frontend dist коммитится | `frontend/dist/` коммитится в git; release-pipeline не зависит от npm. `cargo build` берёт уже built bundle через `include_dir!`. Trade-off: ~150-250 KB built артефактов в репо ради простоты RPM/DEB/Docker сборки без npm-toolchain. Lint/typecheck остаются обязательны на pre-commit (см. 10.4). |

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
│   ├── pages/  (Overview, Pools, Clients, Caches, Logs, ConfigState)
│   ├── components/  (Sidebar, Chart, Sparkline, Heatmap, Table, LogStream,
│   │                AuthGate, FreshnessIndicator, HealthPill, Drawer,
│   │                TimePicker, EmptyState, Banner, Badge, Button)
│   ├── hooks/  (usePoll, useHistory, useAdminAuth, useUrlState,
│   │           useThresholdPaint, useKeyboard)
│   └── styles/tailwind.css
└── public/favicon.ico
```

**Set страниц сокращён с 8 до 6** (по итогу UX-ревью):

| Старая страница | Новое расположение |
|---|---|
| Overview | Overview (без изменений в назначении, новый layout — см. раздел 10) |
| Pools | Pools (с server detail в drawer/sub-route, Servers как отдельная страница убрана) |
| Servers | Pool detail drawer + по-server-у вкладка внутри Pools |
| Clients | Clients (с filter/sort/url-state) |
| Prepared | Caches → tab «Prepared» |
| Interner | Caches → tab «Query Cache» (UI label, endpoint `/api/interner` остаётся) |
| Logs | Logs |
| Config | ConfigState (содержит config + auth_query + log_level + databases + users + sockets + pool_scaling + pool_coordinator подразделами) |

Стек: React 18 + TS 5 + Vite 5 + react-router 6 + uPlot 1.6 + Tailwind v3.

### 4.4 Embedding в бинарь

`src/web/static_assets.rs` использует `include_dir!` macro:

```rust
use include_dir::{include_dir, Dir};
static SPA: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/frontend/dist");
```

**Built артефакты коммитятся в git** как `frontend/dist/`. Release-pipeline (RPM/DEB/Docker) НЕ запускает `npm run build` — `cargo build` напрямую embed'ит уже закоммитнутый bundle. Это сознательный trade-off: добавлять npm-toolchain в release machinery дороже, чем коммитить ~150-250 KB built файлов.

В dev — `vite dev` на :5173 с proxy на :9127. Разработчик отвечает за `npm run build` перед коммитом изменений во `frontend/src/`. См. раздел 10.4 про CI и pre-commit verification.

## 5. Конфиг

### 5.1 TOML

```toml
[web]
enabled = true                 # поднимать listener (бывший [prometheus] enabled)
host = "0.0.0.0"
port = 9127
ui = false                     # NEW: дать UI и /api/*
ui_anonymous = true            # NEW: public-пути без auth
log_tap_max_entries = 8192     # NEW: capacity ring buffer'а логов (в строках, не байтах)
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
    #[serde(default = "Web::default_log_tap_max_entries")]
    pub log_tap_max_entries: u32,
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
| `log_tap_max_entries` | `8192` | На info-уровне при 5kTPS — ~1.5-2 мин истории; на debug — ~100 сек; ≤2 MB RSS при средней записи 250 байт. Меньше 8192 (например 1024) даёт sub-секунду истории на горячем pooler'е и делает live-tail бесполезным на инциденте; больше 32768 — RSS неоправданный для дефолта (operator может бампнуть руками). |

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

Архитектура — **lock-free producer + single consumer task**. Producer (Log::log) пишет через bounded MPSC, не берёт никаких локов. Consumer task — единственный owner ring buffer'а, делает eviction и отвечает на drain'ы. Это устраняет contention в hot path SQL: даже под debug-уровнем при 500 worker'ах × 100 logs/sec hot path остаётся lock-free.

```rust
pub struct LogEntry {
    pub seq: u64,           // присваивает consumer (он один writer)
    pub ts_ms: u64,
    pub level: log::Level,
    pub target: String,
    pub message: String,    // pre-truncated до 4 KB через bounded fmt::Write
}

/// Public-side handle; producer и handler владеют клонами.
pub struct LogTap {
    /// Hot-path gate: когда false, producer возвращается за ~1 ns не делая send.
    pub tap_active: Arc<AtomicBool>,
    /// Producer side bounded MPSC. На full — `try_send` отдаёт SendError, считаем drop.
    pub tx: tokio::sync::mpsc::Sender<RawEntry>,
    /// Counter дропов: и channel-full, и evict'нутые consumer'ом из VecDeque.
    pub dropped_total: Arc<AtomicU64>,
    /// Команды consumer'у: drain или shutdown.
    pub cmd_tx: tokio::sync::mpsc::Sender<TapCommand>,
    /// Monotonic ms последнего запроса GET /api/logs — для reaper.
    pub last_request_at: Arc<AtomicU64>,
}

/// Что producer кладёт в канал. seq и ts_ms заполняет consumer/producer соответственно.
pub struct RawEntry {
    pub ts_ms: u64,
    pub level: log::Level,
    pub target: &'static str,    // log::Record::target() это &'static str
    pub message: String,         // bounded 4 KB
}

pub enum TapCommand {
    Drain { since: u64, max: usize, reply: tokio::sync::oneshot::Sender<DrainResult> },
    Shutdown,
}

pub struct DrainResult {
    pub entries: Vec<LogEntry>,
    pub next_seq: u64,
    pub dropped_before: u64,
    pub used_entries: usize,
}
```

Внутри consumer-task'а живёт private state (никто кроме task'а его не трогает):

```rust
struct ConsumerState {
    entries: VecDeque<LogEntry>,
    next_seq: u64,
    max_entries: usize,
    dropped_evicted: u64,    // выгнанные при overflow VecDeque
}
```

**Per-entry лимит:** 4 KB на сообщение, реализуется через **bounded `fmt::Write`** ещё в producer'е, до push'а в канал. Это спасает от ОЗУ-всплеска при мегабайтном args (debug-лог prepared statement может содержать SQL на сотни KB):

```rust
struct BoundedWriter {
    buf: String,
    cap: usize,
    overflow: bool,
}

impl fmt::Write for BoundedWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let remaining = self.cap.saturating_sub(self.buf.len());
        if remaining == 0 {
            self.overflow = true;
            return Ok(());
        }
        if s.len() <= remaining {
            self.buf.push_str(s);
        } else {
            // безопасный truncate по UTF-8 границе
            let mut end = remaining;
            while !s.is_char_boundary(end) { end -= 1; }
            self.buf.push_str(&s[..end]);
            self.overflow = true;
        }
        Ok(())
    }
}
```

Если `overflow == true` — при выдаче в канал к message добавляется маркер `"…<truncated>"`.

**Channel capacity:** `tokio::sync::mpsc::channel(channel_cap)`, где `channel_cap = max_entries`. Это даёт producer'у запас на burst (≈ 12 секунд debug-стрима при 80 строк/сек), но не безграничный — на пере-прыжке consumer'а producer молча дропает (`try_send` -> `Err(TrySendError::Full)`).

**Consumer task** (single tokio task, spawn-ится при `enable_log_tap`):

```rust
async fn run_consumer(
    mut rx: mpsc::Receiver<RawEntry>,
    mut cmd_rx: mpsc::Receiver<TapCommand>,
    state: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    max_entries: usize,
) {
    let mut s = ConsumerState { entries: VecDeque::with_capacity(max_entries), next_seq: 0, max_entries, dropped_evicted: 0 };
    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(TapCommand::Drain { since, max, reply }) => {
                        let entries: Vec<LogEntry> = s.entries.iter()
                            .skip_while(|e| e.seq < since)
                            .take(max)
                            .cloned()
                            .collect();
                        let dropped_before = if since < s.entries.front().map(|e| e.seq).unwrap_or(s.next_seq) {
                            s.entries.front().map(|e| e.seq).unwrap_or(s.next_seq).saturating_sub(since)
                        } else { 0 };
                        let _ = reply.send(DrainResult { entries, next_seq: s.next_seq, dropped_before, used_entries: s.entries.len() });
                    }
                    Some(TapCommand::Shutdown) | None => break,
                }
            }
            raw = rx.recv() => {
                let Some(raw) = raw else { break; };
                let entry = LogEntry { seq: s.next_seq, ts_ms: raw.ts_ms, level: raw.level, target: raw.target.to_string(), message: raw.message };
                s.next_seq += 1;
                s.entries.push_back(entry);
                while s.entries.len() > s.max_entries {
                    s.entries.pop_front();
                    s.dropped_evicted += 1;
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    state.store(false, Ordering::Release);
}
```

Drain делает clone подмножества под одним и тем же task'ом — нет cross-thread synchronisation на этой операции вообще. Стоимость drain — O(N), N ≤ 1024, единицы µs.

### 7.4 Доработка `src/app/log_level.rs`

Hot path логгера спроектирован так, чтобы в неактивном состоянии стоить ровно один Relaxed-load (`AtomicBool`), а в активном не брать локов вообще:

```rust
pub struct LogLevelController {
    inner: Box<dyn Log>,
    /// Hot-path gate. False — никакого capture не делаем.
    tap_active: AtomicBool,
    /// Доступ к channel'у активного tap'а; обновляется только под mutex'ом ниже,
    /// читается только когда tap_active=true.
    tap: ArcSwap<Option<Arc<LogTap>>>,
    /// Гарантирует ровно одну активацию/деактивацию за раз; вне hot path.
    lifecycle: parking_lot::Mutex<()>,
}

impl Log for LogLevelController {
    fn log(&self, record: &Record) {
        if !self.inner.enabled(record.metadata()) { return; }
        self.inner.log(record);

        // Hot path при tap_active=false — ровно один Relaxed load (~1 ns).
        if !self.tap_active.load(Ordering::Relaxed) { return; }

        // Format в bounded buffer (4 KB cap). Защищает от мегабайтных args в debug.
        let mut buf = BoundedWriter::new(4096);
        let _ = write!(buf, "{}", record.args());
        let message = if buf.overflow {
            let mut m = buf.into_string();
            m.push_str("…<truncated>");
            m
        } else {
            buf.into_string()
        };

        let raw = RawEntry {
            ts_ms: now_unix_ms(),
            level: record.level(),
            target: record.target(),     // &'static str из log macro
            message,
        };

        // Snapshot текущего tap'а через ArcSwap (lock-free read).
        let guard = self.tap.load();
        let Some(tap) = guard.as_ref() else { return; };

        // Lock-free try_send: если канал full — молча инкрементим dropped и идём дальше.
        // Никогда не блокируем hot path SQL.
        if tap.tx.try_send(raw).is_err() {
            tap.dropped_total.fetch_add(1, Ordering::Relaxed);
        }
    }
}
```

API контроллера:
- `enable_log_tap(max_entries) -> Arc<LogTap>` (idempotent под mutex'ом lifecycle; первый вызов создаёт LogTap + spawn'ит consumer task'и + `tap_active.store(true)`).
- `disable_log_tap()` (под mutex'ом lifecycle: `tap_active.store(false)` → producer'ы перестают писать → drop sender → consumer выходит → старый Arc dropped).
- `log_tap() -> Option<Arc<LogTap>>` — для handler'а `/api/logs`.

**Стоимость hot path:**
- `tap_active = false` (полная цепочка production'а с info-уровнем): один Relaxed load ≈ 1 ns. Дешевле, чем ArcSwap-вариант на ~5 ns в неактивном состоянии — что важно при миллионах log!() в высоконагруженной нагрузке.
- `tap_active = true`: format в bounded buffer (~200-500 ns для типичной записи 100-200 символов), один atomic load на ArcSwap (~5 ns), `try_send` в `tokio::sync::mpsc` (lock-free на uncontended ~30-50 ns; на full — `Err` и инкремент counter'а ~10 ns). **Никакого Mutex'а в hot path нет вообще.**

**Backpressure / drop policy:**
- Стационарный режим: producer пишет успешно → consumer обрабатывает → когда VecDeque переполнен (`len > max_entries`), consumer выкидывает front (drop-old). Оператор видит свежие записи.
- Burst-режим: producer'ы пишут быстрее, чем consumer успевает читать → channel заполняется → `try_send` отдаёт `Full` → producer молча инкрементит `dropped_total` (drop-new) и идёт дальше. UI показывает `dropped_before` в `/api/logs` response, оператор видит счётчик «N lines dropped» серой плашкой.

То есть точность лога жертвуется в обмен на zero contention в hot path: **под нагрузкой логи могут быть неполными, но SQL-обработка не тормозит**. Это сознательный выбор для high-throughput pooler'а.

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

**Core endpoints** (питают Overview, Pools, Clients, Caches, Logs):

| Путь | Доступ | Назначение |
|---|---|---|
| `/metrics` | always public | Prometheus exporter, не трогаем |
| `/api/version` | public | `{version, build_date, git_commit, ts}` |
| `/api/overview` | public | Композит для главной (см. 8.3) |
| `/api/pools` | public | Per-pool строки (см. 8.4) |
| `/api/clients` | public | Active client connections (см. 8.5) |
| `/api/servers` | public | Backend connections, питает Pool detail drawer |
| `/api/prepared` | public | Агрегат prepared statements без текстов |
| `/api/interner` | public | Агрегат query interner без preview |
| `/api/prepared/text/{hash}` | **admin** | Тело конкретного prepared statement |
| `/api/interner/top?n=N` | **admin** | Top-N интернированных запросов с 120-char preview |
| `/api/logs?since=&max=&level=&target=` | **admin** | Live-tail (см. секция 9) |

**Top-N endpoints** (DBA-ревью: главный killer feature для триажа):

| Путь | Доступ | Назначение |
|---|---|---|
| `/api/top/queries?by=count\|duration&n=20&pool=` | public | Top-N query texts по количеству / средней длительности |
| `/api/top/clients?by=qps\|errors\|age&n=20&pool=` | public | Top-N клиентов по QPS / errors / connection age |
| `/api/top/prepared?by=hits\|misses&n=20` | public | Top-N prepared statements по hits/misses |
| `/api/apps?sort=&order=` | public | Aggregation по `application_name`: clients, qps, tps, errors per app |

**Annotations / events** (для отметок на графиках):

| Путь | Доступ | Назначение |
|---|---|---|
| `/api/events?since=` | public | Subset логов с `level=INFO` и `target` ∈ {RELOAD, RECONNECT, PAUSE, RESUME}, плюс наблюдённые `epoch`-discontinuities. Используется для аннотаций vertical-line на графиках Overview. |

**State endpoints** (питают страницу ConfigState — каждый под своей вкладкой):

| Путь | Доступ | Назначение |
|---|---|---|
| `/api/config` | public | Ключ-значение, **секреты маскированы** |
| `/api/connections` | public | Cumulative counters (total/tls/plain/cancel/errors) |
| `/api/stats` | public | Per-pool xact/query/wait counters |
| `/api/databases` | public | Конфиг database entries |
| `/api/users` | public | Список пользователей |
| `/api/auth_query` | public | Auth-query cache stats per pool |
| `/api/log_level` | public | Текущий filter (RUST_LOG-формат) |
| `/api/pool_scaling` | public | Anticipation/burst-gate counters per pool |
| `/api/pool_coordinator` | public | Coordinator limits/usage per database |
| `/api/sockets` | public, linux-only | TCP socket states |

**Общие query parameters для list endpoints (`/api/clients`, `/api/pools`, `/api/servers`, `/api/prepared`, `/api/apps`, `/api/top/*`):**
- `?sort=<column>&order=asc|desc` — сортировка (default fixed in handler).
- `?limit=&offset=` — pagination.
- Filter parameters: для `/api/clients` — `?pool=&database=&user=&application_name=&state=`; для `/api/pools` — `?paused=true|false`; для `/api/servers` — `?pool=`. Конкретные допустимые значения и их семантика — в snapshot-тестах handler'а.

URL view-state в SPA encodes эти параметры как query string, чтобы операторы могли копировать URL коллегам (Sharable URLs — decision #17, scenario H в DBA-ревью).

Маскирование секретов в `/api/config`: значение `"***"` для любого поля, чьё имя в TOML — точно `password`, `secret`, либо имеет суффикс `_password`/`_secret`/`_token`/`_key`. Покрывает `admin_password`, `talos_jwt_secret`, per-user `[user] password`, потенциальные `*_token` и `*_key`. Конкретный whitelist полей фиксируется в `routes::config` тестом, чтобы добавление нового секрета в config'е сразу провалило тест без апдейта маскера.

### 8.3 Shape `/api/overview`

```json
{
  "ts": 1714752000123,

  "active_clients": 1247, "idle_clients": 312, "waiting_clients": 0,
  "active_servers": 78,   "idle_servers": 22,

  "connections_total": 18934, "connections_tls_total": 18012, "connections_plain_total": 922,
  "connections_cancel_total": 41,

  "query_count_total": 9871234,
  "transaction_count_total": 4123456,
  "errors_count_total": 14,

  "prepared_hits_total": 88123,
  "prepared_misses_total": 412,

  "pools_total": 12, "pools_paused": 0
}
```

**Принципы (decision #21 — thin server, fat client):**

- Backend отдаёт **только** raw counters / gauges. Никаких computed `health`, `worst_pool`, `errors_per_sec`, max-роллапов.
- Все производные метрики (rates, percents, deltas, max-across-pools, health.state) — frontend через `useHistory` + чистую функцию в `src/lib/thresholds.ts`.
- Top-bar HealthPill питается через `useThreshold(latestOverview, latestPoolsRows)` — собирает per-pool worst severity по таблице 15.4 и возвращает `{ state, reason }`.
- Cumulative counters суффикс `_total` (Prometheus convention) и **раздавим унификацию**: `connections_tls_total`, `connections_plain_total`, `connections_cancel_total`, `errors_count_total`, `prepared_hits_total`, `prepared_misses_total`.
- Убраны `pool_size_sum` / `pool_current_sum` — vanity, не используются операторами; per-pool данные на странице Pools.
- `pools_total` и `pools_paused` остаются как cheap counters (это `i32`, не computation), их экспоновать дешевле чем гонять клиента считать паузы из `/api/pools`.

Полная семантика field naming фиксируется snapshot-тестом handler'а (раздел 13.1).

### 8.4 Shape `/api/pools?sort=&order=&paused=`

```json
{ "ts": ..., "pools": [
  { "id":"main@db1", "user":"app", "database":"db1", "host":"pg1", "port":5432,
    "pool_mode":"transaction",
    "max_connections":50, "min_connections":5,
    "connections":42, "idle":8, "active":34, "waiting":0,
    "max_active_age_ms":142,
    "query_p95_ms":84, "query_p99_ms":210,
    "transactions_p95_ms":162, "transactions_p99_ms":480,
    "wait_avg_ms":0, "wait_p95_ms":0,
    "queries_total":871234, "transactions_total":423456, "errors_total":14,
    "paused":false, "epoch":3 },
  ...
] }
```

**Принципы:**
- Только raw counters / gauges / percentiles. `saturation_pct` (= 100×connections/max_connections) — это производная, считается на frontend; то же касается `errors_per_sec` (delta of `errors_total` over window).
- `max_active_age_ms`, `query_p95_ms`, `wait_p95_ms` — это **gauge'и** уже посчитанных HDR percentiles, не computation; их выгодно экспонировать (одна точка вместо raw histogram'а).
- Cumulative counters суффикс `_total`: `queries_total`, `transactions_total`, `errors_total`.

**Изменения от первой версии:**
- Поле `current` → `connections`, `pool_size` → `max_connections`, `min_pool_size` → `min_connections`.
- Убран `saturation_pct` — вычисляется на frontend в `usePoolDerived(pool)`.
- Убран `errors_per_sec` — derived rate, frontend считает delta из cumulative.

Default sort: `sort=connections&order=desc`. Server возвращает в указанном порядке без знания про severity. Frontend клиентским sort выносит crit-пулы наверх через `useSeveritySort()` (decision #21). Для отдельных user-flow ответ можно фильтровать через query params (`?paused=true`).

### 8.5 Shape `/api/clients?limit=&offset=&sort=&order=&pool=&database=&user=&application_name=&state=`

```json
{ "ts": ..., "total": 1247, "limit": 100, "offset": 0, "clients": [
  { "client_id":"#c12345", "database":"db1", "user":"app",
    "application_name":"myservice@v3", "addr":"10.1.2.3:54321",
    "tls":true, "state":"active", "wait":"none", "wait_ms":0,
    "transactions_total":4123, "queries_total":18421, "errors_total":2,
    "age_seconds":1842, "current_query_age_ms":42 },
  ...
] }
```

**Изменения:**
- Поля cumulative counters унифицированы под суффикс `_total`: `transactions_total`, `queries_total`, `errors_total`.
- Добавлено `wait_ms` — сколько именно конкретный клиент уже ждёт (для подсветки long-wait).
- Добавлено `current_query_age_ms` для clients в state=active — возраст текущей выполняющейся query (для long-running paint).

**Filter parameters:**
- `?pool=<id>` — `<user>@<database>`-id пула. Например `?pool=main@db1`.
- `?database=`, `?user=`, `?application_name=` — exact match (multi-select через repeated keys: `?application_name=app1&application_name=app2`).
- `?state=active|idle|waiting|disconnect` — фильтр по client state.

**Default sort:** `sort=queries_total&order=desc` (top consumers сверху). Operator может переключить на `errors_total`, `age_seconds`, `current_query_age_ms`.

### 8.6 Shape `/api/logs?since=<seq>&max=<200>`

```json
{ "ts": ...,
  "tap_active": true,
  "tap_capacity_entries": 1024,
  "tap_used_entries": 312,
  "next_seq": 10421,
  "dropped_before": 0,
  "dropped_total": 47,
  "entries": [
    { "seq":10401, "ts_ms":1714752000098, "level":"INFO",
      "target":"pg_doorman::pool",
      "message":"server #s12 returned to pool main@db1 (idle)" },
    ...
  ]
}
```

`dropped_before` — сколько записей пропало до `since` (consumer выкинул из VecDeque, оператор листал слишком медленно). `dropped_total` — общее число дропов с момента активации tap'а, включая burst-дропы (producer не смог положить из-за full channel) и evict-дропы. Разница `dropped_total - dropped_before` показывает burst-дропы внутри текущего окна.

При `web.log_tap_max_entries = 0` — handler возвращает 503 + `{"error":"log_tap_disabled","message":"log_tap_max_entries is 0 in config"}`.

## 9. LogTap lifecycle

### 9.1 State machine

```
Off ──first GET /api/logs──▶ Active ──no requests for 30s──▶ Off
                              │
                              └── try_send() из Log::log() пока Active
```

`Off` = `tap_active.load() == false` (один Relaxed load — ~1 ns в hot path).
`Active` = `tap_active = true`, channel + consumer task живы, ArcSwap держит `Some(Arc<LogTap>)`.

### 9.2 Activation handler

```rust
async fn handle_logs(query: LogsQuery, auth: Authenticated) -> Response {
    if !auth.admin { return Response::status(401); }
    if config.web.log_tap_max_entries == 0 {
        return Response::json_status(503, Error { code: "log_tap_disabled", ... });
    }

    let tap = log_level::log_tap()
        .unwrap_or_else(|| log_level::enable_log_tap(config.web.log_tap_max_entries as usize));

    tap.last_request_at.store(now_monotonic_ms(), Ordering::Relaxed);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let send_ok = tap.cmd_tx.send(TapCommand::Drain {
        since: query.since,
        max: query.max.unwrap_or(200),
        reply: reply_tx,
    }).await.is_ok();
    if !send_ok {
        // Consumer уже завершился (race с reaper'ом). Считаем как пустой результат.
        return Response::json(empty_drain_response());
    }

    let DrainResult { entries, next_seq, dropped_before, used_entries }
        = reply_rx.await.unwrap_or_else(|_| empty_drain());

    Response::json(LogsResponse {
        ts: now_unix_ms(),
        tap_active: true,
        tap_capacity_entries: config.web.log_tap_max_entries,
        tap_used_entries: used_entries,
        next_seq,
        dropped_before,
        dropped_total: tap.dropped_total.load(Ordering::Relaxed),
        entries,
    })
}
```

### 9.3 Reaper

```rust
async fn reaper() {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Some(tap) = log_level::log_tap() {
            let last = tap.last_request_at.load(Ordering::Relaxed);
            if now_monotonic_ms().saturating_sub(last) > 30_000 {
                log_level::disable_log_tap();
                log::debug!("LogTap disabled (no consumers for 30s)");
            }
        }
    }
}
```

Reaper-task запускается один раз в `web::server::start` при `ui_active = true && log_tap_max_entries > 0`.

### 9.4 Гонки и инварианты

Архитектура single-consumer + lock-free producer существенно проще классических ring-buffer гонок:

1. **Activation race** (T1, T2 одновременно делают первый GET). `enable_log_tap` берёт `lifecycle` mutex — это **вне hot path**, contention здесь нет вообще. Под mutex'ом проверяет `tap_active.load()`; если `true` — возвращает существующий Arc. Если `false` — создаёт channel + spawn'ит consumer + `tap_active.store(true, Release)` + `tap.store(Some(arc))` в ArcSwap. Mutex отпускается. Второй пришедший видит `true` и возвращает существующий Arc.
2. **Reap race** (reaper выключает в момент следующего polling'а от UI). Reaper берёт `lifecycle` mutex, выставляет `tap_active = false`, дропает sender (старый channel), tap.store(None). Consumer получает `None` из rx.recv(), завершается. UI делает запрос: `log_tap()` отдаёт None → `enable_log_tap` создаёт новый channel + новый consumer task. Дыра в **записи новых событий** — 0-5 секунд (interval reaper'а) до момента следующего GET. Память старого ring'а освобождается за миллисекунды (drop последнего Arc).
3. **Push-during-disable** (Log::log читает `tap_active=true`, потом reaper выключает). Producer уже взял ArcSwap снапшот старого `tx`. `try_send` либо успеет (запись попадёт в old ring, прочитается consumer'ом до его завершения), либо получит `Closed` (consumer уже завершён, sender дропнут) — обрабатывается как drop. Никаких корраптов или зависаний.
4. **Seq monotonicity**. `seq` присваивает consumer задачей (одно writer-местно — внутри `run_consumer`). Порядок строго совпадает с очерёдностью recv() из MPSC. **Sort на стороне UI больше не нужен**, что упрощает frontend.
5. **drain под нагрузкой**. `Drain` команда обрабатывается consumer'ом в том же `tokio::select`, что и `recv` из producer-channel. Drain — это `entries.iter().cloned().collect()`, для 1024 entries × 250 B — десятки µs, без contention с producer'ами (producer кладёт в channel, не в VecDeque; channel — non-blocking try_send). На время drain'а producer'ы могут заполнить channel, но это означает burst — отрабатывает drop-new policy.

### 9.5 Format и truncation

Format в `LogEntry::message` происходит **в producer'е**, через bounded `fmt::Write` (см. 7.3) с cap 4 KB. Это критично: если бы format происходил в consumer'е, при мегабайтном `record.args()` (debug deparse prepared statement) producer пересылал бы через канал гигантский String — лишняя аллокация и copy. Bounded format в producer'е останавливает запись на UTF-8 границе и добавляет `…<truncated>` ровно один раз.

`target` — `&'static str` из log macro (имя Rust-модуля типа `"pg_doorman::pool::server_pool"`). Передаётся через канал как ссылка, в LogEntry конвертируется в `String` consumer'ом. Не обрезается.

JSON сериализация — в handler'е `/api/logs`, не в consumer'е. Consumer владеет VecDeque, handler вызывает Drain command и получает Vec entries для serialise.

### 9.6 Filter (level/target)

**Server-side filter — в MVP, не follow-up** (DBA-ревью: ring 8192 entries на info-уровне @5kTPS даёт ~100 секунд debug — оператор пролистывает быстрее, чем backend заполняет, без фильтра live-tail невозможно использовать на debug-уровне).

`/api/logs` принимает:
- `?level=ERROR|WARN|INFO|DEBUG|TRACE` — минимальный отображаемый уровень (`level=WARN` отдаёт WARN+ERROR). Default — `INFO`.
- `?target=<substring>` — substring match по `target` (`pg_doorman::pool` отдаст `pg_doorman::pool::server_pool` и подобные).

Consumer применяет фильтр в `TapCommand::Drain` обработчике на сборке entries — это дешевле, чем гонять ненужные сообщения через сериализацию JSON и сеть. Семантика `dropped_before` сохраняется относительно нефильтрованного ring'а: оператор всегда видит, сколько событий полностью выпало из памяти (а не сколько отфильтровано на этом запросе).

UI default mode: `level=WARN` (errors+warnings only, фокус на проблемном). One-click toggle на `level=DEBUG` для ныряния глубже. Это снимает 80% шума при первом открытии страницы.

Multi-line message — explicit decision: `record.args()` сохраняется как есть (с embedded `\n`), один вызов `log!()` = один `LogEntry`. Front-end рендерит `\n` как visual line break внутри одной строки логов; в substring-фильтре поиск идёт по полному содержимому (с переводами строк).

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

**Release-pipeline не зависит от npm.** `frontend/dist/` коммитится в git как built артефакт; `cargo build` напрямую embed'ит готовый bundle через `include_dir!`. RPM/DEB/Docker сборка не нуждается в node toolchain. См. decision #22.

**Frontend CI job** (отдельный от Rust release):

```yaml
- name: Frontend lint and typecheck
  run: |
    cd frontend
    npm ci
    npm run lint        # eslint, обязательно — блокирует merge при errors
    npm run typecheck   # tsc --noEmit, обязательно
    npm run build       # rebuild и сравнить с frontend/dist (см. ниже)
- name: Verify dist is in sync with sources
  run: |
    git diff --exit-code frontend/dist
    # exit 1 если built bundle отличается от закоммитнутого — разработчик
    # забыл прогнать `npm run build` перед коммитом
```

`npm run build` в CI — sanity check, не источник release-артефакта. Если diff найден → CI fail с инструкцией: «run `npm run build` locally and commit the result».

**Dev loop:** `vite dev` на :5173 с proxy `/api/* → :9127`. Hot reload работает. Разработчик ответственен за `npm run build` перед коммитом изменений во `frontend/src/` — pre-commit hook (опционально) автоматизирует это.

**Pre-commit hook** (`.git/hooks/pre-commit` или husky):

```bash
#!/bin/sh
if git diff --cached --name-only | grep -q '^frontend/src/'; then
  echo "Frontend sources changed — rebuilding dist..."
  cd frontend && npm run build && git add dist
fi
```

Это nice-to-have, не строгое требование MVP — некоторые разработчики предпочитают ручной контроль над dist.

## 11. Error handling

### 11.1 HTTP-коды

| Код | Когда |
|---|---|
| 200 | OK |
| 401 | Auth required (включая admin-only без creds, и любой путь при `ui_anonymous=false` без creds) |
| 404 | Путь не найден; `/api/*` при `ui_active=false` |
| 500 | Handler поймал `Result::Err` (логируется через `log::error!`) |
| 503 | `/api/logs` при `log_tap_max_entries=0` |

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
- `src/web/log_tap.rs`:
  - `BoundedWriter`: stop на UTF-8 границе при overflow, маркер `…<truncated>` добавляется ровно один раз, не обрезает середину multibyte-символа.
  - Consumer task: drain поверх известного state'а (since в прошлом → корректный `dropped_before`; since в окне → корректный slice; since в будущем → пусто и `next_seq` соответствует), eviction при переполнении VecDeque (drop-old, dropped_total инкрементится).
  - try_send drop: при заполненном channel'е producer возвращается за единицы наносекунд, dropped_total инкрементится, hot path не зависает.
  - Idempotent `enable_log_tap`: второй вызов под mutex'ом возвращает существующий Arc.
  - `tap_active` race: producer, который только что прошёл `tap_active.load()=true`, и `disable` вне hot path — ни один путь не панически валится (моделируется через test с `loom` или ручным sequence'ом).
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
- `npm run lint` (eslint) и `npm run typecheck` (tsc --noEmit) — CI-gated, блокируют merge при errors.
- Sanity rebuild в CI: `npm run build` затем `git diff --exit-code frontend/dist` — fail если разработчик не закоммитил обновлённый bundle. Сама сборка не источник release artifact (см. 10.4).

## 13. Migration

### 13.1 Конфиг

Backwards-compatible через `#[serde(alias = "prometheus")]`. Старые `pg_doorman.toml` с `[prometheus] enabled = true / host / port` продолжат работать без изменений. Дефолты новых ключей (`ui = false`, `ui_anonymous = true`, `log_tap_max_entries = 8192`) безопасны: ничего не появляется само.

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
- Hot path логов, info-уровень, `tap_active=false`: бенчмарк до/после, регрессия должна быть в шуме измерений (≤ 2%).
- Hot path логов под concurrent producers, debug-уровень, `tap_active=true`: бенчмарк с 16/64/256 producer threads × 1k log!()/s. Контроль — отсутствие contention'а на producer'е (no measurable wait time на `try_send`), `dropped_total` растёт только при заведомом burst'е выше channel capacity.
- Frontend: `npm run lint` и `npm run typecheck` зелёные. `npm run build` запущен локально, `frontend/dist/` синхронизирован с `frontend/src/` и закоммичен (CI fail'нет иначе).

## 14. Implementation phases

Будет уточнено в `writing-plans`. Высокоуровневое разбиение:

1. **Reorg + config** — переезд `prometheus → web/metrics`, новая `Web` структура с alias, defaults. Без изменения поведения наружу. ✅ DONE (commit b24082f).
2. **Listener + mux + auth** — расширение `web/server.rs` до mux'а, `web/auth.rs`, отказ при дефолтном пароле. `/metrics` продолжает работать.
3. **Backend routes** — `routes/*.rs` плюс рефакторинг `admin/show.rs` (`collect_*` функции), все public endpoint'ы.
4. **LogTap + admin endpoint'ы** — `log_tap.rs`, `enable_log_tap` в `LogLevelController`, `routes/logs.rs`, reaper-task.
5. **Frontend skeleton** — `frontend/` с Vite, sidebar layout, AuthGate, react-router, `npm run lint`/`typecheck` в CI. Все страницы — заглушки.
6. **Frontend pages** — Overview (с uPlot), Pools, Clients, Caches, Logs, ConfigState. Каждая страница — отдельный коммит с обновлением `frontend/dist/`.
7. **Embedding + docs** — `include_dir!`, BDD-сценарии, обновление `documentation/{en,ru}/src/configuration*.md`. Release-pipeline остаётся cargo-only (никакого `npm run build` шага); CI job для frontend lint/typecheck/dist-sync проверки добавляется отдельным workflow.

## 15. Observability layout & thresholds

Этот раздел кодифицирует, **что показывать на каждой странице**, **как располагать**, **какие пороги считать аномалией**, и **что НЕ подсвечивать** во избежание alert fatigue. Источники: research-агент по Golden Signals + USE/RED methodology, DBA-ревью со сценариями A-H, UX-ревью.

### 15.1 Композиция Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│ Row 1 — Health bar (64 px):                                         │
│ [● OK] 12 pools | 0 paused | 0.20 err/s | hit 99.6% | 1247 active   │
│        | 78/100 servers (78%) | 0 waiting       Updated 0.8s ago    │
├──────────────┬──────────────┬──────────────┬───────────────────────┤
│ Latency P95  │ Traffic      │ Errors/s     │ Saturation max        │
│   (sparkline)│  (qps + tps) │  (sparkline) │  (sparkline + %)      │ Row 2:
│   84 ms      │  1.3k qps    │  0.20 /s     │  78%                  │ Golden
│   ┄┄ 100 ms  │  410 tps     │  ┄┄ 1.0 /s   │  ┄┄ 70% / 90%         │ Signals
│   ┄┄ 500 ms  │              │  ┄┄ 10 /s    │                       │ (96 px)
├──────────────┴──────────────┴──────────────┴───────────────────────┤
│ Connection breakdown — stacked area (active/idle/waiting), 3 min   │ Row 3a
├────────────────────────────────────────────────────────────────────┤
│ Pool fill heatmap — row per pool, 60 cells × saturation %          │ Row 3b
├────────────────────────────────────────────────────────────────────┤
│ Wait queue + oldest-active-age — dual-axis line                    │ Row 3c
├────────────────────────────────────────────────────────────────────┤
│ ▾ Errors per pool — top-5 stacked area                  [collapse] │ Row 3d (default open)
├────────────────────────────────────────────────────────────────────┤
│ ▸ Resource detail — memory, sockets, interner            [collapsed]│ Row 4 (default closed)
└────────────────────────────────────────────────────────────────────┘
```

**Row 1 — Health bar**, единый компонент через всю ширину, всегда видимый. Источник: `/api/overview.health`. Состав: status pill (OK/degraded/critical) + 7 counter chips. Цвет pill = `health.state`. При `state ≠ ok` справа от pill появляется `health.reason` курсивом, чтобы оператор сразу видел «почему».

**Row 2 — Golden Signals strip**, четыре sparkline-карточки в одну строку:

| Карточка | Метрика | Источник | Threshold lines |
|---|---|---|---|
| Latency P95 | `query_p95_max_ms` (max P95 across pools) | `/api/overview` | dashed amber 100 ms, dashed red 500 ms |
| Traffic | qps + tps overlay (две линии) | derived из `query_count_total`, `transaction_count_total` | none (informational) |
| Errors/s | `errors_count_total` derivative + sum prom error sources | derived | dashed amber 1/s, dashed red 10/s |
| Saturation max | `saturation_max_pct` (max fill ratio) | `/api/overview` | dashed amber 70 %, dashed red 90 % |

Высота строки 96 px (фикс), Y-axis log для Latency, linear для остальных. Числовое значение — в правом верхнем углу карточки, IBM Plex Mono semibold 20 px.

**Row 3 — per-aspect detail**, четыре графика 200 px высотой каждый, full-width:

3a. **Connection breakdown** — stacked area, три серии: `active_clients`, `idle_clients`, `waiting_clients` за 3 минуты. Цвета: success / muted / warning соответственно. Threshold paint — нет (это вид «что происходит», не алерт).

3b. **Pool fill heatmap** — вертикальный список пулов (default 12, не больше 30, иначе truncate с кнопкой «show all»), для каждого пула 60 ячеек времени (60 × 1.5 s = 1.5 минуты). Цвет ячейки — saturation %: green 0-69 %, amber 70-89 %, red 90-100 %. Это единственное место, где оператор видит **все пулы одновременно** и сразу замечает «один из них горит».

3c. **Wait queue + oldest-active-age** — line chart с двумя Y-осями. Слева: `waiting_clients` count. Справа: `oldest_active_age_max_ms` log scale ms. Threshold dashed: справа на 30 000 ms (amber) и 300 000 ms (red).

3d. **Errors per pool** — top-5 stacked area (5 пулов с наибольшим error rate за окно). Threshold paint dashed на 1/s. Default open, операторы используют чаще, чем resource detail.

**Row 4 — Resource detail**, default collapsed:
- Process memory (`pg_doorman_total_memory`).
- Sockets by type (linux only, `pg_doorman_sockets`).
- Interner size + bytes per kind.
- Patroni API duration P95 + error rate (если patroni_proxy запущен).

**Cross-hair sync.** Все uPlot чарты в Row 2-3 объединены через `sync.key = 'overview'`. Hover в любом чарте подсвечивает ту же временную точку во всех остальных. Tooltip cursor — vertical guideline 1 px `--accent`, label справа фиксированный.

Состояние раскрытия collapsible-рядов (3d, 4) персистится в `localStorage[overview_rows]`.

### 15.2 Композиция страницы Pools

Per-pool row — таблица + 4 inline sparklines:

| Колонки | Тип |
|---|---|
| `id` (`user@database`) | text + chevron-down (drill-down) |
| `host:port`, `pool_mode` | text |
| Saturation gauge | inline gauge `connections / max_connections` + % |
| Sparkline saturation 3 min | inline 60×24 px |
| Sparkline waiting 3 min | inline 60×24 px |
| Sparkline P95 query 3 min | inline 60×24 px |
| Sparkline errors/s 3 min | inline 60×24 px |
| `paused`/`epoch` | badge |

Click on row → drawer (slide-in справа, 480 px wide) с детальной информацией пула, в т.ч. **серверы этого пула** (бывшая Servers page интегрирована сюда). Drawer содержит: per-server-row таблицу, конфиг пула, last 50 events для пула.

Default sort: `saturation_pct desc`. Critical-пулы (любая метрика в red zone) пиннятся к топу клиентским sort, независимо от выбранной колонки.

### 15.3 Композиция Clients и per-client highlights

Default sort: `queries_total desc`. Visual cues для подсветки аномальных клиентов:

| Условие | Visual cue |
|---|---|
| `state=active` AND `current_query_age_ms > 30000` | левая полоса 2 px amber, badge «long-running» |
| `state=active` AND `current_query_age_ms > 300000` | левая полоса 2 px red, badge «stuck?» |
| `state=waiting` AND `wait_ms > 10000` | левая полоса 2 px amber |
| `errors_total` delta > 0 в окне 60 s | red dot prefix на колонке `errors_total` |
| `application_name` matches `^pg_isready|^pgbench-test|^psql\\b` | row dimmed (text muted), служебные не отвлекают на инциденте |
| `transactions_total < 10` AND `queries_total > 1000` | informational badge «session-mode» (не warning, контекст) |

Эти подсветки делаются на frontend; backend отдаёт raw поля.

### 15.4 Threshold table (codified rules)

Все правила, по которым UI красит amber/red. Backend вычисляет `health.state` по этой же таблице (раздел 8.3). Sustain window — сколько подряд должно держаться, чтобы перейти в state.

| Метрика | Warning | Critical | Sustain | Контекст |
|---|---|---|---|---|
| `pool.connections / pool.max_connections` | ≥ 70 % | ≥ 90 % | instant | per-pool |
| `pool.waiting` | ≥ 1 | ≥ max(10, 0.10×max_connections) | 10 s | absolute count |
| `pool.max_active_age_ms` | > 30 000 | > 300 000 | instant | per-pool max |
| `pool.query_p95_ms` | > 100 | > 500 | 30 s | gauge |
| `pool.query_p99_ms` | > 500 | > 2 000 | 30 s | gauge |
| `pool.wait_avg_ms` | > 5 | > 50 | 30 s | derived из avg |
| `pool.wait_p95_ms` | > 50 | > 500 | 30 s | derived из percentile (backend gap) |
| `pool.errors_per_sec` | > 0.1 | > 1.0 | 30 s | derived из cumulative |
| Auth failure rate (per db) | > 0.5 % attempts | > 5 % | 60 s | `auth_query_auth{result=failure}` |
| TLS handshake error rate (per pool) | > 0 sustained | > 10 % attempts | 30 s | `server_tls_handshake_errors_total` |
| Anonymous LRU evictions (per user/db) | > 0 sustained | > 10 / s | 60 s | подкрутить `client_anonymous_prepared_cache_size` |
| Synthetic misses SQLSTATE 26000 (global) | > 0 sustained | > 1 / s | 60 s | подкрутить `query_interner_anon_idle_ttl_seconds` |
| Backend prepared hit rate (per pool) | < 95 % | < 80 % | 5 min, post-warm-up | `prepared_hits / (hits + misses)` |
| Auth-query cache hit rate (per db) | < 95 % | < 80 % | 5 min, post-warm-up | `auth_query_cache.hits / total` |
| Reconnect rate (per pool) | ≥ 0.10×max_connections / s | ≥ 0.30×max_connections / s | 30 s, suppress ±60 s рестарта | `pool_scaling_total{creates_started}` delta |
| `coordinator.exhaustions_total` rate | > 0 | > 1 / s | 60 s | per-database |
| `burst_gate_budget_exhausted` rate | > 0 | > 0.1 / s | 60 s | per-pool |
| `fallback_active` | = 1 | sustained > 5 min | n/a | per-pool |
| Patroni API error rate | > 5 % | > 50 % | 60 s | per-pool |
| Patroni API P95 duration | > 500 ms | > 2 s | 30 s | `patroni_api_duration_seconds` |
| Process RSS | > 80 % cgroup limit | > 95 % | 60 s | `total_memory` (cgroup gap — раздел 16) |

`health.state` = max severity among all rules whose sustain window прошёл. Reason = первое зафиксированное правило (для UI вывода).

**Где живёт логика (decision #21):** threshold engine — чистая функция в frontend (`src/lib/thresholds.ts`):

```ts
export type Severity = 'ok' | 'degraded' | 'critical';

export function evaluatePool(
  raw: PoolRow,
  history: PoolHistorySlice,
): { severity: Severity; reasons: string[] } { /* ... */ }

export function aggregateHealth(
  overview: OverviewRaw,
  pools: PoolRow[],
  poolHistory: Map<string, PoolHistorySlice>,
): { state: Severity; reason: string | null } { /* ... */ }
```

Backend ничего не знает про severity / state / reason. Это даёт два бонуса: (1) поменять порог = PR во frontend, без backend rebuild и restart; (2) `cargo test` не флапает на изменении порогов.

Sustain window реализуется через `useHistory` — frontend держит rolling window 120 точек × 1.5s = 3 минуты. Правило «errors > 0.1/s sustained 30s» = «20 последних точек подряд `delta(errors_total) / Δt > 0.1`».

**Anti-patterns (15.6) тоже на frontend** — `evaluatePool` имеет access к `pool.epoch` и `pool.fallback_active`, через `useUptime` — к app uptime для warm-up suppression; через `usePoolEpochHistory` — к recent epoch transitions для restart-window suppression.

### 15.5 Anomaly highlighting conventions

Единый словарь подсветки через все страницы:

- **Threshold lines в чартах** — dashed horizontal, `--warning` (amber) и `--danger` (red), opacity 0.4. Drawn под data line.
- **Sparkline cell paint** — при `latest_value` в red zone: левая граница 2 px `--danger`, фон `rgba(--danger, 0.04)`. Для amber — те же правила с `--warning` и opacity 0.03.
- **Numeric value cell** — colored dot prefix (`●`) перед числом для amber/red. **Не меняем цвет текста** (accessibility — number должно читаться независимо от состояния).
- **Pool row** — при любой crit-метрике строки: left rule 2 px `--danger`. Tooltip строки показывает, какое правило сработало.
- **Top bar alert badge** — счётчик crit-пулов справа от health pill. Click → переход на `/pools?filter=critical`.

**Запрещено:** flash, blink, sound, modal popup. UI read-only, реальные алерты живут в Grafana / Alertmanager.

### 15.6 Anti-patterns: что НЕ подсвечивать

Эти правила защищают оператора от ложных срабатываний и alert fatigue. Все они применяются **на стороне backend** при вычислении `health.state` и **на стороне UI** при threshold paint:

1. **Reconnect spike в окне ±60 s от discontinuity `connections_total`.** Это deploy app, нормальный churn. UI: показывает hint `~deploy?` рядом со spike, но не paint'ит red.

2. **Hit rate в первые 5 минут warm-up.** Пока pooler не накопил 10 000 hits + miss attempts на target — поле отображается gray, без threshold paint. Это касается prepared hit rate, interner hit rate, auth-query hit rate.

3. **Wait queue при `fallback_active=1`.** Failover-окно — ожидаемое короткое queueing. UI: вместо red paint пишет inline label «failover» на графике.

4. **TLS handshake duration на первых 100 attempts после старта пула.** OpenSSL прогревает context. UI: первые 100 — без threshold paint, в muted color.

5. **`oldest_active_age_ms < 30 000`.** Любое значение ниже 30 секунд — green, без paint, даже 25 секунд. Только при пересечении 30 s — amber.

6. **Bytes-received / bytes-sent counter resets.** Counter может уменьшаться при reload. UI: clamp производной на ноль; не показываем «отрицательный» traffic.

Anti-patterns 1, 2, 4 кодируются в backend через флаги `degrade_after_uptime_seconds` и `suppress_within_restart_window` в обработчике health-pipeline. UI получает финальный `health.state` уже с учётом этих правил, не дублирует логику.

## 16. Backend gaps blocking 3.8.0

Чтобы dashboard мог честно показывать threshold-таблицу из 15.4 и Overview-композицию из 15.1, в backend pg_doorman нужны два must-have добавления. Принцип «thin server, fat client» (decision #21) минимизирует backend-changes — никаких computed health, errors_per_sec, max-rollups; только новые **сырые** counters/gauges, которые backend всё равно знает (или скоро будет знать) внутри.

### 16.1 Per-pool error counter (must)

Сегодня errors разбросаны по 5 семействам метрик: `auth_query_auth{result=failure}`, `server_tls_handshake_errors_total`, `auth_query_executor{type=errors}`, `fallback_candidate_failures_total`, `query_interner_synthetic_misses_total`. Per-pool error count в Prometheus не агрегируется, и `/api/pools[i].errors_total` отдать нечем.

**Добавить:** `pg_doorman_pool_errors_total{user, database, kind}` где `kind` ∈ `{auth, query, disconnect, timeout, tls}`. Инкрементируется в существующих error-сайтах через тонкий helper:

```rust
fn pool_error(pool_id: &PoolIdentifier, kind: ErrorKind) {
    POOL_ERRORS_TOTAL
        .with_label_values(&[&pool_id.user, &pool_id.db, kind.as_str()])
        .inc();
}
```

Используется в:
- `/api/pools[*].errors_total` — cumulative counter, frontend считает delta для `errors_per_sec`.
- `/api/overview.errors_count_total` — sum по всем pools.
- Threshold-правило «errors per pool» (15.4) — frontend применяет к delta.

### 16.2 Wait time percentiles per pool (must)

Сегодня есть только `pg_doorman_pools_avg_wait_time` (gauge, ms). Avg бесполезен при бимодальном распределении: 99 клиентов ждут 0 мс, один ждёт 5 секунд → avg 50 мс, оператор не видит проблемы.

**Добавить:** `pg_doorman_pools_wait_time_percentile{user, database, percentile}` через тот же HDR-histogram механизм, который уже используется для `pools_queries_percentile` и `pools_transactions_percentile`. Backend изменения механические — переюзается существующий код.

Используется в:
- `/api/pools[*].wait_p95_ms`, `wait_p99_ms` — gauge'и для per-pool sparkline.
- Threshold-правило «wait_p95» (15.4) — frontend применяет к gauge.

---

**`pool_paused` gauge перенесён в nice-to-have:** поле `paused` уже есть в `/api/pools[i]`, frontend знает про paused-state без отдельного Prometheus gauge'а. Gauge нужен только для Grafana parity (`pg_doorman_pool_paused{user, database}` 0|1) — это не блокирует UI.

---

**Nice-to-have для 3.8.1** (не блокируют 3.8.0):
- `pg_doorman_pool_paused{user, database}` (0 | 1) — для Grafana parity (`/api/pools[i].paused` уже отдаёт это поле для UI).
- Max-wait-age в queue (gauge, ms). Аналог `oldest_active_age_ms` для waiting clients.
- Per-pool TLS handshake **success** counter alongside error counter (для honest rate computation).
- `pg_doorman_client_session_duration_seconds_bucket` (histogram).
- Application-name aggregations server-side (для `/api/apps` и `/api/top/clients?by=app`).
- Cgroup memory limit alongside `total_memory`, чтобы RSS % считался без guessing.
- Histogram for query/transaction time (вместо точечных квантилей — позволит Grafana вычислять произвольные percentiles).
- Connection age P99 per pool.

## 17. Open follow-ups (не входит в MVP, явно отложено)

**Visualisation & data:**
- Per-pool breakdown на графиках Overview (легенда, цвета, persistance выбора).
- Server-side `/api/top/queries?by=p99` (требует хранить per-query latency распределение в interner).
- Heatmap zoom in (click cell → переход на time-window-bound view с extended history).
- Annotation events на Row 3 (RELOAD/PAUSE/RECONNECT vertical lines).
- SSE / WebSocket для логов и графиков, если polling окажется слабым местом.
- Серверное хранение rolling-окон (например, 24 часа в SQLite/parquet) для bookmarkable URL с конкретным временным окном (закрывает scenario D из DBA-ревью — postmortem).

**UX:**
- Темизация (light/dark, system-preference).
- i18n.
- Мобильная адаптация sidebar nav.
- Compare view (now vs N-min ago, как Datadog «compare to last week»).
- Inline help (`?` icon) для column headers — доменная семантика для new-hire DBA.
- Saved views (named bookmarks с filter+sort+page) — поверх URL-state.
- Export CSV / JSON для таблиц.

**Admin-команды (требуют security-обзора):**
- PAUSE / RESUME / RECONNECT / RELOAD / SET log_level через UI — после CSRF protection, audit log, idempotency keys на кнопках.

**Backend — для расширенной observability (3.8.1+):**
- Max-wait-age per pool, TLS success counter, client session duration histogram, app_name aggregations, cgroup limits, query latency histogram (вместо точечных квантилей), connection age P99. Полный список — раздел 16 «Nice-to-have для 3.8.1».

**Logs UX (3.8.1):**
- Time picker «jump to ts» в LogStream header (требует, чтобы entries имели ts_ms — уже так).
- Match highlight через `<mark>` подстроки фильтра.
- Multi-line collapse-to-1-line по умолчанию для stack traces, expand on click.

**Keyboard / accessibility:**
- Расширенные shortcuts: `?` overlay со списком всех bindings, `Ctrl+K` quick command palette.
