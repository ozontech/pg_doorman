# Web UI — Phase 3c-2 Implementation Plan: 6 ConfigState endpoints

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Поднять `GET /api/config` (с маскированием секретов), `/api/log_level`, `/api/auth_query`, `/api/pool_scaling`, `/api/pool_coordinator`, `/api/sockets` (linux-only). Это оставшиеся 6 ConfigState endpoint'ов из spec section 8.2.

**Architecture:** Каждый endpoint — тривиальная обёртка над `collect_*()` в `collect.rs`, читающей global state. Поля зеркалят соответствующие admin SHOW-команды (`show_config`, `show_log_level`, `show_auth_query`, `show_pool_scaling`, `show_pool_coordinator`, `show_sockets`).

**Секрет-маскинг для `/api/config`:** Pure helper `is_secret_key(key: &str) -> bool` — true для имён, точно равных `password` или `secret`, либо имеющих суффиксы `_password`, `_secret`, `_token`, `_key`. При сериализации значение секретного поля заменяется на `"***"`. Тест `is_secret_key` фиксирует whitelist. Текущий `From<&Config> for HashMap<String, String>` (config/mod.rs:257) пока не включает большинства секретов — это ограничение существующего admin-протокола, расширение HashMap-конверсии — отдельная задача (out of scope этой фазы); маскинг ставится готовым для случая когда конверсия будет расширена.

**Linux-only `/api/sockets`:** На non-linux handler возвращает `503 Service Unavailable` с body `{"error":"not_supported","message":"sockets endpoint requires Linux"}`. Соответствует `#[cfg(target_os = "linux")]` на existing `show_sockets`. Содержательный код — за `#[cfg(target_os = "linux")]`.

**Reference:**
- Spec: section 8.2 (полный endpoint list, маскинг секретов, linux-only sockets), section 8.1 (общие принципы).
- Phase 3c-1 commit: `509e876`.
- Существующие admin SHOW: `src/admin/show.rs:105` (log_level), `:429` (config), `:656` (auth_query), `:712` (sockets), `:774` (pool_scaling), `:824` (pool_coordinator).

**Не входит в фазу 3c-2:**
- `/api/prepared`, `/api/interner`, admin stubs `/api/prepared/text/{hash}`, `/api/interner/top` — phase 3c-3.
- Расширение `From<&Config> for HashMap<String, String>` для покрытия `admin_password`, `talos_jwt_secret`, per-user passwords — out of scope (phase 3c-2 ограничивается зеркалированием существующего show_config + установкой маскера).
- Top-N (`/api/top/*`), `/api/apps`, `/api/events` — phase 3d.

---

## File Structure

**Новые файлы:**
- `src/web/routes/config.rs` — handler `GET /api/config` + маскер.
- `src/web/routes/log_level.rs` — handler `GET /api/log_level`.
- `src/web/routes/auth_query.rs` — handler `GET /api/auth_query`.
- `src/web/routes/pool_scaling.rs` — handler `GET /api/pool_scaling`.
- `src/web/routes/pool_coordinator.rs` — handler `GET /api/pool_coordinator`.
- `src/web/routes/sockets.rs` — handler `GET /api/sockets` (linux-only содержимое).

**Модифицируемые файлы:**
- `src/web/routes/dto.rs` — добавить `ConfigDto`, `ConfigEntry`, `LogLevelDto`, `AuthQueryDto`, `AuthQueryRowDto`, `PoolScalingDto`, `PoolScalingRowDto`, `PoolCoordinatorDto`, `PoolCoordinatorRowDto`, `SocketsDto`, `SocketCounts`.
- `src/web/routes/collect.rs` — добавить `collect_config`, `collect_log_level`, `collect_auth_query`, `collect_pool_scaling`, `collect_pool_coordinator`, плюс на linux `collect_sockets`. Pure helper `is_secret_key`.
- `src/web/routes/mod.rs` — register 6 modules.
- `src/web/server.rs` — 6 arm'ов в `route_api` + 6 dispatch tests.
- `src/web/tests.rs` — 6 integration tests.

---

## Task 0: Baseline проверка

**Files:** none modified.

- [ ] **Step 0.1: Зафиксировать чистое состояние ветки**

```bash
cd /home/vadv/Projects/pg_doorman
git status
git log --oneline -3
```
Expected: HEAD = `509e876`, working tree clean (untracked `.local/`, `Dockerfile.ubuntu22-tls`, `INCIDENT_*.md` допустимы).

- [ ] **Step 0.2: Зафиксировать baseline тестов**

```bash
cargo test --lib --quiet 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 730 passed, clippy clean, fmt clean.

---

## Task 1: добавить DTOs в `dto.rs`

**Files:**
- Modify: `src/web/routes/dto.rs`.

- [ ] **Step 1.1: `ConfigDto` + `ConfigEntry`**

Добавить в конец `dto.rs`:

```rust
/// `GET /api/config` — flattened key/value view of the active configuration.
///
/// Mirrors the columns of `SHOW CONFIG`. Values for secret keys are replaced
/// with `"***"`; the predicate is documented on `is_secret_key` in collect.rs.
/// The flat representation today omits per-user passwords, admin_password,
/// talos_jwt_secret and similar (existing limitation of
/// `From<&Config> for HashMap<String, String>`); when that conversion is
/// later extended the masker will pick up the new keys automatically.
#[derive(Debug, Serialize)]
pub struct ConfigDto {
    pub ts: u64,
    pub config: Vec<ConfigEntry>,
}

#[derive(Debug, Serialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
    /// Marker text used by `SHOW CONFIG` for the default-value column.
    /// pg_doorman has never populated real defaults here; kept for shape parity.
    pub default: &'static str,
    /// `"yes"` for keys that take effect on `RELOAD`, `"no"` for keys that
    /// require a restart. Mirrors the `immutables` list inside `show_config`.
    pub changeable: &'static str,
}
```

- [ ] **Step 1.2: `LogLevelDto`**

```rust
/// `GET /api/log_level` — the active log filter (RUST_LOG-style).
#[derive(Debug, Serialize)]
pub struct LogLevelDto {
    pub ts: u64,
    pub log_level: String,
}
```

- [ ] **Step 1.3: `AuthQueryDto` + `AuthQueryRowDto`**

```rust
/// `GET /api/auth_query` — per-pool auth_query cache and authentication
/// metrics. Field names mirror `SHOW AUTH_QUERY` columns.
#[derive(Debug, Serialize)]
pub struct AuthQueryDto {
    pub ts: u64,
    pub pools: Vec<AuthQueryRowDto>,
}

#[derive(Debug, Serialize)]
pub struct AuthQueryRowDto {
    pub database: String,
    pub cache_entries: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_refetches: u64,
    pub cache_rate_limited: u64,
    pub auth_success: u64,
    pub auth_failure: u64,
    pub executor_queries: u64,
    pub executor_errors: u64,
    pub dynamic_pools_current: u64,
    pub dynamic_pools_created: u64,
    pub dynamic_pools_destroyed: u64,
}
```

- [ ] **Step 1.4: `PoolScalingDto` + `PoolScalingRowDto`**

```rust
/// `GET /api/pool_scaling` — per-pool counters for the anticipation and
/// bounded-burst create paths. Field names mirror `SHOW POOL_SCALING`.
#[derive(Debug, Serialize)]
pub struct PoolScalingDto {
    pub ts: u64,
    pub pools: Vec<PoolScalingRowDto>,
}

#[derive(Debug, Serialize)]
pub struct PoolScalingRowDto {
    pub user: String,
    pub database: String,
    pub inflight: u64,
    pub creates: u64,
    pub gate_waits: u64,
    pub gate_budget_ex: u64,
    pub antic_notify: u64,
    pub antic_timeout: u64,
    pub create_fallback: u64,
    pub replenish_def: u64,
}
```

- [ ] **Step 1.5: `PoolCoordinatorDto` + `PoolCoordinatorRowDto`**

```rust
/// `GET /api/pool_coordinator` — per-database limits and reserve-pool counters.
/// Field names mirror `SHOW POOL_COORDINATOR`.
#[derive(Debug, Serialize)]
pub struct PoolCoordinatorDto {
    pub ts: u64,
    pub databases: Vec<PoolCoordinatorRowDto>,
}

#[derive(Debug, Serialize)]
pub struct PoolCoordinatorRowDto {
    pub database: String,
    pub max_db_conn: u64,
    pub current: u64,
    pub reserve_size: u64,
    pub reserve_used: u64,
    pub evictions: u64,
    pub reserve_acq: u64,
    pub exhaustions: u64,
}
```

- [ ] **Step 1.6: `SocketsDto` + counts**

JSON shape отражает структуру `SocketStateCount` после visibility bump (см. Task 2 Step 2.0). `unix_stream` остаётся отдельной группой; `unix_dgram` и `unix_seq_packet` — отдельные top-level счётчики (не вложены в struct), потому что в backend они хранятся как `u16`-поля рядом с `unix_stream`, а не внутри.

```rust
/// `GET /api/sockets` — TCP / TCP6 / Unix socket state counts. Linux-only.
/// Field names mirror the backend `SocketStateCount` and (transitively)
/// the columns of `SHOW SOCKETS`.
#[derive(Debug, Serialize)]
pub struct SocketsDto {
    pub ts: u64,
    pub tcp: TcpCounts,
    pub tcp6: TcpCounts,
    pub unix_stream: UnixStreamCounts,
    pub unix_dgram: u64,
    pub unix_seq_packet: u64,
    pub unknown: u64,
}

#[derive(Debug, Serialize, Default)]
pub struct TcpCounts {
    pub established: u64,
    pub syn_sent: u64,
    pub syn_recv: u64,
    pub fin_wait1: u64,
    pub fin_wait2: u64,
    pub time_wait: u64,
    pub close: u64,
    pub close_wait: u64,
    pub last_ack: u64,
    pub listen: u64,
    pub closing: u64,
    pub new_syn_recv: u64,
    pub bound_inactive: u64,
}

#[derive(Debug, Serialize, Default)]
pub struct UnixStreamCounts {
    pub free: u64,
    pub unconnected: u64,
    pub connecting: u64,
    pub connected: u64,
    pub disconnecting: u64,
}
```

- [ ] **Step 1.7: cargo build + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 1.8: Не коммитить.**

---

## Task 2: collect-функции + unit-тесты на маскер

**Files:**
- Modify: `src/stats/socket.rs` (visibility bump для socket-counter struct'ов и полей).
- Modify: `src/web/routes/collect.rs`.

- [ ] **Step 2.0: Поднять видимость socket-count struct'ов**

В `src/stats/socket.rs` нужно три изменения для того чтобы `collect_sockets` мог читать поля. Все правки — только видимость:

```rust
// line 33-34: было `#[derive(Default)] struct TcpStateCount {`
#[derive(Default)]
pub struct TcpStateCount {
    pub established: u16,
    pub syn_sent: u16,
    pub syn_recv: u16,
    pub fin_wait1: u16,
    pub fin_wait2: u16,
    pub time_wait: u16,
    pub close: u16,
    pub close_wait: u16,
    pub last_ack: u16,
    pub listen: u16,
    pub closing: u16,
    pub new_syn_recv: u16,
    pub bound_inactive: u16,
    pub total_count: u32,
}

// line 53-54: было `#[derive(Default)] struct UnixStreamStateCount {`
#[derive(Default)]
pub struct UnixStreamStateCount {
    pub free: u16,
    pub unconnected: u16,
    pub connecting: u16,
    pub connected: u16,
    pub disconnecting: u16,
    pub total_count: u32,
}

// line 65-72: SocketStateCount уже pub struct, но поля private
#[derive(Default)]
pub struct SocketStateCount {
    pub tcp: TcpStateCount,
    pub tcp6: TcpStateCount,
    pub unix_stream: UnixStreamStateCount,
    pub unix_dgram: u16,
    pub unix_seq_packet: u16,
    pub unknown: u16,
}
```

**Это единственное изменение в `src/stats/socket.rs`.** Остальное — внутри `src/web/`.

- [ ] **Step 2.1: Расширить use-импорты**

В верхней use-секции добавить нужные импорты:

```rust
use crate::pool::{AUTH_QUERY_STATE, COORDINATORS, DYNAMIC_POOLS};
use crate::app::log_level;
use crate::config::get_config;
use crate::web::routes::dto::{
    AuthQueryDto, AuthQueryRowDto, ConfigDto, ConfigEntry, LogLevelDto,
    PoolCoordinatorDto, PoolCoordinatorRowDto, PoolScalingDto, PoolScalingRowDto,
    // ... existing entries
};
```

На linux дополнительно:

```rust
#[cfg(target_os = "linux")]
use crate::web::routes::dto::{SocketsDto, TcpCounts, UnixStreamCounts};
```

- [ ] **Step 2.2: `is_secret_key` pure helper + `collect_config`**

```rust
/// Returns `true` for configuration keys whose value should be masked in
/// `/api/config`. A key is secret if its trailing path segment (after the
/// last `.`) is exactly `password` or `secret`, or has any of the suffixes
/// `_password`, `_secret`, `_token`, `_key`.
///
/// The trailing-segment matching is so that `pools.foo.users.bar.password`
/// is recognised as secret, not just top-level `password`.
fn is_secret_key(key: &str) -> bool {
    let last_segment = key.rsplit('.').next().unwrap_or(key);
    matches!(last_segment, "password" | "secret")
        || last_segment.ends_with("_password")
        || last_segment.ends_with("_secret")
        || last_segment.ends_with("_token")
        || last_segment.ends_with("_key")
}

pub fn collect_config() -> ConfigDto {
    // Mirrors `show_config` in src/admin/show.rs:429 for the immutables list
    // (these are the only fields that require a restart to change).
    const IMMUTABLES: &[&str] = &["host", "port", "connect_timeout"];

    let config = get_config();
    let flat: std::collections::HashMap<String, String> = (&config).into();

    let mut entries: Vec<ConfigEntry> = flat
        .into_iter()
        .map(|(key, value)| {
            let value = if is_secret_key(&key) {
                "***".to_string()
            } else {
                value
            };
            let changeable = if IMMUTABLES.iter().any(|c| *c == key) {
                "no"
            } else {
                "yes"
            };
            ConfigEntry {
                key,
                value,
                default: "-",
                changeable,
            }
        })
        .collect();

    entries.sort_by(|a, b| a.key.cmp(&b.key));

    ConfigDto {
        ts: now_unix_ms(),
        config: entries,
    }
}
```

**Verify:** `get_config()` возвращает owned `Config` (config/mod.rs:694), не `Arc`. `(&config).into()` использует `impl From<&Config> for HashMap<String, String>`.

- [ ] **Step 2.3: `collect_log_level`**

```rust
pub fn collect_log_level() -> LogLevelDto {
    LogLevelDto {
        ts: now_unix_ms(),
        log_level: log_level::get_log_level(),
    }
}
```

- [ ] **Step 2.4: `collect_auth_query`**

```rust
pub fn collect_auth_query() -> AuthQueryDto {
    let states = AUTH_QUERY_STATE.load();
    let dynamic = DYNAMIC_POOLS.load();

    let mut pools: Vec<AuthQueryRowDto> = states
        .iter()
        .map(|(pool_name, state)| {
            let cache_entries = state.cache_len() as u64;
            let dyn_current = dynamic.iter().filter(|id| id.db == *pool_name).count() as u64;
            let s = state.stats.snapshot();
            AuthQueryRowDto {
                database: pool_name.clone(),
                cache_entries,
                cache_hits: s.cache_hits,
                cache_misses: s.cache_misses,
                cache_refetches: s.cache_refetches,
                cache_rate_limited: s.cache_rate_limited,
                auth_success: s.auth_success,
                auth_failure: s.auth_failure,
                executor_queries: s.executor_queries,
                executor_errors: s.executor_errors,
                dynamic_pools_current: dyn_current,
                dynamic_pools_created: s.dynamic_pools_created,
                dynamic_pools_destroyed: s.dynamic_pools_destroyed,
            }
        })
        .collect();

    pools.sort_by(|a, b| a.database.cmp(&b.database));

    AuthQueryDto {
        ts: now_unix_ms(),
        pools,
    }
}
```

**Verify:** `state.stats.snapshot()` поля. `state.cache_len()` returns usize. Если поля snapshot отличаются — поправить. Грепнуть `pub struct .*Stats` в `src/pool/auth_query_state.rs`.

- [ ] **Step 2.5: `collect_pool_scaling`**

```rust
pub fn collect_pool_scaling() -> PoolScalingDto {
    let mut entries: Vec<_> = get_all_pools()
        .iter()
        .map(|(id, pool)| (id.clone(), pool.database.scaling_stats()))
        .collect();
    entries.sort_by(|a, b| (&a.0.db, &a.0.user).cmp(&(&b.0.db, &b.0.user)));

    let pools = entries
        .into_iter()
        .map(|(id, snapshot)| PoolScalingRowDto {
            user: id.user.clone(),
            database: id.db.clone(),
            inflight: snapshot.inflight_creates,
            creates: snapshot.creates_started,
            gate_waits: snapshot.burst_gate_waits,
            gate_budget_ex: snapshot.burst_gate_budget_exhausted,
            antic_notify: snapshot.anticipation_wakes_notify,
            antic_timeout: snapshot.anticipation_wakes_timeout,
            create_fallback: snapshot.create_fallback,
            replenish_def: snapshot.replenish_deferred,
        })
        .collect();

    PoolScalingDto {
        ts: now_unix_ms(),
        pools,
    }
}
```

**Verify:** field имена в `ScalingStats` snapshot. Грепнуть `pub struct ScalingStats` или fields на `inflight_creates|creates_started|burst_gate_waits|anticipation_wakes_notify`.

- [ ] **Step 2.6: `collect_pool_coordinator`**

```rust
pub fn collect_pool_coordinator() -> PoolCoordinatorDto {
    let coordinators = COORDINATORS.load();
    let mut databases: Vec<PoolCoordinatorRowDto> = coordinators
        .iter()
        .map(|(db, coordinator)| {
            let stats = coordinator.stats();
            let config = coordinator.config();
            PoolCoordinatorRowDto {
                database: db.clone(),
                max_db_conn: config.max_db_connections as u64,
                current: stats.total_connections as u64,
                reserve_size: config.reserve_pool_size as u64,
                reserve_used: stats.reserve_in_use as u64,
                evictions: stats.evictions_total,
                reserve_acq: stats.reserve_acquisitions_total,
                exhaustions: stats.exhaustions_total,
            }
        })
        .collect();

    databases.sort_by(|a, b| a.database.cmp(&b.database));

    PoolCoordinatorDto {
        ts: now_unix_ms(),
        databases,
    }
}
```

**Verify:** field types на `coordinator.stats()` / `coordinator.config()`. `total_connections` / `reserve_in_use` могут быть `usize` или `AtomicUsize`-load.

- [ ] **Step 2.7: `collect_sockets` (linux-only)**

`get_socket_states_count` живёт в `src/stats/socket.rs:324` и возвращает `Result<SocketStateCount, SocketInfoErr>` (struct из step 2.0).

```rust
#[cfg(target_os = "linux")]
pub fn collect_sockets() -> Result<SocketsDto, &'static str> {
    use crate::stats::socket::{get_socket_states_count, TcpStateCount, UnixStreamStateCount};

    let info = get_socket_states_count(std::process::id())
        .map_err(|_| "failed to read socket states from /proc")?;

    fn tcp(c: &TcpStateCount) -> TcpCounts {
        TcpCounts {
            established: c.established as u64,
            syn_sent: c.syn_sent as u64,
            syn_recv: c.syn_recv as u64,
            fin_wait1: c.fin_wait1 as u64,
            fin_wait2: c.fin_wait2 as u64,
            time_wait: c.time_wait as u64,
            close: c.close as u64,
            close_wait: c.close_wait as u64,
            last_ack: c.last_ack as u64,
            listen: c.listen as u64,
            closing: c.closing as u64,
            new_syn_recv: c.new_syn_recv as u64,
            bound_inactive: c.bound_inactive as u64,
        }
    }

    fn unix_stream(c: &UnixStreamStateCount) -> UnixStreamCounts {
        UnixStreamCounts {
            free: c.free as u64,
            unconnected: c.unconnected as u64,
            connecting: c.connecting as u64,
            connected: c.connected as u64,
            disconnecting: c.disconnecting as u64,
        }
    }

    Ok(SocketsDto {
        ts: now_unix_ms(),
        tcp: tcp(&info.tcp),
        tcp6: tcp(&info.tcp6),
        unix_stream: unix_stream(&info.unix_stream),
        unix_dgram: info.unix_dgram as u64,
        unix_seq_packet: info.unix_seq_packet as u64,
        unknown: info.unknown as u64,
    })
}
```

- [ ] **Step 2.8: Unit-тесты на `is_secret_key`**

В `#[cfg(test)] mod tests` (collect.rs:373+) добавить:

```rust
    // ---------------------------------------------------------------------------
    // Secret-key masking
    // ---------------------------------------------------------------------------

    #[test]
    fn is_secret_key_top_level_password() {
        assert!(super::is_secret_key("password"));
        assert!(super::is_secret_key("admin_password"));
        assert!(super::is_secret_key("server_password"));
    }

    #[test]
    fn is_secret_key_top_level_secret() {
        assert!(super::is_secret_key("secret"));
        assert!(super::is_secret_key("talos_jwt_secret"));
    }

    #[test]
    fn is_secret_key_token_and_key_suffixes() {
        assert!(super::is_secret_key("api_token"));
        assert!(super::is_secret_key("private_key"));
    }

    #[test]
    fn is_secret_key_nested_password_path() {
        assert!(super::is_secret_key("pools.main.users.alice.password"));
        assert!(super::is_secret_key("users.app.api_token"));
    }

    #[test]
    fn is_secret_key_does_not_match_unrelated_keys() {
        assert!(!super::is_secret_key("host"));
        assert!(!super::is_secret_key("port"));
        assert!(!super::is_secret_key("connect_timeout"));
        assert!(!super::is_secret_key("pool_mode"));
        assert!(!super::is_secret_key("max_connections"));
    }

    #[test]
    fn is_secret_key_does_not_match_partial_substring() {
        // Substring "password" elsewhere in the key should not trigger masking.
        // Only exact equals or exact suffix counts.
        assert!(!super::is_secret_key("password_check_attempts"));
        assert!(!super::is_secret_key("not_a_secret_check"));
    }
```

- [ ] **Step 2.9: cargo build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib web::routes::collect:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 33 collect tests passing (27 baseline + 6 is_secret_key), clean.

- [ ] **Step 2.10: Не коммитить.**

---

## Task 3: handlers (один файл на endpoint)

**Files:**
- Create: `src/web/routes/{config,log_level,auth_query,pool_scaling,pool_coordinator,sockets}.rs`.
- Modify: `src/web/routes/mod.rs`.

- [ ] **Step 3.1: `config.rs`**

```rust
//! GET /api/config handler.

use crate::web::routes::collect::collect_config;
use crate::web::server::Response;

pub(crate) fn handle_config() -> Response {
    Response::ok_json(&collect_config())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_response_is_200_with_envelope() {
        let r = handle_config();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"config\""));
    }
}
```

- [ ] **Step 3.2: `log_level.rs`**

```rust
//! GET /api/log_level handler.

use crate::web::routes::collect::collect_log_level;
use crate::web::server::Response;

pub(crate) fn handle_log_level() -> Response {
    Response::ok_json(&collect_log_level())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_response_is_200_with_envelope() {
        let r = handle_log_level();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"log_level\""));
    }
}
```

- [ ] **Step 3.3: `auth_query.rs`**

```rust
//! GET /api/auth_query handler.

use crate::web::routes::collect::collect_auth_query;
use crate::web::server::Response;

pub(crate) fn handle_auth_query() -> Response {
    Response::ok_json(&collect_auth_query())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_query_response_is_200_with_envelope() {
        let r = handle_auth_query();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"pools\""));
    }
}
```

- [ ] **Step 3.4: `pool_scaling.rs`**

```rust
//! GET /api/pool_scaling handler.

use crate::web::routes::collect::collect_pool_scaling;
use crate::web::server::Response;

pub(crate) fn handle_pool_scaling() -> Response {
    Response::ok_json(&collect_pool_scaling())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_scaling_response_is_200_with_envelope() {
        let r = handle_pool_scaling();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"pools\""));
    }
}
```

- [ ] **Step 3.5: `pool_coordinator.rs`**

```rust
//! GET /api/pool_coordinator handler.

use crate::web::routes::collect::collect_pool_coordinator;
use crate::web::server::Response;

pub(crate) fn handle_pool_coordinator() -> Response {
    Response::ok_json(&collect_pool_coordinator())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_coordinator_response_is_200_with_envelope() {
        let r = handle_pool_coordinator();
        assert_eq!(r.status, 200);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("\"ts\""));
        assert!(body.contains("\"databases\""));
    }
}
```

- [ ] **Step 3.6: `sockets.rs`**

```rust
//! GET /api/sockets handler. Linux-only — non-linux returns 503 not_supported.

use crate::web::server::Response;

pub(crate) fn handle_sockets() -> Response {
    #[cfg(target_os = "linux")]
    {
        match crate::web::routes::collect::collect_sockets() {
            Ok(dto) => Response::ok_json(&dto),
            Err(msg) => {
                log::error!("collect_sockets failed: {msg}");
                Response::json(
                    500,
                    "Internal Server Error",
                    r#"{"error":"sockets_unavailable","message":"failed to read socket states"}"#,
                )
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Response::json(
            503,
            "Service Unavailable",
            r#"{"error":"not_supported","message":"sockets endpoint requires Linux"}"#,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn sockets_response_is_200_on_linux() {
        let r = handle_sockets();
        // Note: returns 500 if /proc/net/tcp* unreadable in CI sandbox; accept
        // both as long as the handler did not panic.
        assert!(r.status == 200 || r.status == 500, "got {}", r.status);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn sockets_response_is_503_on_non_linux() {
        let r = handle_sockets();
        assert_eq!(r.status, 503);
        let body = std::str::from_utf8(&r.body).unwrap();
        assert!(body.contains("not_supported"));
    }
}
```

- [ ] **Step 3.7: `mod.rs` — register modules**

В `src/web/routes/mod.rs` (alphabetical order):

```rust
pub mod collect;
pub mod dto;

pub(crate) mod auth_query;
pub(crate) mod clients;
pub(crate) mod config;
pub(crate) mod connections;
pub(crate) mod databases;
pub(crate) mod log_level;
pub(crate) mod overview;
pub(crate) mod pool_coordinator;
pub(crate) mod pool_scaling;
pub(crate) mod pools;
pub(crate) mod query;
pub(crate) mod servers;
pub(crate) mod sockets;
pub(crate) mod stats;
pub(crate) mod users;
pub(crate) mod version;
```

- [ ] **Step 3.8: cargo build + test + clippy + fmt**

```bash
cargo build --lib 2>&1 | tail -5
cargo test --lib web::routes:: 2>&1 | tail -15
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 6 handler-тестов passing (по одному на endpoint), clippy/fmt clean.

- [ ] **Step 3.9: Не коммитить.**

---

## Task 4: интеграция в mux + dispatch tests

**Files:**
- Modify: `src/web/server.rs`.

- [ ] **Step 4.1: Расширить `route_api`**

В `route_api` добавить 6 arm'ов (alphabetical):

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
        "/api/auth_query" => routes::auth_query::handle_auth_query(),
        "/api/config" => routes::config::handle_config(),
        "/api/log_level" => routes::log_level::handle_log_level(),
        "/api/pool_coordinator" => routes::pool_coordinator::handle_pool_coordinator(),
        "/api/pool_scaling" => routes::pool_scaling::handle_pool_scaling(),
        "/api/sockets" => routes::sockets::handle_sockets(),
        _ => Response::json(
            501,
            "Not Implemented",
            r#"{"error":"not_implemented","message":"endpoint will be wired in a later phase"}"#,
        ),
    }
```

- [ ] **Step 4.2: 6 dispatch-тестов**

В `mod tests` после `dispatch_users_returns_200`:

```rust
    #[test]
    fn dispatch_auth_query_returns_200() {
        let r = dispatch(
            &req("GET", "/api/auth_query"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_config_returns_200() {
        let r = dispatch(
            &req("GET", "/api/config"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_log_level_returns_200() {
        let r = dispatch(
            &req("GET", "/api/log_level"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_pool_coordinator_returns_200() {
        let r = dispatch(
            &req("GET", "/api/pool_coordinator"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn dispatch_pool_scaling_returns_200() {
        let r = dispatch(
            &req("GET", "/api/pool_scaling"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 200);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dispatch_sockets_returns_200_on_linux() {
        let r = dispatch(
            &req("GET", "/api/sockets"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        // 500 acceptable in sandbox; handler did not panic = pass.
        assert!(r.status == 200 || r.status == 500, "got {}", r.status);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn dispatch_sockets_returns_503_on_non_linux() {
        let r = dispatch(
            &req("GET", "/api/sockets"),
            &opts(true, true),
            AuthOutcome::Anonymous,
        );
        assert_eq!(r.status, 503);
    }
```

- [ ] **Step 4.3: Прогон тестов**

```bash
cargo test --lib web::server:: 2>&1 | tail -10
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 31 server-тест (25 baseline + 6 new), clippy/fmt clean.

- [ ] **Step 4.4: Не коммитить.**

---

## Task 5: integration tests

**Files:**
- Modify: `src/web/tests.rs`.

- [ ] **Step 5.1: Добавить 6 integration-тестов**

В конец `src/web/tests.rs`:

```rust
#[tokio::test]
async fn api_config_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(port, "GET /api/config HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"ts\""), "raw={raw}");
    assert!(raw.contains("\"config\""), "raw={raw}");
}

#[tokio::test]
async fn api_log_level_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/log_level HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"log_level\""), "raw={raw}");
}

#[tokio::test]
async fn api_auth_query_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/auth_query HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"pools\""), "raw={raw}");
}

#[tokio::test]
async fn api_pool_scaling_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/pool_scaling HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"pools\""), "raw={raw}");
}

#[tokio::test]
async fn api_pool_coordinator_returns_envelope() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/pool_coordinator HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "raw={raw}");
    assert!(raw.contains("\"databases\""), "raw={raw}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn api_sockets_returns_200_or_500_on_linux() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/sockets HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK") || raw.starts_with("HTTP/1.1 500"),
        "raw={raw}"
    );
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn api_sockets_returns_503_on_non_linux() {
    let port = spawn_server(opts(true, true)).await;
    let raw = send(
        port,
        "GET /api/sockets HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 503"), "raw={raw}");
}
```

- [ ] **Step 5.2: Полный прогон**

```bash
cargo test --lib 2>&1 | tail -3
cargo clippy --lib -- --deny warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: ~754 passed (730 baseline + 6 is_secret_key + 6 handler unit + 6 dispatch + 6 integration). Точное число подтверждаем.

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
./target/release/pg_doorman /tmp/doorman-phase3a.toml > /tmp/doorman-phase3c2.log 2>&1 &
DPID=$!
sleep 2
echo "--- /api/config ---"
curl -s 'http://127.0.0.1:19127/api/config' | head -c 600
echo ""
echo "--- /api/log_level ---"
curl -s 'http://127.0.0.1:19127/api/log_level' | head -c 200
echo ""
echo "--- /api/auth_query ---"
curl -s 'http://127.0.0.1:19127/api/auth_query' | head -c 400
echo ""
echo "--- /api/pool_scaling ---"
curl -s 'http://127.0.0.1:19127/api/pool_scaling' | head -c 400
echo ""
echo "--- /api/pool_coordinator ---"
curl -s 'http://127.0.0.1:19127/api/pool_coordinator' | head -c 400
echo ""
echo "--- /api/sockets ---"
curl -s 'http://127.0.0.1:19127/api/sockets' | head -c 600
echo ""
kill $DPID 2>/dev/null
wait $DPID 2>/dev/null
```
Expected: каждый endpoint отвечает `200` (или `503` для sockets на non-linux), валидный JSON.

- [ ] **Step 6.3: Pre-commit code review**

Через Agent (правило CLAUDE.md). Черновик commit-сообщения:

```
feat(web): land /api/config /api/log_level /api/auth_query /api/pool_scaling /api/pool_coordinator /api/sockets

ConfigState page in the upcoming Web UI now has the rest of its data
sources: the active configuration (with secret values redacted), the
runtime log filter, the auth_query cache stats, the anticipation/burst
gate counters per pool, the per-database coordinator limits, and on
Linux the TCP/Unix socket-state breakdown.

Field names mirror the corresponding admin SHOW commands one-to-one.
The /api/sockets endpoint stays at parity with the existing platform
gate: Linux returns the counters, other operating systems return
503 not_supported.

Secret-value masking for /api/config is implemented as a pure helper
that redacts any key whose trailing path segment is exactly "password"
or "secret", or ends with _password / _secret / _token / _key. The
flat config representation today omits per-user passwords and
admin_password — that is a long-standing limitation of the existing
SHOW CONFIG conversion; when the conversion is extended in a future
PR the masker will pick the new keys up automatically.

Tests: <N> passed (was 730), clippy and fmt clean. Verified by release
binary smoke-tests against all six endpoints.

Phase 3c-2 of seven; phase 3c-3 lands /api/prepared, /api/interner and
the admin-only stubs prepared/text/{hash} and interner/top.
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
- ✅ `/api/config` shape (раздел 8.2 «Ключ-значение, секреты маскированы») — Tasks 1, 2, 3 + masker test.
- ✅ `/api/log_level` shape (раздел 8.2 «Текущий filter (RUST_LOG-формат)») — Tasks 1, 2, 3.
- ✅ `/api/auth_query` shape (раздел 8.2 «Auth-query cache stats per pool») — Tasks 1, 2, 3.
- ✅ `/api/pool_scaling` shape (раздел 8.2 «Anticipation/burst-gate counters per pool») — Tasks 1, 2, 3.
- ✅ `/api/pool_coordinator` shape (раздел 8.2 «Coordinator limits/usage per database») — Tasks 1, 2, 3.
- ✅ `/api/sockets` linux-only (раздел 8.2) — Tasks 1, 2, 3 с `#[cfg(target_os = "linux")]` и 503 на non-linux.
- ✅ Public access — handlers без auth-проверок (mux обрабатывает `ui_anonymous`).

**Не покрыто этой фазой:**
- `/api/prepared`, `/api/interner`, admin stubs `/api/prepared/text/{hash}` и `/api/interner/top` — phase 3c-3.
- Расширение `From<&Config> for HashMap<String, String>` для покрытия всех секретов — отдельный PR.
- Frontend ConfigState page — phase 6.

**Type-consistency check:**
- Все `*Dto` поля — `String`, `u64`, `&'static str`, вложенные структы (для sockets).
- Sort order detereministic — `config` по `key`, `auth_query` по `database`, `pool_scaling` по `(database, user)`, `pool_coordinator` по `database`.

**Placeholder check:** Нет полей со заглушкой `0` или TODO. Все значения из реальных backend источников. Единственный fixed string — `default: "-"` в `ConfigEntry`, что зеркалит существующее `show_config` поведение и документировано.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-3c-2.md`.
Subagent-driven execution; controller dispatches one implementer per task plus two-stage review.
