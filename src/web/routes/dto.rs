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
