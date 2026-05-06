# Web UI — Phase 1 Implementation Plan: Reorg + Web Config

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Переименовать `[prometheus]` секцию конфига в `[web]` (с serde alias для обратной совместимости), добавить новые поля (`ui`, `ui_anonymous`, `log_tap_max_entries`), переместить модуль `src/prometheus/` в `src/web/metrics/` без изменения внешнего поведения. После фазы 1 `/metrics` продолжает работать ровно как сейчас, старые конфиги с `[prometheus]` парсятся без изменений.

**Architecture:** Тонкий, чисто рефакторинговый шаг. Ни одной строки логики — только переименования файлов, struct'ов, полей и use-путей. Все существующие тесты должны продолжать проходить без изменений (кроме переименования `Prometheus` → `Web`, которое затронет один-два теста явно). Фаза подготавливает namespace для последующих `web/auth.rs`, `web/log_tap.rs`, `web/routes/`, которые будут жить рядом с `web/metrics/`.

**Tech Stack:** Rust + serde (`#[serde(alias = ...)]` для backwards compat) + cargo (build, test, clippy, fmt). Никакого frontend-кода в этой фазе.

**Reference:**
- Spec: `docs/superpowers/specs/2026-05-06-web-ui-design.md` разделы 4.2 (реорганизация модулей), 5.1-5.2 (TOML, Rust struct), 5.3 (дефолты), 14.1-14.2 (migration).
- Decision log #11 (имя секции `[web]` с alias `prometheus`), #14, #19.

**Не входит в фазу 1** (это будут отдельные планы для фаз 2-7):
- Listener mux, HTTP routing для `/api/*`, auth.
- Routes handlers, `collect_*()` рефакторинг `admin/show.rs`.
- LogTap.
- Frontend.
- BDD-сценарии.

---

## File Structure

**Новые файлы / директории:**
- `src/web/mod.rs` — root модуля. На фазе 1 содержит только `pub mod metrics;`.
- `src/web/metrics/mod.rs` — то, что было `src/prometheus/mod.rs`.
- `src/web/metrics/server.rs` — то, что было `src/prometheus/server.rs`.
- `src/web/metrics/metrics.rs` — то, что было `src/prometheus/metrics.rs`.
- `src/web/metrics/system.rs` — то, что было `src/prometheus/system.rs`.
- `src/web/metrics/tests.rs` — то, что было `src/prometheus/tests.rs`.
- `src/config/web.rs` — то, что было `src/config/prometheus.rs`, содержит `pub struct Web` с alias на `Prometheus`.

**Удаляемые файлы:**
- `src/prometheus/` — вся директория после `git mv` в `src/web/metrics/`.
- `src/config/prometheus.rs` — после переименования в `src/config/web.rs`.

**Модифицируемые файлы:**
- `src/lib.rs` (или `src/main.rs` — узнать в Task 0): убрать `pub mod prometheus;`, добавить `pub mod web;`. Также `mod config;` остаётся.
- `src/config/mod.rs:218`: поле `pub prometheus: Prometheus` → `pub web: Web` с `#[serde(alias = "prometheus")]`.
- `src/config/mod.rs` upper imports: `use crate::config::prometheus::Prometheus;` → `use crate::config::web::Web;`. Также `pub mod prometheus;` → `pub mod web;` (если объявление есть).
- `src/app/server.rs:1`: `use crate::prometheus::{record_interner_gc, start_prometheus_server};` → `use crate::web::metrics::{record_interner_gc, start_prometheus_server};`.
- `src/app/server.rs:361-368`: `config.prometheus.enabled / .host / .port` → `config.web.enabled / .host / .port`.
- `src/app/generate/annotated.rs:189`: `&config.prometheus` → `&config.web`. Любая function `write_prometheus_section` остаётся как есть (это про TOML output, не про struct field) — но имя секции в выводе меняется на `[web]`.
- `pg_doorman.toml` (пример): секция `[prometheus]` → `[web]`, плюс новые поля закомментированными.
- Все usage points `crate::prometheus::SHOW_CONNECTIONS` и подобные глобальные метрики → `crate::web::metrics::SHOW_CONNECTIONS` (Task 5 проходит по всем).

---

## Task 0: Baseline проверка

**Files:** none modified.

- [ ] **Step 0.1: Зафиксировать чистое состояние ветки**

```bash
cd /home/vadv/Projects/pg_doorman
git status
```
Expected: working tree clean (или только uncommitted ожидаемые изменения, типа спеки которая уже была закоммичена). Если есть untracked / unstaged — обсудить с user перед началом плана.

- [ ] **Step 0.2: Зафиксировать прохождение тестов до начала**

```bash
cargo test --lib --quiet 2>&1 | tail -30
```
Expected: PASS, без failures, без warnings. Запомнить число тестов — после фазы 1 оно должно совпасть.

- [ ] **Step 0.3: Зафиксировать состояние clippy**

```bash
cargo clippy --all-targets -- --deny warnings 2>&1 | tail -10
```
Expected: тихий выход, никаких warnings. Если warnings есть — это блокер, фиксим до начала плана.

- [ ] **Step 0.4: Зафиксировать, lib.rs или main.rs корневой модуль**

```bash
ls /home/vadv/Projects/pg_doorman/src/lib.rs /home/vadv/Projects/pg_doorman/src/main.rs 2>/dev/null
```
Зависит от структуры — pg_doorman это бинарь, поэтому возможно отсутствие lib.rs. В оставшихся task'ах используется обозначение `<root_module>` — на этом шаге фиксируем, какой именно файл это.

---

## Task 1: Переименовать `Prometheus` struct → `Web`, добавить новые поля

**Files:**
- Modify: `src/config/prometheus.rs` (переименовываем struct, переносим в новый файл в Task 2).
- Modify: `src/config/mod.rs` (поле `prometheus` → `web` с alias).
- Test: `src/config/tests.rs` (новые тесты на парсинг `[web]` и `[prometheus]` alias).

- [ ] **Step 1.1: Написать failing-тест на парсинг `[web]` секции**

Открыть `src/config/tests.rs`. В конец файла добавить:

```rust
#[tokio::test]
async fn test_config_web_section() {
    use crate::config::Config;
    let toml = r#"
[general]
host = "0.0.0.0"
port = 6432
admin_username = "admin"
admin_password = "secret"

[web]
enabled = true
host = "127.0.0.1"
port = 9128
ui = true
ui_anonymous = false
log_tap_max_entries = 4096

[pools.test]
[pools.test.users]
0 = { username = "u", password = "p", pool_size = 5 }
"#;
    let config: Config = toml::from_str(toml).expect("parse [web] section");
    assert!(config.web.enabled);
    assert_eq!(config.web.host, "127.0.0.1");
    assert_eq!(config.web.port, 9128);
    assert!(config.web.ui);
    assert!(!config.web.ui_anonymous);
    assert_eq!(config.web.log_tap_max_entries, 4096);
}
```

- [ ] **Step 1.2: Написать failing-тест на backwards-compat alias `[prometheus]`**

В тот же файл добавить:

```rust
#[tokio::test]
async fn test_config_prometheus_alias() {
    use crate::config::Config;
    let toml = r#"
[general]
host = "0.0.0.0"
port = 6432
admin_username = "admin"
admin_password = "secret"

[prometheus]
enabled = true
host = "127.0.0.1"
port = 9128

[pools.test]
[pools.test.users]
0 = { username = "u", password = "p", pool_size = 5 }
"#;
    let config: Config = toml::from_str(toml).expect("parse [prometheus] alias");
    assert!(config.web.enabled);
    assert_eq!(config.web.host, "127.0.0.1");
    assert_eq!(config.web.port, 9128);
    // Дефолты новых полей сохраняются
    assert!(!config.web.ui);
    assert!(config.web.ui_anonymous);
    assert_eq!(config.web.log_tap_max_entries, 8192);
}
```

- [ ] **Step 1.3: Запустить тесты, убедиться что они falling**

```bash
cargo test --lib config::tests::test_config_web_section -- --nocapture 2>&1 | tail -20
cargo test --lib config::tests::test_config_prometheus_alias -- --nocapture 2>&1 | tail -20
```
Expected: оба `error[E0...]` или panic про отсутствующее `config.web` поле — компилируется неудачно, как и должно.

- [ ] **Step 1.4: Переименовать `Prometheus` → `Web` в `src/config/prometheus.rs`**

В файле `src/config/prometheus.rs`:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Web {
    #[serde(default = "Web::default_host")]
    pub host: String,
    #[serde(default = "Web::default_port")]
    pub port: u16,
    #[serde(default = "Web::default_enabled")]
    pub enabled: bool,
    #[serde(default = "Web::default_ui")]
    pub ui: bool,
    #[serde(default = "Web::default_ui_anonymous")]
    pub ui_anonymous: bool,
    #[serde(default = "Web::default_log_tap_max_entries")]
    pub log_tap_max_entries: u32,
}

impl Web {
    pub fn empty() -> Web {
        Web {
            host: Self::default_host(),
            port: Self::default_port(),
            enabled: Self::default_enabled(),
            ui: Self::default_ui(),
            ui_anonymous: Self::default_ui_anonymous(),
            log_tap_max_entries: Self::default_log_tap_max_entries(),
        }
    }

    pub fn default_host() -> String { "0.0.0.0".to_string() }
    pub fn default_port() -> u16 { 9127 }
    pub fn default_enabled() -> bool { false }
    pub fn default_ui() -> bool { false }
    pub fn default_ui_anonymous() -> bool { true }
    pub fn default_log_tap_max_entries() -> u32 { 8192 }
}
```

(Если в `src/config/prometheus.rs` есть функция `default_enable` — переименовать в `default_enabled` для консистентности.)

- [ ] **Step 1.5: Обновить `Config` struct в `src/config/mod.rs`**

Найти строку `pub prometheus: Prometheus,` (около line 218) и заменить блок:

```rust
    // Web UI / metrics settings.
    #[serde(default = "Web::empty", alias = "prometheus")]
    pub web: Web,
```

И вверху файла обновить import:

```rust
pub mod web;
```

(Старая строка `pub mod prometheus;` если есть — удалить; иначе создать новую `pub mod web;`.)

В том же файле `pub use prometheus::Prometheus;` (если есть) удалить, добавить `pub use web::Web;`.

- [ ] **Step 1.6: Запустить тесты — failed → переименование пройдёт компиляцию**

```bash
cargo build --lib 2>&1 | tail -30
```
Expected: ошибки компиляции в use-сайтах: `app/server.rs`, `app/generate/annotated.rs`. Это нормально — починим в следующих task'ах.

- [ ] **Step 1.7: Не коммитить пока — фаза 1 завершается коммитом в Task 7**

Переходим к Task 2.

---

## Task 2: Переместить `src/config/prometheus.rs` → `src/config/web.rs`

**Files:**
- Move: `src/config/prometheus.rs` → `src/config/web.rs`.
- Modify: `src/config/mod.rs`.

- [ ] **Step 2.1: git mv файла**

```bash
git mv src/config/prometheus.rs src/config/web.rs
```

- [ ] **Step 2.2: Подтвердить, что внутри файла остался `pub struct Web`**

Open `src/config/web.rs`. Проверить, что после Task 1 он содержит `pub struct Web` со всеми новыми полями.

- [ ] **Step 2.3: Удалить `pub mod prometheus;` из `src/config/mod.rs`**

Если он был добавлен в Task 1 и при этом ещё есть отдельная строка `pub mod prometheus;` — удалить, оставить только `pub mod web;`.

- [ ] **Step 2.4: Запустить cargo build**

```bash
cargo build --lib 2>&1 | tail -30
```
Expected: ошибки компиляции по-прежнему только в `app/server.rs` и `app/generate/annotated.rs`, не в config модуле.

- [ ] **Step 2.5: Не коммитить пока**

---

## Task 3: Обновить call sites вне модуля config

**Files:**
- Modify: `src/app/server.rs:1` (use), `src/app/server.rs:361-368` (config field access).
- Modify: `src/app/generate/annotated.rs:189` (config field access; функция `write_prometheus_section` остаётся, но имя секции в её output меняется на `[web]`).

- [ ] **Step 3.1: `src/app/server.rs` — обновить use-import**

Найти на line 1 (или около):
```rust
use crate::prometheus::{record_interner_gc, start_prometheus_server};
```
Заменить на:
```rust
use crate::web::metrics::{record_interner_gc, start_prometheus_server};
```

(Hmm: на этом шаге crate::web::metrics ещё не существует. Поэтому фактическое перемещение модуля — Task 4. Здесь сохраним временно `crate::prometheus`, обновим в Task 5. Корректировка плана: оставить use-import как есть на этом шаге, обновить только field access.)

**Replan Step 3.1:** обновить только `config.prometheus.*` → `config.web.*`, use-импорт остаётся `use crate::prometheus::*` до Task 5.

Найти на line 361-368:
```rust
if config.prometheus.enabled {
    tokio::task::spawn(async move {
        start_prometheus_server(
            format!("{}:{}", config.prometheus.host, config.prometheus.port).as_str(),
        )
        .await;
    });
}
```
Заменить на:
```rust
if config.web.enabled {
    tokio::task::spawn(async move {
        start_prometheus_server(
            format!("{}:{}", config.web.host, config.web.port).as_str(),
        )
        .await;
    });
}
```

- [ ] **Step 3.2: `src/app/generate/annotated.rs:189` — обновить field access**

Найти строку с `&config.prometheus` (около line 189) и заменить на `&config.web`. Имя самой функции (`write_prometheus_section` или похожее) на этом шаге не трогаем — оно касается output-секции в TOML, тоже надо переименовать, но в Task 6.

- [ ] **Step 3.3: Проверить `cargo build`**

```bash
cargo build --lib 2>&1 | tail -10
```
Expected: компиляция проходит. Если осталась ошибка `field 'prometheus' not found on Config` — дополнительные usage points, найти их через `grep -rn 'config\.prometheus' src/`:

```bash
grep -rn 'config\.prometheus' src/
```
И заменить каждый на `config.web`.

- [ ] **Step 3.4: Прогнать тесты, чтобы убедиться что нечего не сломалось**

```bash
cargo test --lib 2>&1 | tail -20
```
Expected: PASS все тесты, кроме новых из Task 1.5–1.7 (но они уже должны проходить, потому что field renamed).

- [ ] **Step 3.5: Не коммитить пока**

---

## Task 4: Reorg `src/prometheus/` → `src/web/metrics/`

**Files:**
- Create: `src/web/mod.rs`.
- Move: `src/prometheus/{mod.rs, server.rs, metrics.rs, system.rs, tests.rs}` → `src/web/metrics/`.
- Modify: `<root_module>` (lib.rs или main.rs — найдено в Task 0): убрать `pub mod prometheus;`, добавить `pub mod web;`.

- [ ] **Step 4.1: Создать `src/web/` директорию + `src/web/mod.rs`**

```bash
mkdir -p /home/vadv/Projects/pg_doorman/src/web
```

Создать файл `src/web/mod.rs` со следующим содержимым:

```rust
//! Web subsystem: Prometheus metrics endpoint, future REST API for the UI,
//! authentication, log tap, and SPA static assets.
//!
//! Phase 1 wires only the metrics submodule (the former `crate::prometheus`).
//! Auth, routes, log_tap, and static_assets are added in subsequent phases.

pub mod metrics;
```

- [ ] **Step 4.2: Переместить файлы через git mv**

```bash
cd /home/vadv/Projects/pg_doorman
git mv src/prometheus src/web/metrics
```

После этого `src/web/metrics/` должен содержать `mod.rs`, `server.rs`, `metrics.rs`, `system.rs`, `tests.rs`. Директория `src/prometheus/` исчезает.

- [ ] **Step 4.3: Обновить root module declaration**

В файле, найденном в Task 0.4 (например `src/lib.rs` или `src/main.rs`), найти строку `pub mod prometheus;` (или `mod prometheus;`) и заменить на `pub mod web;` (или `mod web;` соответственно — какой visibility был у prometheus, такой же оставить у web).

Если оба варианта `pub mod prometheus;` и `mod prometheus;` появлялись в разных файлах — обновить обе.

- [ ] **Step 4.4: Запустить cargo build**

```bash
cargo build --lib 2>&1 | tail -30
```
Expected: ошибки в каждом файле, который имеет `use crate::prometheus::*;` или ссылается на `crate::prometheus::SHOW_CONNECTIONS` (и подобные глобальные метрики). Это нормально, чиним в Task 5.

- [ ] **Step 4.5: Не коммитить пока**

---

## Task 5: Обновить все use crate::prometheus → crate::web::metrics

**Files:**
- Modify: каждый `.rs` файл с использованием `crate::prometheus`.

- [ ] **Step 5.1: Найти все use-сайты**

```bash
grep -rln 'crate::prometheus' src/
```

Скорее всего попадут: `src/app/server.rs`, `src/pool/*.rs` (fallback.rs и пр.), `src/server/*.rs`, `src/web/metrics/metrics.rs` (внутри сам себя — `super::REGISTRY` и пр., их не трогаем), `src/admin/*.rs`. Плюс возможно `src/stats/`.

- [ ] **Step 5.2: Применить замену**

Для каждого файла из вывода Step 5.1, заменить `crate::prometheus` на `crate::web::metrics`. Это можно сделать одним sed (но с обязательной перепроверкой grep'ом после):

```bash
grep -rln 'crate::prometheus' src/ | xargs sed -i 's|crate::prometheus|crate::web::metrics|g'
```

(Без `--no-backup` или подобного — sed -i работает inplace.)

- [ ] **Step 5.3: Запустить cargo build, проверить что компиляция идёт**

```bash
cargo build --lib 2>&1 | tail -30
```
Expected: компиляция должна пройти. Если остались ошибки про `crate::prometheus` — найти их, исправить вручную (типичная причина — multiline use или формат `use crate ::\n  prometheus`). Проверить также:

```bash
grep -rn 'crate::prometheus' src/
```
Expected: пусто.

- [ ] **Step 5.4: Запустить полный набор тестов**

```bash
cargo test --lib 2>&1 | tail -20
```
Expected: PASS, тестов столько же сколько в Task 0.2 + 2 новых (test_config_web_section, test_config_prometheus_alias).

- [ ] **Step 5.5: Запустить интеграционные тесты prometheus**

```bash
cargo test --lib web::metrics::tests::test_prometheus_server_basic 2>&1 | tail -20
```
Expected: PASS. Этот тест продолжает использовать имя `start_prometheus_server` функции — оно не переименовывается в фазе 1.

- [ ] **Step 5.6: Не коммитить пока**

---

## Task 6: Обновить пример `pg_doorman.toml`

**Files:**
- Modify: `pg_doorman.toml` (или другой example в корне репо).
- Modify: `src/app/generate/annotated.rs` — функция, которая генерирует `[prometheus]`-секцию, должна теперь генерировать `[web]` с новыми полями.

- [ ] **Step 6.1: Найти и обновить example TOML**

В `pg_doorman.toml` (linе 410–423) заменить блок:

```toml
# ############################################################################
# PROMETHEUS METRICS
# ############################################################################
[prometheus]
# Enable Prometheus metrics exporter.
# Default: false
enabled = false

# Host for the metrics HTTP endpoint.
# Default: "0.0.0.0"
host = "0.0.0.0"

# Port for the metrics HTTP endpoint.
# Default: 9127
port = 9127
```

на:

```toml
# ############################################################################
# WEB UI / METRICS
# ############################################################################
# The legacy [prometheus] section name is also accepted as an alias for
# backwards compatibility.
[web]
# Enable HTTP listener (Prometheus metrics + future Web UI).
# Default: false
enabled = false

# Host for the HTTP endpoint.
# Default: "0.0.0.0"
host = "0.0.0.0"

# Port for the HTTP endpoint.
# Default: 9127
port = 9127

# Serve the Web UI (and /api/* routes) on this listener.
# Requires admin_password to be set to a non-default value.
# Default: false
ui = false

# Allow unauthenticated access to the Web UI public pages.
# When false, basic-auth (admin_username/admin_password) is required for /.
# Default: true
ui_anonymous = true

# Capacity of the in-memory log tail buffer (entries, not bytes).
# Set to 0 to disable /api/logs entirely.
# Default: 8192
log_tap_max_entries = 8192
```

- [ ] **Step 6.2: Обновить `src/app/generate/annotated.rs`**

Найти функцию, генерирующую секцию (вероятно `write_prometheus_section` или подобное). Переименовать заголовок секции в выводе с `[prometheus]` на `[web]`. Добавить новые поля (`ui`, `ui_anonymous`, `log_tap_max_entries`). Если у функции есть имя `write_prometheus_section` — переименовать в `write_web_section` (только в этом файле, проверить нет ли ссылок).

```bash
grep -rn 'write_prometheus_section' src/
```
И заменить везде.

- [ ] **Step 6.3: Запустить cargo build, тесты**

```bash
cargo build --lib 2>&1 | tail -10
cargo test --lib 2>&1 | tail -20
```
Expected: PASS, без warnings.

- [ ] **Step 6.4: Не коммитить пока**

---

## Task 7: Final-проверка + коммит

**Files:** none modified at this stage.

- [ ] **Step 7.1: cargo fmt**

```bash
cargo fmt
git diff --stat
```
Expected: либо diff пустой (если код уже отформатирован), либо мелкие forматные изменения. Никаких больших diffs — если есть, разобраться откуда.

- [ ] **Step 7.2: cargo clippy --deny warnings**

```bash
cargo clippy --all-targets -- --deny warnings 2>&1 | tail -20
```
Expected: тихий выход, никаких warnings. Если warnings есть — фиксим до коммита.

- [ ] **Step 7.3: cargo test (полный набор)**

```bash
cargo test --lib 2>&1 | tail -30
```
Expected: PASS. Число тестов = baseline (Task 0.2) + 2 (новые tests из Task 1).

- [ ] **Step 7.4: Smoke check `/metrics` endpoint**

Поднять doorman локально с дефолтным конфигом, в котором `enabled = true`:

```bash
cargo build --release 2>&1 | tail -5
```

Создать temp config:

```bash
cat > /tmp/doorman-phase1.toml <<'EOF'
[general]
host = "127.0.0.1"
port = 16432
admin_username = "admin"
admin_password = "phase1test"

[web]
enabled = true
host = "127.0.0.1"
port = 19127

[pools.smoke]
[pools.smoke.users]
0 = { username = "u", password = "p", pool_size = 5 }
EOF

./target/release/pg_doorman --config /tmp/doorman-phase1.toml &
DOORMAN_PID=$!
sleep 2
curl -s http://127.0.0.1:19127/metrics | head -20
kill $DOORMAN_PID
```
Expected: `pg_doorman_*` метрики в выводе. Это smoke-тест: фаза 1 не должна сломать /metrics endpoint.

- [ ] **Step 7.5: Smoke check backwards-compat alias**

Тот же smoke, но с `[prometheus]` вместо `[web]`:

```bash
cat > /tmp/doorman-phase1-alias.toml <<'EOF'
[general]
host = "127.0.0.1"
port = 16432
admin_username = "admin"
admin_password = "phase1test"

[prometheus]
enabled = true
host = "127.0.0.1"
port = 19127

[pools.smoke]
[pools.smoke.users]
0 = { username = "u", password = "p", pool_size = 5 }
EOF

./target/release/pg_doorman --config /tmp/doorman-phase1-alias.toml &
DOORMAN_PID=$!
sleep 2
curl -s http://127.0.0.1:19127/metrics | head -5
kill $DOORMAN_PID
```
Expected: точно такой же успех — alias работает.

- [ ] **Step 7.6: Pre-commit code review**

Согласно CLAUDE.md правилу — перед commit'ом запустить отдельного агента для code review с черновиком commit-сообщения.

Черновик:
```
refactor(web): rename [prometheus] config section to [web], move src/prometheus to src/web/metrics

Что требовалось: подготовить namespace для будущего Web UI (auth, log tap, REST routes), переименовать [prometheus] секцию конфига в [web], добавить новые поля (ui, ui_anonymous, log_tap_max_entries) с защищающими дефолтами, не сломав существующие конфиги пользователей.

Суть: модуль src/prometheus стал src/web/metrics, Prometheus struct стал Web с serde alias на старое имя; новые поля имеют conservative дефолты (ui=false, ui_anonymous=true, log_tap_max_entries=8192). /metrics endpoint работает как раньше; конфиги с [prometheus] продолжают парситься без изменений. Никакой новой логики — чисто рефакторинг под фазу 1 имплементации Web UI.
```

Дальше — диспатч Agent с `subagent_type: general-purpose`, `model: opus`, передаём весь промпт code-review агента из CLAUDE.md, с этим черновиком.

- [ ] **Step 7.7: Если ревью «КОММИТ ЗАБЛОКИРОВАН» — починить блокеры, повторить ревью**

Цикл до получения «Ревью пройдено».

- [ ] **Step 7.8: Создать единый коммит фазы 1**

```bash
git add -A
git status   # глянуть что попадает
git commit -m "refactor(web): rename [prometheus] config section to [web], move src/prometheus to src/web/metrics

Renamed config section [prometheus] to [web] with serde alias for backward
compatibility, added new fields ui, ui_anonymous and log_tap_max_entries with
conservative defaults. Moved src/prometheus to src/web/metrics. /metrics
endpoint remains unchanged; existing configs with [prometheus] continue to
parse. No behavior change — preparation namespace for upcoming Web UI phases."
```

(Commit message — на английском, по project convention из feedback memory.)

- [ ] **Step 7.9: Проверить, что ветка в чистом состоянии после коммита**

```bash
git log --oneline -1
git status
```
Expected: HEAD на новом коммите, working tree clean.

- [ ] **Step 7.10: Mark task #3 в TaskList как completed**

Это сигнал, что фаза 1 готова. Дальнейшие фазы 2-7 — отдельные планы.

---

## Self-review

**Spec coverage check:**
- ✅ Reorg `src/prometheus/` → `src/web/metrics/` — Task 4.
- ✅ Новый `Web` struct с alias — Task 1, 2.
- ✅ Новые поля `ui`, `ui_anonymous`, `log_tap_max_entries` с дефолтами — Task 1.4.
- ✅ Дефолты `ui=false`, `ui_anonymous=true`, `log_tap_max_entries=8192` (раздел 5.3) — Task 1.4.
- ✅ Backwards-compat для старого `[prometheus]` — Task 1.2 (тест), Task 1.5 (alias).
- ✅ `/metrics` продолжает работать — Task 7.4 (smoke).
- ✅ Pre-commit code review — Task 7.6.
- ✅ Commit message — Task 7.8.

**Не покрыто этой фазой (намеренно):**
- mux на listener'е → фаза 2.
- Auth → фаза 2.
- Backend routes / collect_*() рефакторинг — фаза 3.
- LogTap — фаза 4.
- Frontend skeleton — фаза 5.
- Frontend pages — фаза 6.
- Embedding + CI — фаза 7.

**Type-consistency check:**
- `Web` struct: одно имя по всему плану.
- `default_enabled` (не `default_enable`) — поправлено в Task 1.4.
- Field name `pub web: Web` — одинаково в Config (Task 1.5) и в use-сайтах (Task 3).

**Placeholder check:** обыскал план на «TBD», «implement later», «similar to» — отсутствуют. Каждый шаг содержит либо точный код, либо точную команду.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-1.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review между task'ами, fast iteration.
2. **Inline Execution** — выполняем task'и в этой сессии последовательно с checkpoint'ами для review.

Which approach?
