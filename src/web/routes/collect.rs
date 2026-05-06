//! Pure collection functions for the REST API.
//!
//! Each function reads from project-wide global state (POOLS,
//! get_client_stats(), get_server_stats(), connection counters) and assembles
//! a serializable DTO. Locking is limited to brief Mutex acquisitions for
//! fields that lack a lock-free getter (server application_name).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::log_level;
use crate::config::get_config;
use crate::pool::{get_all_pools, AUTH_QUERY_STATE, COORDINATORS, DYNAMIC_POOLS};
use crate::server::{anon_snapshot, named_snapshot, now_monotonic_ms};
use crate::stats::pool::PoolStats;
use crate::stats::{
    get_client_stats, get_server_stats, CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER,
    TLS_CONNECTION_COUNTER, TOTAL_CONNECTION_COUNTER,
};
use crate::web::routes::dto::{
    AppFilters, AppRowDto, AppSort, AppsDto, AuthQueryDto, AuthQueryRowDto, ClientDto,
    ClientFilters, ClientSort, ClientsDto, ConfigDto, ConfigEntry, ConnectionsDto, DatabaseDto,
    DatabasesDto, InternerDto, InternerKindDto, InternerTopDto, InternerTopRowDto, LogLevelDto,
    OverviewDto, PoolCoordinatorDto, PoolCoordinatorRowDto, PoolDto, PoolScalingDto,
    PoolScalingRowDto, PoolsDto, PreparedDto, PreparedRowDto, PreparedTextDto, ServerDto,
    ServerFilters, ServerSort, ServersDto, SortOrder, StatsDto, StatsRowDto, TopClientBy,
    TopClientFilters, TopClientRowDto, TopClientsDto, UserDto, UsersDto, VersionDto,
};

#[cfg(target_os = "linux")]
use crate::web::routes::dto::{SocketsDto, TcpCounts, UnixStreamCounts};

fn cnt(counter: &AtomicUsize) -> u64 {
    counter.load(Ordering::Relaxed) as u64
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn collect_version() -> VersionDto {
    VersionDto {
        version: env!("CARGO_PKG_VERSION"),
        git_commit: option_env!("PG_DOORMAN_GIT_COMMIT").unwrap_or("unknown"),
        build_date: option_env!("PG_DOORMAN_BUILD_DATE").unwrap_or("unknown"),
        ts: now_unix_ms(),
    }
}

pub fn collect_overview() -> OverviewDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let client_states = get_client_stats();
    let server_states = get_server_stats();

    let mut active_clients = 0u64;
    let mut idle_clients = 0u64;
    let mut waiting_clients = 0u64;
    for stats in client_states.values() {
        match stats.state_to_string().as_str() {
            "active" => active_clients += 1,
            "idle" => idle_clients += 1,
            "waiting" => waiting_clients += 1,
            _ => {}
        }
    }

    let mut active_servers = 0u64;
    let mut idle_servers = 0u64;
    for stats in server_states.values() {
        match stats.state_to_string().as_str() {
            "active" => active_servers += 1,
            "idle" => idle_servers += 1,
            _ => {}
        }
    }

    let connections_total = cnt(&TOTAL_CONNECTION_COUNTER);
    let connections_tls_total = cnt(&TLS_CONNECTION_COUNTER);
    let connections_plain_total = cnt(&PLAIN_CONNECTION_COUNTER);
    let connections_cancel_total = cnt(&CANCEL_CONNECTION_COUNTER);

    let mut query_count_total = 0u64;
    let mut transaction_count_total = 0u64;
    let mut prepared_hits_total = 0u64;
    let mut prepared_misses_total = 0u64;
    let mut pools_paused = 0u64;
    for stats in pool_lookup.values() {
        query_count_total += stats.total_query_count;
        transaction_count_total += stats.total_xact_count;
        if stats.paused {
            pools_paused += 1;
        }
    }
    for stats in server_states.values() {
        prepared_hits_total += stats.prepared_hit_count.load(Ordering::Relaxed);
        prepared_misses_total += stats.prepared_miss_count.load(Ordering::Relaxed);
    }

    OverviewDto {
        ts: now_unix_ms(),

        active_clients,
        idle_clients,
        waiting_clients,

        active_servers,
        idle_servers,

        connections_total,
        connections_tls_total,
        connections_plain_total,
        connections_cancel_total,

        query_count_total,
        transaction_count_total,
        // Sum of per-pool error counters. Already populated by PoolStats.errors.
        errors_count_total: pool_lookup.values().map(|s| s.errors).sum(),

        prepared_hits_total,
        prepared_misses_total,

        pools_total: pool_lookup.len() as u64,
        pools_paused,
    }
}

pub fn collect_pools() -> PoolsDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let pools_map = get_all_pools();

    let mut pools = Vec::with_capacity(pool_lookup.len());
    for (identifier, stats) in pool_lookup.iter() {
        let Some(pool) = pools_map.get(identifier) else {
            continue;
        };
        let address = pool.address();
        let dto = PoolDto {
            id: format!("{}@{}", identifier.user, identifier.db),
            user: identifier.user.clone(),
            database: identifier.db.clone(),
            host: address.host.clone(),
            port: address.port,
            pool_mode: stats.mode.to_string(),
            max_connections: stats.pool_size,
            // min_pool_size lives on the per-user config; PoolSettings wraps it via settings.user.
            min_connections: pool.settings.user.min_pool_size.unwrap_or(0),
            connections: stats.sv_active + stats.sv_idle + stats.sv_used + stats.sv_login,
            idle: stats.sv_idle,
            active: stats.sv_active,
            waiting: stats.cl_waiting,
            max_active_age_ms: stats.oldest_active_age_ms,
            // Percentile fields are plain u64, not methods.
            query_p95_ms: stats.query_percentile.p95,
            query_p99_ms: stats.query_percentile.p99,
            transactions_p95_ms: stats.xact_percentile.p95,
            transactions_p99_ms: stats.xact_percentile.p99,
            wait_avg_ms: stats.avg_wait_time / 1_000, // micros -> ms
            wait_p95_ms: stats.wait_percentile.p95 / 1_000, // micros -> ms
            queries_total: stats.total_query_count,
            transactions_total: stats.total_xact_count,
            errors_total: stats.errors,
            paused: stats.paused,
            // TODO: epoch wiring in phase 3e (no epoch field on PoolSettings yet).
            epoch: 0,
        };
        pools.push(dto);
    }

    // Stable order for snapshot tests.
    pools.sort_by(|a, b| a.id.cmp(&b.id));

    PoolsDto {
        ts: now_unix_ms(),
        pools,
    }
}

// MAX_LIMIT capped at 1000 rows because at typical pooler scale (few thousand
// clients) this is enough for first-page UX; increase if operator feedback
// demands it.
const MAX_LIMIT: u64 = 1000;

pub fn collect_clients(filters: &ClientFilters) -> ClientsDto {
    let snapshot: Vec<_> = get_client_stats().values().cloned().collect();
    collect_clients_from(snapshot, filters)
}

/// Pure inner logic for `collect_clients` — operates on a pre-built snapshot
/// so it can be called from unit tests without touching global state.
fn collect_clients_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ClientStats>>,
    filters: &ClientFilters,
) -> ClientsDto {
    let mut rows: Vec<ClientDto> = snapshot
        .iter()
        .filter(|s| client_matches(s, filters))
        .map(client_to_dto)
        .collect();

    let total = rows.len() as u64;

    rows.sort_by(|a, b| {
        let ord = match filters.sort {
            ClientSort::QueriesTotal => a.queries_total.cmp(&b.queries_total),
            ClientSort::ErrorsTotal => a.errors_total.cmp(&b.errors_total),
            ClientSort::AgeSeconds => a.age_seconds.cmp(&b.age_seconds),
            ClientSort::CurrentQueryAgeMs => a.current_query_age_ms.cmp(&b.current_query_age_ms),
        };
        match filters.order {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        }
    });

    let limit = filters.limit.clamp(1, MAX_LIMIT);
    let offset = filters.offset;
    let page: Vec<_> = rows
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    ClientsDto {
        ts: now_unix_ms(),
        total,
        limit,
        offset,
        clients: page,
    }
}

fn client_matches(s: &crate::stats::ClientStats, f: &ClientFilters) -> bool {
    let pool_name = s.pool_name();
    let user = s.username();
    let app = s.application_name();
    let state = s.state_to_string();

    if let Some(p) = &f.pool {
        let id = format!("{}@{}", user, pool_name);
        if id != *p {
            return false;
        }
    }
    if let Some(db) = &f.database {
        if pool_name != *db {
            return false;
        }
    }
    if let Some(u) = &f.user {
        if user != *u {
            return false;
        }
    }
    if !f.application_name.is_empty() && !f.application_name.contains(&app) {
        return false;
    }
    if !f.state.is_empty() && !f.state.contains(&state) {
        return false;
    }
    true
}

fn client_to_dto(s: &std::sync::Arc<crate::stats::ClientStats>) -> ClientDto {
    let age_seconds = s.connect_time().elapsed().as_secs();
    ClientDto {
        client_id: format!("#c{}", s.connection_id()),
        database: s.pool_name(),
        user: s.username(),
        application_name: s.application_name(),
        addr: s.ipaddr(),
        tls: s.tls(),
        state: s.state_to_string(),
        wait: s.wait_to_string(),
        wait_ms: s.wait_ms().unwrap_or(0),
        transactions_total: s.transaction_count.load(Ordering::Relaxed),
        queries_total: s.query_count.load(Ordering::Relaxed),
        errors_total: s.error_count.load(Ordering::Relaxed),
        age_seconds,
        current_query_age_ms: s.current_query_age_ms().unwrap_or(0),
    }
}

pub fn collect_servers(filters: &ServerFilters) -> ServersDto {
    let snapshot: Vec<_> = get_server_stats().values().cloned().collect();
    collect_servers_from(snapshot, filters)
}

/// Pure inner logic for `collect_servers` — operates on a pre-built snapshot
/// so it can be called from unit tests without touching global state.
fn collect_servers_from(
    snapshot: Vec<std::sync::Arc<crate::stats::ServerStats>>,
    filters: &ServerFilters,
) -> ServersDto {
    let mut rows: Vec<ServerDto> = snapshot
        .iter()
        .filter(|s| server_matches(s, filters))
        .map(server_to_dto)
        .collect();

    let total = rows.len() as u64;

    rows.sort_by(|a, b| {
        let ord = match filters.sort {
            ServerSort::AgeSeconds => a.age_seconds.cmp(&b.age_seconds),
            ServerSort::QueriesTotal => a.queries_total.cmp(&b.queries_total),
            ServerSort::ErrorsTotal => a.errors_total.cmp(&b.errors_total),
            ServerSort::ActiveAgeMs => a.active_age_ms.cmp(&b.active_age_ms),
        };
        match filters.order {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        }
    });

    let limit = filters.limit.clamp(1, MAX_LIMIT);
    let offset = filters.offset;
    let page: Vec<_> = rows
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    ServersDto {
        ts: now_unix_ms(),
        total,
        limit,
        offset,
        servers: page,
    }
}

fn server_matches(s: &crate::stats::ServerStats, f: &ServerFilters) -> bool {
    let pool_name = s.pool_name();
    let user = s.username();

    if let Some(p) = &f.pool {
        let id = format!("{}@{}", user, pool_name);
        if id != *p {
            return false;
        }
    }
    if let Some(db) = &f.database {
        if pool_name != *db {
            return false;
        }
    }
    if let Some(u) = &f.user {
        if user != *u {
            return false;
        }
    }
    true
}

fn server_to_dto(s: &std::sync::Arc<crate::stats::ServerStats>) -> ServerDto {
    let age_seconds = s.connect_time().elapsed().as_secs();
    let application_name = s.application_name();
    ServerDto {
        server_id: s.server_id(),
        process_id: s.process_id(),
        database: s.pool_name(),
        user: s.username(),
        application_name,
        tls: s.tls(),
        state: s.state_to_string(),
        wait: s.wait_to_string(),
        age_seconds,
        active_age_ms: s.active_age_ms().unwrap_or(0),
        transactions_total: s.transaction_count.load(Ordering::Relaxed),
        queries_total: s.query_count.load(Ordering::Relaxed),
        errors_total: s.error_count.load(Ordering::Relaxed),
        bytes_sent: s.bytes_sent.load(Ordering::Relaxed),
        bytes_received: s.bytes_received.load(Ordering::Relaxed),
        prepared_hits_total: s.prepared_hit_count.load(Ordering::Relaxed),
        prepared_misses_total: s.prepared_miss_count.load(Ordering::Relaxed),
        prepared_cache_size: s.prepared_cache_size.load(Ordering::Relaxed),
    }
}

pub fn collect_connections() -> ConnectionsDto {
    connections_from_raw(
        cnt(&TOTAL_CONNECTION_COUNTER),
        cnt(&TLS_CONNECTION_COUNTER),
        cnt(&PLAIN_CONNECTION_COUNTER),
        cnt(&CANCEL_CONNECTION_COUNTER),
    )
}

/// Builds a `ConnectionsDto` from raw counter values. Pure function — exists
/// so the `errors = total - tls - plain - cancel` derivation is exercised by
/// unit tests without touching the global atomics.
fn connections_from_raw(total: u64, tls: u64, plain: u64, cancel: u64) -> ConnectionsDto {
    ConnectionsDto {
        ts: now_unix_ms(),
        total,
        tls,
        plain,
        cancel,
        // `errors` mirrors `SHOW CONNECTIONS`: it is whatever is left after
        // subtracting the categorised counters from the total. May be zero or
        // positive in normal operation.
        errors: total
            .saturating_sub(tls)
            .saturating_sub(plain)
            .saturating_sub(cancel),
    }
}

pub fn collect_stats() -> StatsDto {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let mut stats: Vec<StatsRowDto> = pool_lookup
        .iter()
        .map(|(identifier, s)| StatsRowDto {
            id: format!("{}@{}", identifier.user, identifier.db),
            database: identifier.db.clone(),
            user: identifier.user.clone(),
            total_xact_count: s.total_xact_count,
            total_query_count: s.total_query_count,
            total_received: s.total_received,
            total_sent: s.total_sent,
            total_xact_time: s.total_xact_time_microseconds,
            total_query_time: s.total_query_time_microseconds,
            total_wait_time: s.wait_time,
            total_errors: s.errors,
            avg_xact_count: s.avg_xact_count,
            avg_query_count: s.avg_query_count,
            avg_recv: s.avg_recv,
            avg_sent: s.avg_sent,
            // `avg_errors` mirrors `generate_show_stats_row`: uses `errors` (no
            // per-window rate stored in PoolStats).
            avg_errors: s.errors,
            avg_xact_time: s.avg_xact_time_microsecons,
            avg_query_time: s.avg_query_time_microseconds,
            avg_wait_time: s.avg_wait_time,
        })
        .collect();

    // Stable order: same `id` ordering as `/api/pools` for deterministic UI.
    stats.sort_by(|a, b| a.id.cmp(&b.id));

    StatsDto {
        ts: now_unix_ms(),
        stats,
    }
}

pub fn collect_databases() -> DatabasesDto {
    let pools_map = get_all_pools();
    let mut databases: Vec<DatabaseDto> = pools_map
        .iter()
        .map(|(_identifier, pool)| {
            let address = pool.address();
            let settings = &pool.settings;
            DatabaseDto {
                name: address.name(),
                host: address.host.clone(),
                port: address.port,
                database: address.database.clone(),
                force_user: settings.user.username.clone(),
                pool_size: settings.user.pool_size,
                min_pool_size: settings.user.min_pool_size.unwrap_or(0),
                // See DatabaseDto::reserve_pool — mirrors SHOW DATABASES quirk.
                reserve_pool: 0,
                pool_mode: settings.pool_mode.to_string(),
                max_connections: settings.user.pool_size,
                current_connections: pool.pool_state().size as u32,
            }
        })
        .collect();

    // Deterministic order using the pool name composite key.
    databases.sort_by(|a, b| a.name.cmp(&b.name));

    DatabasesDto {
        ts: now_unix_ms(),
        databases,
    }
}

pub fn collect_users() -> UsersDto {
    let pools_map = get_all_pools();
    let mut users: Vec<UserDto> = pools_map
        .iter()
        .map(|(identifier, pool)| UserDto {
            name: identifier.user.clone(),
            pool_mode: pool.settings.pool_mode.to_string(),
        })
        .collect();

    users.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.pool_mode.cmp(&b.pool_mode))
    });

    UsersDto {
        ts: now_unix_ms(),
        users,
    }
}

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

pub fn collect_log_level() -> LogLevelDto {
    LogLevelDto {
        ts: now_unix_ms(),
        log_level: log_level::get_log_level(),
    }
}

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
            inflight: snapshot.inflight_creates as u64,
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

pub fn collect_prepared() -> PreparedDto {
    let mut prepared: Vec<PreparedRowDto> = Vec::new();
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        for (hash, parse, count_used, kind) in cache.get_entries() {
            prepared.push(PreparedRowDto {
                pool: identifier.to_string(),
                hash: hash.to_string(),
                name: parse.name.clone(),
                count_used,
                kind: kind.as_str().to_string(),
            });
        }
    }

    // Stable order: pool first, then hash, for deterministic UI display.
    prepared.sort_by(|a, b| {
        (a.pool.as_str(), a.hash.as_str()).cmp(&(b.pool.as_str(), b.hash.as_str()))
    });

    PreparedDto {
        ts: now_unix_ms(),
        prepared,
    }
}

pub fn collect_interner() -> InternerDto {
    let named = named_snapshot();
    let anon = anon_snapshot();
    let named_bytes: u64 = named.iter().map(|(_, e)| e.text().len() as u64).sum();
    let anon_bytes: u64 = anon.iter().map(|(_, e)| e.text().len() as u64).sum();

    InternerDto {
        ts: now_unix_ms(),
        named: InternerKindDto {
            entries: named.len() as u64,
            bytes: named_bytes,
        },
        anonymous: InternerKindDto {
            entries: anon.len() as u64,
            bytes: anon_bytes,
        },
    }
}

pub fn collect_prepared_text(hash: u64) -> Option<PreparedTextDto> {
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        for (h, parse, _count, kind) in cache.get_entries() {
            if h == hash {
                return Some(PreparedTextDto {
                    ts: now_unix_ms(),
                    hash: format!("{:#x}", hash),
                    pool: identifier.to_string(),
                    name: parse.name.clone(),
                    query: parse.query().to_string(),
                    kind: kind.as_str().to_string(),
                });
            }
        }
    }
    None
}

/// Clamps the user-supplied `?n=` parameter to a sensible range.
///
/// `0` and missing → default 20 (matches SHOW INTERNER TOP convention).
/// Values above 200 are capped — the page would be unusable beyond that
/// and a 100k-entry interner shouldn't materialise an unbounded preview list.
pub(crate) fn clamp_top_n(requested: u64) -> u64 {
    const DEFAULT: u64 = 20;
    const MAX: u64 = 200;
    match requested {
        0 => DEFAULT,
        n if n > MAX => MAX,
        n => n,
    }
}

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
            let queries_total = s.query_count.load(Ordering::Relaxed);
            let errors_total = s.error_count.load(Ordering::Relaxed);
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
        entry.queries_total += s.query_count.load(Ordering::Relaxed);
        entry.transactions_total += s.transaction_count.load(Ordering::Relaxed);
        entry.errors_total += s.error_count.load(Ordering::Relaxed);
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

pub fn collect_interner_top(n: u64) -> InternerTopDto {
    let n = clamp_top_n(n);
    let now = now_monotonic_ms();

    enum Handle {
        Named(std::sync::Arc<crate::server::NamedEntry>),
        Anon(std::sync::Arc<crate::server::AnonEntry>),
    }

    let mut combined: Vec<(u64, &'static str, usize, i64, Handle)> = Vec::new();
    for (hash, entry) in named_snapshot() {
        let bytes = entry.text().len();
        combined.push((hash, "named", bytes, -1, Handle::Named(entry)));
    }
    for (hash, entry) in anon_snapshot() {
        let idle = entry.idle_ms(now) as i64;
        let bytes = entry.text().len();
        combined.push((hash, "anonymous", bytes, idle, Handle::Anon(entry)));
    }
    combined.sort_by_key(|r| std::cmp::Reverse(r.2));

    let entries = combined
        .into_iter()
        .take(n as usize)
        .map(|(hash, kind, bytes, idle_ms, handle)| {
            let text = match handle {
                Handle::Named(e) => e.text().clone(),
                Handle::Anon(e) => e.text().clone(),
            };
            let preview: String = text.chars().take(120).collect();
            InternerTopRowDto {
                hash: format!("{:#x}", hash),
                kind: kind.to_string(),
                bytes: bytes as u64,
                idle_ms,
                preview,
            }
        })
        .collect();

    InternerTopDto {
        ts: now_unix_ms(),
        n,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{client::ClientStats, server::ServerStats};
    use crate::utils::clock;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    // ---------------------------------------------------------------------------
    // Fixture helpers
    // ---------------------------------------------------------------------------

    fn make_client(
        connection_id: u64,
        db: &str,
        user: &str,
        app: &str,
        queries: u64,
        errors: u64,
    ) -> Arc<ClientStats> {
        let stats = Arc::new(ClientStats::new(
            connection_id,
            app,
            user,
            db,
            "127.0.0.1",
            clock::now(),
            false,
        ));
        stats.query_count.store(queries, Ordering::Relaxed);
        stats.error_count.store(errors, Ordering::Relaxed);
        stats
    }

    fn make_server(db: &str, user: &str) -> Arc<ServerStats> {
        let address = crate::config::Address {
            pool_name: db.to_string(),
            username: user.to_string(),
            ..crate::config::Address::default()
        };
        Arc::new(ServerStats::new(address, clock::now()))
    }

    fn default_client_filters() -> ClientFilters {
        ClientFilters {
            limit: 100,
            offset: 0,
            sort: ClientSort::QueriesTotal,
            order: SortOrder::Asc,
            pool: None,
            database: None,
            user: None,
            application_name: vec![],
            state: vec![],
        }
    }

    fn default_server_filters() -> ServerFilters {
        ServerFilters {
            limit: 100,
            offset: 0,
            sort: ServerSort::AgeSeconds,
            order: SortOrder::Asc,
            pool: None,
            database: None,
            user: None,
        }
    }

    // ---------------------------------------------------------------------------
    // Client filter tests
    // ---------------------------------------------------------------------------

    #[test]
    fn client_filter_by_pool_exact_match() {
        // pool filter uses the "user@db" composite id.
        let clients = vec![
            make_client(1, "db1", "alice", "app", 0, 0),
            make_client(2, "db2", "bob", "app", 0, 0),
        ];
        let mut f = default_client_filters();
        f.pool = Some("alice@db1".to_string());
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.clients[0].user, "alice");
    }

    #[test]
    fn client_filter_by_database_only() {
        let clients = vec![
            make_client(1, "prod", "alice", "app", 0, 0),
            make_client(2, "staging", "alice", "app", 0, 0),
        ];
        let mut f = default_client_filters();
        f.database = Some("prod".to_string());
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.clients[0].database, "prod");
    }

    #[test]
    fn client_filter_by_user_only() {
        let clients = vec![
            make_client(1, "db", "alice", "app", 0, 0),
            make_client(2, "db", "bob", "app", 0, 0),
            make_client(3, "db", "alice", "app2", 0, 0),
        ];
        let mut f = default_client_filters();
        f.user = Some("alice".to_string());
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
        assert!(result.clients.iter().all(|c| c.user == "alice"));
    }

    #[test]
    fn client_filter_application_name_or_semantics() {
        // A row matches if its app_name is in the filter list (OR).
        let clients = vec![
            make_client(1, "db", "alice", "pgadmin", 0, 0),
            make_client(2, "db", "bob", "psql", 0, 0),
            make_client(3, "db", "carol", "other", 0, 0),
        ];
        let mut f = default_client_filters();
        f.application_name = vec!["pgadmin".to_string(), "psql".to_string()];
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
        let apps: Vec<_> = result
            .clients
            .iter()
            .map(|c| c.application_name.as_str())
            .collect();
        assert!(apps.contains(&"pgadmin"));
        assert!(apps.contains(&"psql"));
    }

    #[test]
    fn client_filter_state_or_semantics() {
        // Default state for a fresh ClientStats is "idle".
        let clients = vec![
            make_client(1, "db", "alice", "app", 0, 0),
            make_client(2, "db", "bob", "app", 0, 0),
        ];
        let mut f = default_client_filters();
        f.state = vec!["idle".to_string()];
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);

        // Filter for a state that no client is in returns nothing.
        f.state = vec!["active".to_string()];
        let clients2 = vec![make_client(10, "db", "alice", "app", 0, 0)];
        let result2 = collect_clients_from(clients2, &f);
        assert_eq!(result2.total, 0);
    }

    // ---------------------------------------------------------------------------
    // Client sort tests
    // ---------------------------------------------------------------------------

    #[test]
    fn client_sort_queries_total_asc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 30, 0),
            make_client(2, "db", "u", "a", 10, 0),
            make_client(3, "db", "u", "a", 20, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::QueriesTotal;
        f.order = SortOrder::Asc;
        let result = collect_clients_from(clients, &f);
        let counts: Vec<u64> = result.clients.iter().map(|c| c.queries_total).collect();
        assert_eq!(counts, vec![10, 20, 30]);
    }

    #[test]
    fn client_sort_queries_total_desc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 30, 0),
            make_client(2, "db", "u", "a", 10, 0),
            make_client(3, "db", "u", "a", 20, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::QueriesTotal;
        f.order = SortOrder::Desc;
        let result = collect_clients_from(clients, &f);
        let counts: Vec<u64> = result.clients.iter().map(|c| c.queries_total).collect();
        assert_eq!(counts, vec![30, 20, 10]);
    }

    #[test]
    fn client_sort_errors_total_asc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 5),
            make_client(2, "db", "u", "a", 0, 1),
            make_client(3, "db", "u", "a", 0, 3),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::ErrorsTotal;
        f.order = SortOrder::Asc;
        let result = collect_clients_from(clients, &f);
        let errs: Vec<u64> = result.clients.iter().map(|c| c.errors_total).collect();
        assert_eq!(errs, vec![1, 3, 5]);
    }

    #[test]
    fn client_sort_errors_total_desc() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 5),
            make_client(2, "db", "u", "a", 0, 1),
            make_client(3, "db", "u", "a", 0, 3),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::ErrorsTotal;
        f.order = SortOrder::Desc;
        let result = collect_clients_from(clients, &f);
        let errs: Vec<u64> = result.clients.iter().map(|c| c.errors_total).collect();
        assert_eq!(errs, vec![5, 3, 1]);
    }

    #[test]
    fn client_sort_age_seconds_asc() {
        // All fixtures share the same clock::now() so ages will all be 0.
        // The sort should be stable and return all rows without panicking.
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 0),
            make_client(2, "db", "u", "a", 0, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::AgeSeconds;
        f.order = SortOrder::Asc;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn client_sort_current_query_age_ms_desc() {
        // Clients not in ACTIVE state return current_query_age_ms == 0.
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 0),
            make_client(2, "db", "u", "a", 0, 0),
        ];
        let mut f = default_client_filters();
        f.sort = ClientSort::CurrentQueryAgeMs;
        f.order = SortOrder::Desc;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
    }

    // ---------------------------------------------------------------------------
    // Client pagination tests
    // ---------------------------------------------------------------------------

    #[test]
    fn client_pagination_offset_beyond_total_returns_empty() {
        let clients = vec![
            make_client(1, "db", "u", "a", 0, 0),
            make_client(2, "db", "u", "a", 0, 0),
        ];
        let mut f = default_client_filters();
        f.offset = 10; // beyond total of 2
        f.limit = 100;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.total, 2);
        assert!(result.clients.is_empty());
    }

    #[test]
    fn client_pagination_limit_clamped_to_max_limit() {
        let clients: Vec<_> = (0..5)
            .map(|i| make_client(i, "db", "u", "a", 0, 0))
            .collect();
        let mut f = default_client_filters();
        f.limit = MAX_LIMIT + 9999; // above cap
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.limit, MAX_LIMIT);
    }

    #[test]
    fn client_pagination_limit_one() {
        let clients: Vec<_> = (0..5)
            .map(|i| make_client(i, "db", "u", "a", 0, 0))
            .collect();
        let mut f = default_client_filters();
        f.limit = 1;
        let result = collect_clients_from(clients, &f);
        assert_eq!(result.clients.len(), 1);
        assert_eq!(result.total, 5);
    }

    // ---------------------------------------------------------------------------
    // Server filter tests
    // ---------------------------------------------------------------------------

    #[test]
    fn server_filter_by_pool() {
        let servers = vec![make_server("db1", "alice"), make_server("db2", "bob")];
        let mut f = default_server_filters();
        f.pool = Some("alice@db1".to_string());
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.servers[0].database, "db1");
    }

    #[test]
    fn server_filter_by_database() {
        let servers = vec![
            make_server("prod", "alice"),
            make_server("staging", "alice"),
        ];
        let mut f = default_server_filters();
        f.database = Some("prod".to_string());
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.servers[0].database, "prod");
    }

    #[test]
    fn server_filter_by_user() {
        let servers = vec![make_server("db", "alice"), make_server("db", "bob")];
        let mut f = default_server_filters();
        f.user = Some("alice".to_string());
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 1);
        assert_eq!(result.servers[0].user, "alice");
    }

    // ---------------------------------------------------------------------------
    // Server sort tests
    // ---------------------------------------------------------------------------

    #[test]
    fn server_sort_queries_total_asc() {
        let servers = vec![
            make_server("db", "u"),
            make_server("db", "u"),
            make_server("db", "u"),
        ];
        servers[0].query_count.store(30, Ordering::Relaxed);
        servers[1].query_count.store(10, Ordering::Relaxed);
        servers[2].query_count.store(20, Ordering::Relaxed);
        let mut f = default_server_filters();
        f.sort = ServerSort::QueriesTotal;
        f.order = SortOrder::Asc;
        let result = collect_servers_from(servers, &f);
        let counts: Vec<u64> = result.servers.iter().map(|s| s.queries_total).collect();
        assert_eq!(counts, vec![10, 20, 30]);
    }

    #[test]
    fn server_sort_queries_total_desc() {
        let servers = vec![
            make_server("db", "u"),
            make_server("db", "u"),
            make_server("db", "u"),
        ];
        servers[0].query_count.store(30, Ordering::Relaxed);
        servers[1].query_count.store(10, Ordering::Relaxed);
        servers[2].query_count.store(20, Ordering::Relaxed);
        let mut f = default_server_filters();
        f.sort = ServerSort::QueriesTotal;
        f.order = SortOrder::Desc;
        let result = collect_servers_from(servers, &f);
        let counts: Vec<u64> = result.servers.iter().map(|s| s.queries_total).collect();
        assert_eq!(counts, vec![30, 20, 10]);
    }

    #[test]
    fn server_sort_errors_total_asc() {
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        servers[0].error_count.store(5, Ordering::Relaxed);
        servers[1].error_count.store(1, Ordering::Relaxed);
        let mut f = default_server_filters();
        f.sort = ServerSort::ErrorsTotal;
        f.order = SortOrder::Asc;
        let result = collect_servers_from(servers, &f);
        let errs: Vec<u64> = result.servers.iter().map(|s| s.errors_total).collect();
        assert_eq!(errs, vec![1, 5]);
    }

    #[test]
    fn server_sort_active_age_ms_desc() {
        // Servers not in ACTIVE state return active_age_ms == 0.
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        let mut f = default_server_filters();
        f.sort = ServerSort::ActiveAgeMs;
        f.order = SortOrder::Desc;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn server_sort_age_seconds_desc() {
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        let mut f = default_server_filters();
        f.sort = ServerSort::AgeSeconds;
        f.order = SortOrder::Desc;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 2);
    }

    // ---------------------------------------------------------------------------
    // Server pagination tests
    // ---------------------------------------------------------------------------

    #[test]
    fn server_pagination_offset_beyond_total_returns_empty() {
        let servers = vec![make_server("db", "u"), make_server("db", "u")];
        let mut f = default_server_filters();
        f.offset = 10;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.total, 2);
        assert!(result.servers.is_empty());
    }

    #[test]
    fn server_pagination_limit_clamped_to_max_limit() {
        let servers: Vec<_> = (0..5).map(|_| make_server("db", "u")).collect();
        let mut f = default_server_filters();
        f.limit = MAX_LIMIT + 9999;
        let result = collect_servers_from(servers, &f);
        assert_eq!(result.limit, MAX_LIMIT);
    }

    // ---------------------------------------------------------------------------
    // ConnectionsDto math
    // ---------------------------------------------------------------------------

    #[test]
    fn connections_errors_derive_from_total_minus_categorised() {
        let dto = super::connections_from_raw(100, 60, 30, 5);
        assert_eq!(dto.total, 100);
        assert_eq!(dto.tls, 60);
        assert_eq!(dto.plain, 30);
        assert_eq!(dto.cancel, 5);
        assert_eq!(dto.errors, 5);
    }

    #[test]
    fn connections_errors_zero_when_categories_cover_total() {
        let dto = super::connections_from_raw(50, 30, 15, 5);
        assert_eq!(dto.errors, 0);
    }

    #[test]
    fn connections_errors_saturate_when_categories_exceed_total() {
        // Race: categorised counters momentarily ahead of total.
        // Without saturating_sub this would underflow into u64::MAX.
        let dto = super::connections_from_raw(10, 8, 5, 0);
        assert_eq!(dto.errors, 0);
    }

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

    #[test]
    fn clamp_top_n_zero_returns_default() {
        assert_eq!(super::clamp_top_n(0), 20);
    }

    #[test]
    fn clamp_top_n_keeps_in_range() {
        assert_eq!(super::clamp_top_n(1), 1);
        assert_eq!(super::clamp_top_n(50), 50);
        assert_eq!(super::clamp_top_n(200), 200);
    }

    #[test]
    fn clamp_top_n_caps_above_max() {
        assert_eq!(super::clamp_top_n(201), 200);
        assert_eq!(super::clamp_top_n(u64::MAX), 200);
    }

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
        let clients: Vec<_> = (0..5)
            .map(|i| make_client(i, "db", "u", "a", 0, 0))
            .collect();
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
        let app_a = result
            .apps
            .iter()
            .find(|a| a.application_name == "appA")
            .unwrap();
        let app_b = result
            .apps
            .iter()
            .find(|a| a.application_name == "appB")
            .unwrap();
        assert_eq!(app_a.clients, 2);
        assert_eq!(app_a.queries_total, 30);
        assert_eq!(app_b.clients, 1);
        assert_eq!(app_b.queries_total, 5);
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
        let names: Vec<_> = result
            .apps
            .iter()
            .map(|a| a.application_name.clone())
            .collect();
        assert_eq!(names, vec!["appB", "appC", "appA"]);
    }
}
