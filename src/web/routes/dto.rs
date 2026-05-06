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
