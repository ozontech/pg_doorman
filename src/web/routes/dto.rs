//! JSON DTO types for the Web UI REST API.
//!
//! These structs define the wire format that the frontend consumes; they are
//! the source of truth for response shapes documented in spec sections 8.3+.
//! Field naming follows the spec exactly. Per-handler unit tests assert that
//! every required JSON key is present in the serialized output; full snapshot
//! tests are a candidate follow-up.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub(crate) struct VersionDto {
    pub version: &'static str,
    pub git_commit: &'static str,
    pub build_date: &'static str,
    pub ts: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct OverviewDto {
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

    /// Database currently holding the most live backend connections,
    /// summed across every `<user>@<db>` pool that targets it. Surfaces
    /// in the SPA sidebar so an operator opening any page sees which
    /// database is taking the load right now. Omitted when no pool has
    /// a live backend connection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hottest_database: Option<HottestDatabaseDto>,

    /// Process resident-set size in bytes, sampled at request time. Linux
    /// reads `/proc/self/statm`; macOS shells out to `ps`. Provides the
    /// "is the pooler leaking memory" tile without requiring Prometheus.
    pub rss_bytes: u64,
    /// Seconds since the binary started (`STARTED_AT` lazy in app/server.rs).
    pub uptime_seconds: u64,
    /// OS process id. Useful when correlating with external tools (`htop`,
    /// `lsof`, `gdb`) on the same host.
    pub pid: u32,
    /// Number of clients currently connected to the pooler. Mirrors
    /// `CURRENT_CLIENT_COUNT` in app/server.rs; not derivable from the per-
    /// pool counts because clients do not always belong to a pool yet (e.g.
    /// during startup negotiation).
    pub current_clients: i64,
    /// Number of clients currently inside an open PG transaction holding
    /// a backend connection. Mirrors `CLIENTS_IN_TRANSACTIONS`.
    pub clients_in_transactions: i64,
    /// Set during `SIGTERM`/admin SHUTDOWN. Operator-visible "the pooler is
    /// draining, do not deploy now" indicator.
    pub shutdown_in_progress: bool,
    /// Set during binary upgrade — clients are migrating to the new process.
    pub migration_in_progress: bool,
}

/// Database that holds the most live backend connections at the moment
/// `collect_overview` runs. `total_connections` sums every state — active,
/// idle, in-use, login — so it matches `connections` on the per-pool
/// table. `active_connections` is the subset currently executing a query
/// or carrying a transaction.
#[derive(Debug, Serialize)]
pub(crate) struct HottestDatabaseDto {
    pub name: String,
    pub total_connections: u64,
    pub active_connections: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct PoolsDto {
    pub ts: u64,
    pub pools: Vec<PoolDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PoolDto {
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

    /// Latency percentiles in milliseconds. Stored as `f64` rather than
    /// `u64` so sub-millisecond values survive: a workload whose true
    /// p95 is 420 µs would otherwise integer-divide to `0 ms` in the
    /// DTO and render as a bogus zero on the dashboard while the log
    /// line correctly showed `query_ms p95 = 0.42`.
    pub query_p95_ms: f64,
    pub query_p99_ms: f64,
    pub transactions_p95_ms: f64,
    pub transactions_p99_ms: f64,

    pub wait_avg_ms: f64,
    pub wait_p95_ms: f64,

    pub queries_total: u64,
    pub transactions_total: u64,
    pub errors_total: u64,
    /// Cumulative error breakdown keyed by PostgreSQL SQLSTATE. Includes
    /// both PG-side ErrorResponse codes and pg_doorman-side codes such as
    /// `53300` raised on checkout failure. Omitted from the JSON when no
    /// errors have been classified yet.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub errors_by_sqlstate: HashMap<String, u64>,

    pub paused: bool,
    pub epoch: u64,

    /// Patroni-assisted fallback flag. Mirrors the `pg_doorman_fallback_active`
    /// gauge — `true` when the local backend is in cooldown and the pool is
    /// routing through a fallback host discovered via Patroni `/cluster`.
    pub fallback_active: bool,
    /// Cumulative count of TLS handshake errors against the backend for this
    /// pool. Mirrors `pg_doorman_server_tls_handshake_errors_total`.
    pub tls_handshake_errors_total: u64,
    /// Live TLS-encrypted backend connections held by the pool. Mirrors
    /// `pg_doorman_server_tls_connections`.
    pub tls_backend_connections: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct ClientsDto {
    pub ts: u64,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
    pub clients: Vec<ClientDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ClientDto {
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
pub(crate) struct ServersDto {
    pub ts: u64,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
    pub servers: Vec<ServerDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ServerDto {
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
pub(crate) struct ClientFilters {
    pub limit: u64,
    pub offset: u64,
    pub sort: ClientSort,
    pub order: SortOrder,
    pub pool: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
    /// Substring match against `ClientStats.addr` (e.g. "10.0.5." for a subnet
    /// or "1.2.3.4:5432" for an exact peer).
    pub addr: Option<String>,
    pub application_name: Vec<String>,
    pub state: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum ClientSort {
    #[default]
    QueriesTotal,
    ErrorsTotal,
    AgeSeconds,
    CurrentQueryAgeMs,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum SortOrder {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ServerFilters {
    pub limit: u64,
    pub offset: u64,
    pub sort: ServerSort,
    pub order: SortOrder,
    pub pool: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum ServerSort {
    #[default]
    AgeSeconds,
    QueriesTotal,
    ErrorsTotal,
    ActiveAgeMs,
}

/// `GET /api/process` — process resource snapshot. Linux reads `/proc/self/*`
/// directly; non-Linux platforms return zeros where information is not
/// available without extra dependencies. CPU usage is provided as
/// monotonic microsecond counters (user + system, total + per thread); the
/// frontend computes `%` by sampling deltas across two consecutive polls.
#[derive(Debug, Serialize)]
pub(crate) struct ProcessDto {
    pub ts: u64,
    pub pid: u32,
    pub hostname: String,
    pub uptime_seconds: u64,
    pub started_at_ms: u64,
    pub rss_bytes: u64,
    pub vm_size_bytes: u64,
    pub threads: u64,
    pub fd_open: u64,
    pub fd_limit: u64,
    /// Cumulative user-mode CPU time across the whole process, microseconds.
    pub cpu_user_us: u64,
    /// Cumulative kernel-mode CPU time across the whole process, microseconds.
    pub cpu_system_us: u64,
    /// Number of online CPU cores (`num_cpus::get`). Frontend uses this to
    /// turn the cumulative deltas into a percentage of one core or of all
    /// cores depending on the operator's preference.
    pub cpu_cores: u32,
    /// Per-thread CPU breakdown. Sorted by `cpu_user_us + cpu_system_us`
    /// descending so the hottest tokio worker is at the top. Linux only;
    /// other platforms return an empty list.
    pub threads_breakdown: Vec<ProcessThreadDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProcessThreadDto {
    pub tid: u64,
    /// Comm field from `/proc/self/task/<tid>/stat` — limited to 15 chars by
    /// the kernel. Names like `tokio-runtime-w`, `pg_doorman` for the main
    /// thread, etc.
    pub name: String,
    pub cpu_user_us: u64,
    pub cpu_system_us: u64,
}

/// `GET /api/process/memory` — memory breakdown for the RSS panel.
/// Linux fills every field; macOS / others return what they can and
/// leave Linux-only fields `None`.
#[derive(Debug, Serialize)]
pub(crate) struct MemoryBreakdownDto {
    pub ts: u64,
    pub rss_bytes: u64,
    /// Fields harvested from `/proc/self/status` in one pass. `None` on
    /// non-Linux.
    pub vm_peak_bytes: Option<u64>,
    pub vm_hwm_bytes: Option<u64>,
    pub vm_data_bytes: Option<u64>,
    pub vm_stack_bytes: Option<u64>,
    pub vm_exe_bytes: Option<u64>,
    pub vm_lib_bytes: Option<u64>,
    pub vm_pte_bytes: Option<u64>,
    pub vm_swap_bytes: Option<u64>,
    pub rss_anon_bytes: Option<u64>,
    pub rss_file_bytes: Option<u64>,
    pub rss_shmem_bytes: Option<u64>,
    /// jemalloc accounting (the global allocator pg_doorman links).
    /// `None` only if the ctl call failed (should never happen at runtime).
    pub jemalloc: Option<JemallocStatsDto>,
    /// Container memory limits and current usage. `None` on non-Linux or
    /// when the cgroup files are not readable (chroot, custom mounts).
    pub cgroup: Option<CgroupMemoryDto>,
    /// pg_doorman-internal accountable bytes — the SQL interner cache and
    /// the prepared-statement cache. Operators look here first when RSS
    /// climbs.
    pub interner_named_bytes: u64,
    pub interner_anonymous_bytes: u64,
    /// Operator-facing rollup categories. Each maps to a `MemoryCategoryDto`
    /// with a stable `key` so the frontend can paint a stacked bar without
    /// hard-coding category names.
    pub categories: Vec<MemoryCategoryDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct JemallocStatsDto {
    pub allocated_bytes: u64,
    pub active_bytes: u64,
    pub resident_bytes: u64,
    pub mapped_bytes: u64,
    pub retained_bytes: u64,
    pub metadata_bytes: u64,
    /// `resident − allocated`. Pages jemalloc holds but is not currently
    /// using; reclaimable on demand.
    pub fragmentation_bytes: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct CgroupMemoryDto {
    /// 1 for cgroup v1, 2 for cgroup v2 unified hierarchy.
    pub version: u8,
    pub current_bytes: u64,
    /// On cgroup v2: `memory.peak` (kernels ≥ 5.19); `None` otherwise.
    /// On cgroup v1: historical maximum from `memory.max_usage_in_bytes`.
    pub peak_bytes: Option<u64>,
    /// `None` when the limit is "max" (uncapped).
    pub max_bytes: Option<u64>,
    /// `None` on cgroup v1.
    pub high_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct MemoryCategoryDto {
    pub key: &'static str,
    pub label: &'static str,
    pub bytes: u64,
    pub explain: &'static str,
}

/// `GET /api/connections` — cumulative connection counters.
///
/// `errors` is derived as `total - tls - plain - cancel` to mirror the
/// existing `SHOW CONNECTIONS` admin output exactly. Operators reading the
/// REST API see the same values they saw via the admin protocol.
#[derive(Debug, Serialize)]
pub(crate) struct ConnectionsDto {
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
pub(crate) struct StatsDto {
    pub ts: u64,
    pub stats: Vec<StatsRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct StatsRowDto {
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
pub(crate) struct DatabasesDto {
    pub ts: u64,
    pub databases: Vec<DatabaseDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DatabaseDto {
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
pub(crate) struct UsersDto {
    pub ts: u64,
    pub users: Vec<UserDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct UserDto {
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
pub(crate) struct ConfigDto {
    pub ts: u64,
    pub config: Vec<ConfigEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConfigEntry {
    pub key: String,
    pub value: String,
    /// Built-in default (computed by serializing `Config::default()`).
    /// `"-"` when the field has no representation in the default config
    /// (e.g. user-defined pools).
    pub default: String,
    /// `"yes"` for keys that take effect on `RELOAD`, `"no"` for keys that
    /// require a restart. Mirrors the `immutables` list inside `show_config`.
    pub changeable: &'static str,
    /// EN-language description sourced from `fields.yaml`. Empty for
    /// fields without a documented surface (operator-defined sections,
    /// internal bookkeeping). Operators see this as the per-row tooltip.
    pub doc: String,
}

/// `GET /api/log_level` — the active log filter (RUST_LOG-style).
#[derive(Debug, Serialize)]
pub(crate) struct LogLevelDto {
    pub ts: u64,
    pub log_level: String,
}

/// `GET /api/auth_query` — per-pool auth_query cache and authentication
/// metrics. Field names mirror `SHOW AUTH_QUERY` columns.
#[derive(Debug, Serialize)]
pub(crate) struct AuthQueryDto {
    pub ts: u64,
    pub pools: Vec<AuthQueryRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AuthQueryRowDto {
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
pub(crate) struct PoolScalingDto {
    pub ts: u64,
    pub pools: Vec<PoolScalingRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PoolScalingRowDto {
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
pub(crate) struct PoolCoordinatorDto {
    pub ts: u64,
    pub databases: Vec<PoolCoordinatorRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PoolCoordinatorRowDto {
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
pub(crate) struct SocketsDto {
    pub ts: u64,
    pub tcp: TcpCounts,
    pub tcp6: TcpCounts,
    pub unix_stream: UnixStreamCounts,
    pub unix_dgram: u64,
    pub unix_seq_packet: u64,
    pub unknown: u64,
}

#[derive(Debug, Serialize, Default)]
pub(crate) struct TcpCounts {
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
pub(crate) struct UnixStreamCounts {
    pub free: u64,
    pub unconnected: u64,
    pub connecting: u64,
    pub connected: u64,
    pub disconnecting: u64,
}

/// `GET /api/prepared` — aggregate of pool-level prepared-statement caches.
///
/// Public endpoint. The `query` text is intentionally NOT included here to
/// avoid leaking SQL bodies to anonymous Web UI viewers; the admin-only
/// `/api/prepared/text/{hash}` endpoint returns the text on demand.
#[derive(Debug, Serialize)]
pub(crate) struct PreparedDto {
    pub ts: u64,
    pub prepared: Vec<PreparedRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PreparedRowDto {
    /// Pool identifier in the form rendered by `PoolIdentifier::Display`.
    pub pool: String,
    /// 64-bit FxHash, formatted as decimal to mirror SHOW PREPARED STATEMENTS.
    pub hash: String,
    pub name: String,
    pub count_used: u64,
    /// Cumulative Parse-time hits — server already had this prepared statement
    /// when the client asked. Per-pool, per-CacheEntry. Lost on LRU eviction.
    pub hits: u64,
    /// Cumulative Parse-time misses — server lacked this prepared statement,
    /// requiring a fresh Parse to PostgreSQL. Per-pool, per-CacheEntry.
    pub misses: u64,
    /// One of "named", "anonymous", "mixed" — `CacheEntryKind::as_str`.
    pub kind: String,
}

/// `GET /api/interner` — global query interner aggregate.
/// Public; no SQL preview.
#[derive(Debug, Serialize)]
pub(crate) struct InternerDto {
    pub ts: u64,
    pub named: InternerKindDto,
    pub anonymous: InternerKindDto,
}

#[derive(Debug, Serialize)]
pub(crate) struct InternerKindDto {
    pub entries: u64,
    pub bytes: u64,
}

/// `GET /api/interner/top?n=N` — admin-only Top-N interner entries by
/// interned-text byte length, with a 120-character SQL preview.
#[derive(Debug, Serialize)]
pub(crate) struct InternerTopDto {
    pub ts: u64,
    /// The clamped value of `n` actually used (1..=MAX).
    pub n: u64,
    pub entries: Vec<InternerTopRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct InternerTopRowDto {
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

/// `GET /api/top/clients` — Top-N clients by qps / errors / age.
#[derive(Debug, Serialize)]
pub(crate) struct TopClientsDto {
    pub ts: u64,
    /// The sort dimension actually used: `"qps"`, `"errors"`, `"age"`.
    pub by: String,
    /// The clamped value of `n` actually used (1..=200; default 20).
    pub n: u64,
    pub clients: Vec<TopClientRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TopClientRowDto {
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
pub(crate) enum TopClientBy {
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
pub(crate) struct TopClientFilters {
    pub by: TopClientBy,
    pub n: u64,
    pub pool: Option<String>,
}

/// `GET /api/apps` — per-application_name aggregate of client counters.
#[derive(Debug, Serialize)]
pub(crate) struct AppsDto {
    pub ts: u64,
    pub apps: Vec<AppRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AppRowDto {
    pub application_name: String,
    /// Number of currently-connected clients reporting this application_name.
    pub clients: u64,
    /// Cumulative counters; frontend computes rates from successive snapshots.
    pub queries_total: u64,
    pub transactions_total: u64,
    pub errors_total: u64,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum AppSort {
    #[default]
    Clients,
    Queries,
    Transactions,
    Errors,
}

impl AppSort {
    #[allow(dead_code)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            AppSort::Clients => "clients",
            AppSort::Queries => "queries",
            AppSort::Transactions => "transactions",
            AppSort::Errors => "errors",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct AppFilters {
    pub sort: AppSort,
    pub order: SortOrder,
}

/// `GET /api/top/queries` — Top-N interner-tracked queries by count or
/// average duration. See plan for accuracy notes (Bind-counted, batch-
/// level duration attribution).
#[derive(Debug, Serialize)]
pub(crate) struct TopQueriesDto {
    pub ts: u64,
    pub by: String,
    pub n: u64,
    pub queries: Vec<TopQueryRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TopQueryRowDto {
    /// `0x<hex>` form of the FxHash, matching `/api/interner/top`.
    pub hash: String,
    /// `"named"` or `"anonymous"`.
    pub kind: String,
    /// First 120 characters of the interned text (UTF-8 safe).
    pub query: String,
    pub count: u64,
    pub total_duration_us: u64,
    /// Average duration in milliseconds: `total_duration_us / count / 1000`.
    /// Returns `0.0` when count is 0 (entry interned but never Bound).
    pub avg_duration_ms: f64,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum TopQueryBy {
    #[default]
    Count,
    Duration,
}

impl TopQueryBy {
    pub fn as_str(self) -> &'static str {
        match self {
            TopQueryBy::Count => "count",
            TopQueryBy::Duration => "duration",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct TopQueryFilters {
    pub by: TopQueryBy,
    pub n: u64,
}

/// `GET /api/events?since=<seq>&max=<N>` — admin command timeline used
/// for vertical-line annotations on the Overview graphs. Bounded ring
/// buffer; oldest events drop silently when full.
#[derive(Debug, Serialize)]
pub(crate) struct EventsDto {
    pub ts: u64,
    /// Sequence number to poll with on the next request to receive only
    /// events newer than this batch. Equal to `since` when nothing new.
    pub next_seq: u64,
    pub events: Vec<EventEntryDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EventEntryDto {
    pub seq: u64,
    pub ts_ms: u64,
    /// One of `"RELOAD"`, `"PAUSE"`, `"RESUME"`, `"RECONNECT"`.
    pub target: String,
    pub message: String,
}

/// `GET /api/prepared/text/{hash}` — admin-only body of a single prepared
/// statement. Returns 404 when the hash is not present in any pool's cache.
#[derive(Debug, Serialize)]
pub(crate) struct PreparedTextDto {
    pub ts: u64,
    pub hash: String,
    pub pool: String,
    pub name: String,
    pub query: String,
    pub kind: String,
}

/// `GET /api/top/prepared?by=hits|misses&n=20` — Top-N prepared statements
/// across all pools, sorted by cumulative hit or miss count. Public; no SQL
/// preview — for the body use admin-only `/api/prepared/text/{hash}`.
#[derive(Debug, Serialize)]
pub(crate) struct TopPreparedDto {
    pub ts: u64,
    pub by: String,
    pub n: u64,
    pub prepared: Vec<TopPreparedRowDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TopPreparedRowDto {
    pub pool: String,
    pub hash: String,
    pub name: String,
    pub count_used: u64,
    pub hits: u64,
    pub misses: u64,
    pub kind: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) enum TopPreparedBy {
    #[default]
    Hits,
    Misses,
}

impl TopPreparedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            TopPreparedBy::Hits => "hits",
            TopPreparedBy::Misses => "misses",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct TopPreparedFilters {
    pub by: TopPreparedBy,
    pub n: u64,
}

/// `GET /api/logs?since=&max=&level=&target=` — admin-only live tail
/// over the in-memory LogTap ring (spec section 8.6 + 9).
#[derive(Debug, Serialize)]
pub(crate) struct LogsDto {
    pub ts: u64,
    pub tap_active: bool,
    pub tap_capacity_entries: u64,
    pub tap_used_entries: u64,
    /// Sequence number to poll with on the next request.
    pub next_seq: u64,
    /// Records lost from the ring before `since` (consumer evicted older
    /// entries because the buffer is full). Operator falling behind sees
    /// this grow.
    pub dropped_before: u64,
    /// Cumulative drops since the tap was activated. Includes evict-drops
    /// (consumer ring overflow) and burst-drops (producer try_send full).
    pub dropped_total: u64,
    pub entries: Vec<LogEntryDto>,
}

#[derive(Debug, Serialize)]
pub(crate) struct LogEntryDto {
    pub seq: u64,
    pub ts_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}
