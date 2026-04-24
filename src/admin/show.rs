//! Admin SHOW commands implementation.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use bytes::{BufMut, BytesMut};

use crate::app::log_level;
use crate::config::{get_config, VERSION};
use crate::errors::Error;
use crate::messages::protocol::{command_complete, data_row, row_description};
use crate::messages::socket::write_all_half;
use crate::messages::types::DataType;
use crate::pool::{get_all_pools, AUTH_QUERY_STATE, COORDINATORS, DYNAMIC_POOLS};
use crate::stats::client::{CLIENT_STATE_ACTIVE, CLIENT_STATE_IDLE};
#[cfg(target_os = "linux")]
use crate::stats::get_socket_states_count;
use crate::stats::pool::PoolStats;
use crate::stats::server::{SERVER_STATE_ACTIVE, SERVER_STATE_IDLE};
use crate::stats::{
    get_client_stats, get_server_stats, CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER,
    TLS_CONNECTION_COUNTER, TOTAL_CONNECTION_COUNTER,
};

/// Column-oriented statistics.
pub async fn show_lists<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let client_stats = get_client_stats();
    let server_stats = get_server_stats();
    let columns = vec![("list", DataType::Text), ("items", DataType::Int4)];
    let mut users = 1;
    let mut databases = 1;
    for (_, _) in get_all_pools().iter() {
        databases += 1; // One db per pool
        users += 1; // One user per pool
    }
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    res.put(data_row(&["databases".to_string(), databases.to_string()]));
    res.put(data_row(&["users".to_string(), users.to_string()]));
    res.put(data_row(&["pools".to_string(), databases.to_string()]));
    res.put(data_row(&[
        "free_clients".to_string(),
        client_stats
            .keys()
            .filter(|client_id| client_stats.get(client_id).unwrap().state() == CLIENT_STATE_IDLE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&[
        "used_clients".to_string(),
        client_stats
            .keys()
            .filter(|client_id| client_stats.get(client_id).unwrap().state() == CLIENT_STATE_ACTIVE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&["login_clients".to_string(), "0".to_string()]));
    res.put(data_row(&[
        "free_servers".to_string(),
        server_stats
            .keys()
            .filter(|server_id| server_stats.get(server_id).unwrap().state() == SERVER_STATE_IDLE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&[
        "used_servers".to_string(),
        server_stats
            .keys()
            .filter(|server_id| server_stats.get(server_id).unwrap().state() == SERVER_STATE_ACTIVE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&["dns_names".to_string(), "0".to_string()]));
    res.put(data_row(&["dns_zones".to_string(), "0".to_string()]));
    res.put(data_row(&["dns_queries".to_string(), "0".to_string()]));
    res.put(data_row(&["dns_pending".to_string(), "0".to_string()]));
    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show PgDoorman version.
pub async fn show_version<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put(row_description(&vec![("version", DataType::Text)]));
    res.put(data_row(&[format!("PgDoorman {}", VERSION)]));
    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show current log level filter.
pub async fn show_log_level<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put(row_description(&vec![("log_level", DataType::Text)]));
    res.put(data_row(&[log_level::get_log_level()]));
    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show utilization of connection pools for each pool.
pub async fn show_pools<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pool_lookup = PoolStats::construct_pool_lookup();
    let mut res = BytesMut::new();
    res.put(row_description(&PoolStats::generate_show_pools_header()));
    pool_lookup.iter().for_each(|(_identifier, pool_stats)| {
        res.put(data_row(&pool_stats.generate_show_pools_row()));
    });
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show memory utilization of connection pools.
pub async fn show_pools_memory<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pool_lookup = PoolStats::construct_pool_lookup();
    let mut res = BytesMut::new();
    res.put(row_description(
        &PoolStats::generate_show_pools_memory_header(),
    ));
    pool_lookup.iter().for_each(|(_identifier, pool_stats)| {
        res.put(data_row(&pool_stats.generate_show_pools_memory_row()));
    });
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show all entries in the global prepared statement cache across all pools.
pub async fn show_prepared_statements<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("pool", DataType::Text),
        ("hash", DataType::Numeric),
        ("name", DataType::Text),
        ("query", DataType::Text),
        ("count_used", DataType::Numeric),
    ];
    let mut res = BytesMut::new();
    res.put(row_description(&columns));

    for (identifier, pool) in get_all_pools().iter() {
        if let Some(cache) = pool.prepared_statement_cache.as_ref() {
            let entries = cache.get_entries();
            for (hash, parse, last_used) in entries {
                res.put(data_row(&[
                    identifier.to_string(),
                    hash.to_string(),
                    parse.name.clone(),
                    parse.query().to_string(),
                    last_used.to_string(),
                ]));
            }
        }
    }

    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show extended utilization of connection pools for each pool.
pub async fn show_pools_extended<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pool_lookup = PoolStats::construct_pool_lookup();
    let mut res = BytesMut::new();
    res.put(row_description(
        &PoolStats::generate_show_pools_extended_header(),
    ));
    pool_lookup.iter().for_each(|(_identifier, pool_stats)| {
        res.put(data_row(&pool_stats.generate_show_pools_extended_row()));
    });
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show all available options.
pub async fn show_help<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![("item", DataType::Text)];
    let show_list = super::SHOW_SUBCOMMANDS
        .iter()
        .map(|s| s.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("|");
    let help_items = [
        format!("SHOW {show_list}"),
        "SHOW LISTS".to_string(),
        "SHOW CONNECTIONS".to_string(),
        "SHOW STATS".to_string(),
        "SET log_level = '<filter>'".to_string(),
        "RELOAD".to_string(),
        "SHUTDOWN".to_string(),
        "UPGRADE".to_string(),
        "PAUSE [db]".to_string(),
        "RESUME [db]".to_string(),
        "RECONNECT [db]".to_string(),
    ];
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    for item in &help_items {
        res.put(data_row(&[item.as_str()]));
    }
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show databases.
pub async fn show_databases<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    // Columns
    let columns = vec![
        ("name", DataType::Text),
        ("host", DataType::Text),
        ("port", DataType::Text),
        ("database", DataType::Text),
        ("force_user", DataType::Text),
        ("pool_size", DataType::Int4),
        ("min_pool_size", DataType::Int4),
        ("reserve_pool", DataType::Int4),
        ("pool_mode", DataType::Text),
        ("max_connections", DataType::Int4),
        ("current_connections", DataType::Int4),
    ];
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    for (_, pool) in get_all_pools().iter() {
        let pool_config = pool.settings.clone();
        let database_name = &pool.address().database;
        let address = pool.address();
        let pool_state = pool.pool_state();
        res.put(data_row(&[
            address.name(),                                          // name
            address.host.to_string(),                                // host
            address.port.to_string(),                                // port
            database_name.to_string(),                               // database
            pool_config.user.username.to_string(),                   // force_user
            pool_config.user.pool_size.to_string(),                  // pool_size
            pool_config.user.min_pool_size.unwrap_or(0).to_string(), // min_pool_size
            "0".to_string(),                                         // reserve_pool
            pool_config.pool_mode.to_string(),                       // pool_mode
            pool_config.user.pool_size.to_string(),                  // max_connections
            pool_state.size.to_string(),                             // current_connections
        ]));
    }
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Shows current configuration.
pub async fn show_config<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let config = &get_config();
    let config: HashMap<String, String> = config.into();
    // Configs that cannot be changed without restarting.
    let immutables = ["host", "port", "connect_timeout"];
    // Columns
    let columns = vec![
        ("key", DataType::Text),
        ("value", DataType::Text),
        ("default", DataType::Text),
        ("changeable", DataType::Text),
    ];
    // Response data
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    // DataRow rows
    for (key, value) in config {
        let changeable = if immutables.iter().filter(|col| *col == &key).count() == 1 {
            "no".to_string()
        } else {
            "yes".to_string()
        };
        let row = vec![key, value, "-".to_string(), changeable];
        res.put(data_row(&row));
    }
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show stats.
pub async fn show_stats<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pool_lookup = PoolStats::construct_pool_lookup();
    let mut res = BytesMut::new();
    res.put(row_description(&PoolStats::generate_show_stats_header()));
    pool_lookup.iter().for_each(|(_identifier, pool_stats)| {
        res.put(data_row(&pool_stats.generate_show_stats_row()));
    });
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show currently connected clients
pub async fn show_clients<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("client_id", DataType::Text),
        ("database", DataType::Text),
        ("user", DataType::Text),
        ("application_name", DataType::Text),
        ("addr", DataType::Text),
        ("tls", DataType::Text),
        ("state", DataType::Text),
        ("wait", DataType::Text),
        ("transaction_count", DataType::Numeric),
        ("query_count", DataType::Numeric),
        ("error_count", DataType::Numeric),
        ("age_seconds", DataType::Numeric),
    ];
    let new_map = get_client_stats();
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    for (_, client) in new_map {
        let row = vec![
            format!("#c{}", client.connection_id()),
            client.pool_name(),
            client.username(),
            client.application_name(),
            client.ipaddr(),
            client.tls().to_string(),
            client.state_to_string(),
            client.wait_to_string(),
            client.transaction_count.load(Ordering::Relaxed).to_string(),
            client.query_count.load(Ordering::Relaxed).to_string(),
            client.error_count.load(Ordering::Relaxed).to_string(),
            client.connect_time().elapsed().as_secs().to_string(),
        ];
        res.put(data_row(&row));
    }
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show connections.
pub async fn show_connections<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("total", DataType::Numeric),
        ("errors", DataType::Numeric),
        ("tls", DataType::Numeric),
        ("plain", DataType::Numeric),
        ("cancel", DataType::Numeric),
    ];
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    let total = TOTAL_CONNECTION_COUNTER.load(Ordering::Relaxed);
    let tls = TLS_CONNECTION_COUNTER.load(Ordering::Relaxed);
    let plain = PLAIN_CONNECTION_COUNTER.load(Ordering::Relaxed);
    let cancel = CANCEL_CONNECTION_COUNTER.load(Ordering::Relaxed);
    let error = total - tls - plain - cancel;
    let row = vec![
        total.to_string(),
        error.to_string(),
        tls.to_string(),
        plain.to_string(),
        cancel.to_string(),
    ];
    res.put(data_row(&row));
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show currently connected servers
pub async fn show_servers<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("server_id", DataType::Text),
        ("server_process_id", DataType::Text),
        ("database_name", DataType::Text),
        ("user", DataType::Text),
        ("application_name", DataType::Text),
        ("tls", DataType::Text),
        ("state", DataType::Text),
        ("wait", DataType::Text),
        ("transaction_count", DataType::Numeric),
        ("query_count", DataType::Numeric),
        ("bytes_sent", DataType::Numeric),
        ("bytes_received", DataType::Numeric),
        ("age_seconds", DataType::Numeric),
        ("prepare_cache_hit", DataType::Numeric),
        ("prepare_cache_miss", DataType::Numeric),
        ("prepare_cache_size", DataType::Numeric),
    ];
    let new_map = get_server_stats();
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    for (_, server) in new_map {
        let application_name = server.application_name.lock();
        let row = vec![
            format!("{:#010X}", server.server_id()),
            server.process_id().to_string(),
            server.pool_name(),
            server.username(),
            application_name.clone(),
            server.tls().to_string(),
            server.state_to_string(),
            server.wait_to_string(),
            server.transaction_count.load(Ordering::Relaxed).to_string(),
            server.query_count.load(Ordering::Relaxed).to_string(),
            server.bytes_sent.load(Ordering::Relaxed).to_string(),
            server.bytes_received.load(Ordering::Relaxed).to_string(),
            server.connect_time().elapsed().as_secs().to_string(),
            server
                .prepared_hit_count
                .load(Ordering::Relaxed)
                .to_string(),
            server
                .prepared_miss_count
                .load(Ordering::Relaxed)
                .to_string(),
            server
                .prepared_cache_size
                .load(Ordering::Relaxed)
                .to_string(),
        ];
        res.put(data_row(&row));
    }
    res.put(command_complete("SHOW"));
    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show Users.
pub async fn show_users<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    res.put(row_description(&vec![
        ("name", DataType::Text),
        ("pool_mode", DataType::Text),
    ]));
    for (user_pool, pool) in get_all_pools().iter() {
        let pool_config = &pool.settings;
        res.put(data_row(&[
            user_pool.user.clone(),
            pool_config.pool_mode.to_string(),
        ]));
    }
    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show auth_query cache and authentication metrics per database pool.
pub async fn show_auth_query<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("database", DataType::Text),
        ("cache_entries", DataType::Numeric),
        ("cache_hits", DataType::Numeric),
        ("cache_misses", DataType::Numeric),
        ("cache_refetches", DataType::Numeric),
        ("cache_rate_limited", DataType::Numeric),
        ("auth_success", DataType::Numeric),
        ("auth_failure", DataType::Numeric),
        ("executor_queries", DataType::Numeric),
        ("executor_errors", DataType::Numeric),
        ("dynamic_pools_current", DataType::Numeric),
        ("dynamic_pools_created", DataType::Numeric),
        ("dynamic_pools_destroyed", DataType::Numeric),
    ];

    let states = AUTH_QUERY_STATE.load();
    let dynamic = DYNAMIC_POOLS.load();

    let mut res = BytesMut::new();
    res.put(row_description(&columns));

    for (pool_name, state) in states.iter() {
        let cache_entries = state.cache_len();
        let dyn_current = dynamic.iter().filter(|id| id.db == *pool_name).count();
        let s = state.stats.snapshot();

        res.put(data_row(&[
            pool_name.clone(),
            cache_entries.to_string(),
            s.cache_hits.to_string(),
            s.cache_misses.to_string(),
            s.cache_refetches.to_string(),
            s.cache_rate_limited.to_string(),
            s.auth_success.to_string(),
            s.auth_failure.to_string(),
            s.executor_queries.to_string(),
            s.executor_errors.to_string(),
            dyn_current.to_string(),
            s.dynamic_pools_created.to_string(),
            s.dynamic_pools_destroyed.to_string(),
        ]));
    }

    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

#[cfg(target_os = "linux")]
pub async fn show_sockets<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();
    let sockets_info = match get_socket_states_count(std::process::id()) {
        Ok(info) => info,
        Err(_) => return Err(Error::ServerError),
    };
    res.put(row_description(&vec![
        // tcp
        ("tcp_established", DataType::Numeric),
        ("tcp_syn_sent", DataType::Numeric),
        ("tcp_syn_recv", DataType::Numeric),
        ("tcp_fin_wait1", DataType::Numeric),
        ("tcp_fin_wait2", DataType::Numeric),
        ("tcp_time_wait", DataType::Numeric),
        ("tcp_close", DataType::Numeric),
        ("tcp_close_wait", DataType::Numeric),
        ("tcp_last_ack", DataType::Numeric),
        ("tcp_listen", DataType::Numeric),
        ("tcp_closing", DataType::Numeric),
        ("tcp_new_syn_recv", DataType::Numeric),
        ("tcp_bound_inactive", DataType::Numeric),
        // tcp6
        ("tcp6_established", DataType::Numeric),
        ("tcp6_syn_sent", DataType::Numeric),
        ("tcp6_syn_recv", DataType::Numeric),
        ("tcp6_fin_wait1", DataType::Numeric),
        ("tcp6_fin_wait2", DataType::Numeric),
        ("tcp6_time_wait", DataType::Numeric),
        ("tcp6_close", DataType::Numeric),
        ("tcp6_close_wait", DataType::Numeric),
        ("tcp6_last_ack", DataType::Numeric),
        ("tcp6_listen", DataType::Numeric),
        ("tcp6_closing", DataType::Numeric),
        ("tcp_new_syn_recv", DataType::Numeric),
        ("tcp_bound_inactive", DataType::Numeric),
        // unix
        ("unix_free", DataType::Numeric),
        ("unix_unconnected", DataType::Numeric),
        ("unix_connecting", DataType::Numeric),
        ("unix_connected", DataType::Numeric),
        ("unix_disconnecting", DataType::Numeric),
        ("unix_dgram", DataType::Numeric),
        ("unix_seq_packet", DataType::Numeric),
        //
        ("unknown", DataType::Numeric),
    ]));
    res.put(data_row(&sockets_info.to_vector()));
    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show per-pool counters for the anticipation + bounded burst create path.
/// Operators tune `scaling_max_parallel_creates` against the relative motion
/// of these counters between scrapes. The anticipation loop bounds itself
/// by the client's remaining `query_wait_timeout` minus a 500 ms reserve
/// for the create path.
pub async fn show_pool_scaling<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("user", DataType::Text),
        ("database", DataType::Text),
        ("inflight", DataType::Numeric),
        ("creates", DataType::Numeric),
        ("gate_waits", DataType::Numeric),
        ("gate_budget_ex", DataType::Numeric),
        ("antic_notify", DataType::Numeric),
        ("antic_timeout", DataType::Numeric),
        ("create_fallback", DataType::Numeric),
        ("replenish_def", DataType::Numeric),
    ];

    let mut res = BytesMut::new();
    res.put(row_description(&columns));

    let mut entries: Vec<_> = get_all_pools()
        .iter()
        .map(|(id, pool)| (id.clone(), pool.database.scaling_stats()))
        .collect();
    entries.sort_by(|a, b| (&a.0.db, &a.0.user).cmp(&(&b.0.db, &b.0.user)));

    for (id, snapshot) in entries {
        res.put(data_row(&[
            id.user.clone(),
            id.db.clone(),
            snapshot.inflight_creates.to_string(),
            snapshot.creates_started.to_string(),
            snapshot.burst_gate_waits.to_string(),
            snapshot.burst_gate_budget_exhausted.to_string(),
            snapshot.anticipation_wakes_notify.to_string(),
            snapshot.anticipation_wakes_timeout.to_string(),
            snapshot.create_fallback.to_string(),
            snapshot.replenish_deferred.to_string(),
        ]));
    }

    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Show pool coordinator status per database.
/// Displays connection limits, current usage, and cumulative counters.
pub async fn show_pool_coordinator<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("database", DataType::Text),
        ("max_db_conn", DataType::Numeric),
        ("current", DataType::Numeric),
        ("reserve_size", DataType::Numeric),
        ("reserve_used", DataType::Numeric),
        ("evictions", DataType::Numeric),
        ("reserve_acq", DataType::Numeric),
        ("exhaustions", DataType::Numeric),
    ];

    let mut res = BytesMut::new();
    res.put(row_description(&columns));

    let coordinators = COORDINATORS.load();
    let mut db_names: Vec<&String> = coordinators.keys().collect();
    db_names.sort();

    for db in db_names {
        if let Some(coordinator) = coordinators.get(db) {
            let stats = coordinator.stats();
            let config = coordinator.config();
            res.put(data_row(&[
                db.to_string(),
                config.max_db_connections.to_string(),
                stats.total_connections.to_string(),
                config.reserve_pool_size.to_string(),
                stats.reserve_in_use.to_string(),
                stats.evictions_total.to_string(),
                stats.reserve_acquisitions_total.to_string(),
                stats.exhaustions_total.to_string(),
            ]));
        }
    }

    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}
