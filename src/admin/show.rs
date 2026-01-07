//! Admin SHOW commands implementation.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use bytes::{BufMut, BytesMut};
use tokio::time::Instant;

use crate::config::{get_config, VERSION};
use crate::errors::Error;
use crate::messages::protocol::{command_complete, data_row, row_description};
use crate::messages::socket::write_all_half;
use crate::messages::types::DataType;
use crate::pool::get_all_pools;
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
    for (_, _) in get_all_pools() {
        databases += 1; // One db per pool
        users += 1; // One user per pool
    }
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    res.put(data_row(&vec![
        "databases".to_string(),
        databases.to_string(),
    ]));
    res.put(data_row(&vec!["users".to_string(), users.to_string()]));
    res.put(data_row(&vec!["pools".to_string(), databases.to_string()]));
    res.put(data_row(&vec![
        "free_clients".to_string(),
        client_stats
            .keys()
            .filter(|client_id| client_stats.get(client_id).unwrap().state() == CLIENT_STATE_IDLE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&vec![
        "used_clients".to_string(),
        client_stats
            .keys()
            .filter(|client_id| client_stats.get(client_id).unwrap().state() == CLIENT_STATE_ACTIVE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&vec![
        "login_clients".to_string(),
        "0".to_string(),
    ]));
    res.put(data_row(&vec![
        "free_servers".to_string(),
        server_stats
            .keys()
            .filter(|server_id| server_stats.get(server_id).unwrap().state() == SERVER_STATE_IDLE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&vec![
        "used_servers".to_string(),
        server_stats
            .keys()
            .filter(|server_id| server_stats.get(server_id).unwrap().state() == SERVER_STATE_ACTIVE)
            .count()
            .to_string(),
    ]));
    res.put(data_row(&vec!["dns_names".to_string(), "0".to_string()]));
    res.put(data_row(&vec!["dns_zones".to_string(), "0".to_string()]));
    res.put(data_row(&vec!["dns_queries".to_string(), "0".to_string()]));
    res.put(data_row(&vec!["dns_pending".to_string(), "0".to_string()]));
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
    res.put(data_row(&vec![format!("PgDoorman {}", VERSION)]));
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
    let help_items = [
        "SHOW HELP|CONFIG|DATABASES|POOLS|POOLS_EXTENDED|CLIENTS|SERVERS|USERS|VERSION",
        "SHOW LISTS",
        "SHOW CONNECTIONS",
        "SHOW STATS",
        "RELOAD",
        "SHUTDOWN",
    ];
    let mut res = BytesMut::new();
    res.put(row_description(&columns));
    for item in help_items {
        res.put(data_row(&vec![item.to_string()]));
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
    for (_, pool) in get_all_pools() {
        let pool_config = pool.settings.clone();
        let database_name = &pool.address().database;
        let address = pool.address();
        let pool_state = pool.pool_state();
        res.put(data_row(&vec![
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
            format!("{:#010X}", client.client_id()),
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
            Instant::now()
                .duration_since(client.connect_time())
                .as_secs()
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
        let application_name = server.application_name.read();
        let row = vec![
            format!("{:#010X}", server.server_id()),
            server.process_id().to_string(),
            server.pool_name(),
            server.username(),
            application_name.clone(),
            server.state_to_string(),
            server.wait_to_string(),
            server.transaction_count.load(Ordering::Relaxed).to_string(),
            server.query_count.load(Ordering::Relaxed).to_string(),
            server.bytes_sent.load(Ordering::Relaxed).to_string(),
            server.bytes_received.load(Ordering::Relaxed).to_string(),
            Instant::now()
                .duration_since(server.connect_time())
                .as_secs()
                .to_string(),
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
    for (user_pool, pool) in get_all_pools() {
        let pool_config = &pool.settings;
        res.put(data_row(&vec![
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
