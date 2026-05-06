//! JSON DTO types for the Web UI REST API.
//!
//! These structs define the wire format that the frontend consumes; they are
//! the source of truth for response shapes documented in spec sections 8.3+.
//! Field naming follows the spec exactly. Per-handler unit tests assert that
//! every required JSON key is present in the serialized output; full snapshot
//! tests are a candidate follow-up.

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
    pub wait_p95_ms: u64,

    pub queries_total: u64,
    pub transactions_total: u64,
    pub errors_total: u64,

    pub paused: bool,
    pub epoch: u64,
}

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
    pub client_id: String,
    pub database: String,
    pub user: String,
    pub application_name: String,
    pub addr: String,
    pub tls: bool,
    pub state: String,
    pub wait: String,
    pub wait_ms: u64,
    pub transactions_total: u64,
    pub queries_total: u64,
    pub errors_total: u64,
    pub age_seconds: u64,
    pub current_query_age_ms: u64,
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
    pub active_age_ms: u64,
    pub transactions_total: u64,
    pub queries_total: u64,
    pub errors_total: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub prepared_hits_total: u64,
    pub prepared_misses_total: u64,
    pub prepared_cache_size: u64,
}

// Filter structs are NOT serialized; they're internal request DTOs.

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
    /// Always 0. `SHOW DATABASES` hardcodes 0 for this column even though
    /// pg_doorman does honour `reserve_pool_size` in the connection pool
    /// itself; the REST API mirrors the admin protocol's shape. For the
    /// configured value use `SHOW POOLS` or the
    /// `pg_doorman_pool_size{type="reserve_pool_size"}` Prometheus gauge.
    pub reserve_pool: u32,
    pub pool_mode: String,
    pub max_connections: u32,
    pub current_connections: u32,
}

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

/// `GET /api/log_level` — the active log filter (RUST_LOG-style).
#[derive(Debug, Serialize)]
pub struct LogLevelDto {
    pub ts: u64,
    pub log_level: String,
}

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
