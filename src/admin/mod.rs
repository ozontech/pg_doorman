//! Admin interface for PgDoorman.
//!
//! This module provides administrative commands for managing the connection pooler,
//! including SHOW commands for statistics and RELOAD/SHUTDOWN commands.

mod commands;
mod show;

use bytes::{Buf, BufMut, BytesMut};
use log::{debug, warn};

use crate::app::log_level;
use crate::errors::Error;
use crate::messages::protocol::{command_complete, data_row, error_response, row_description};
use crate::messages::types::DataType;
use crate::messages::write_all_half;
use crate::pool::ClientServerMap;

/// Canonical list of SHOW subcommands. Single source of truth for:
/// - SHOW dispatch (match arms below)
/// - SHOW HELP output (show.rs)
/// - psql tab-completion (handle_tab_completion)
pub(crate) const SHOW_SUBCOMMANDS: &[&str] = &[
    "help",
    "config",
    "databases",
    "pools",
    "pools_extended",
    "pools_memory",
    "pool_coordinator",
    "pool_scaling",
    "prepared_statements",
    "clients",
    "servers",
    "connections",
    "stats",
    "version",
    "users",
    "auth_query",
    "log_level",
    "lists",
    #[cfg(target_os = "linux")]
    "sockets",
];

#[cfg(not(windows))]
use commands::upgrade;
use commands::{pause, reconnect, reload, resume, shutdown};
#[cfg(target_os = "linux")]
use show::show_sockets;
use show::{
    show_auth_query, show_clients, show_config, show_connections, show_databases, show_help,
    show_lists, show_log_level, show_pool_coordinator, show_pool_scaling, show_pools,
    show_pools_extended, show_pools_memory, show_prepared_statements, show_servers, show_stats,
    show_users, show_version,
};

/// Handle admin client.
pub async fn handle_admin<T>(
    stream: &mut T,
    mut query: BytesMut,
    client_server_map: ClientServerMap,
) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let code = query.get_u8() as char;

    if code != 'Q' {
        return Err(Error::ProtocolSyncError(format!(
            "Invalid code, expected 'Q' but got '{code}'"
        )));
    }

    let len = query.get_i32() as usize;
    let query = String::from_utf8_lossy(&query[..len - 5]).to_string();

    debug!("Admin query: {query}");

    // Intercept psql tab-completion queries to pg_catalog.pg_settings
    if query.contains("pg_catalog.pg_settings") {
        return handle_tab_completion(stream, &query).await;
    }

    let query_parts: Vec<&str> = query.trim_end_matches(';').split_whitespace().collect();

    match query_parts[0].to_ascii_uppercase().as_str() {
        "SET" => set_command(stream, &query_parts).await,
        "RELOAD" => reload(stream, client_server_map).await,
        "SHUTDOWN" => shutdown(stream).await,
        #[cfg(not(windows))]
        "UPGRADE" => upgrade(stream).await,
        "PAUSE" => {
            let db = query_parts.get(1).map(|s| s.to_string());
            pause(stream, db).await
        }
        "RESUME" => {
            let db = query_parts.get(1).map(|s| s.to_string());
            resume(stream, db).await
        }
        "RECONNECT" => {
            let db = query_parts.get(1).map(|s| s.to_string());
            reconnect(stream, db).await
        }
        "SHOW" => {
            if query_parts.len() != 2 {
                warn!("unsupported admin subcommand for SHOW: {query_parts:?}");
                error_response(
                    stream,
                    "Unsupported query against the admin database, please use SHOW HELP for a list of supported subcommands",
                    "58000",
                )
                .await
            } else {
                match query_parts[1].to_ascii_uppercase().as_str() {
                    "HELP" => show_help(stream).await,
                    "CONFIG" => show_config(stream).await,
                    "DATABASES" => show_databases(stream).await,
                    "LISTS" => show_lists(stream).await,
                    "POOLS" => show_pools(stream).await,
                    "POOLS_EXTENDED" => show_pools_extended(stream).await,
                    "POOLS_MEMORY" | "POOL_MEMORY" => show_pools_memory(stream).await,
                    "PREPARED_STATEMENTS" => show_prepared_statements(stream).await,
                    "CLIENTS" => show_clients(stream).await,
                    "SERVERS" => show_servers(stream).await,
                    "CONNECTIONS" => show_connections(stream).await,
                    "STATS" => show_stats(stream).await,
                    "VERSION" => show_version(stream).await,
                    "USERS" => show_users(stream).await,
                    "AUTH_QUERY" => show_auth_query(stream).await,
                    "POOL_COORDINATOR" => show_pool_coordinator(stream).await,
                    "POOL_SCALING" => show_pool_scaling(stream).await,
                    "LOG_LEVEL" => show_log_level(stream).await,
                    #[cfg(target_os = "linux")]
                    "SOCKETS" => show_sockets(stream).await,
                    _ => {
                        warn!(
                            "unsupported admin subcommand for SHOW: {}",
                            query_parts[1].to_ascii_uppercase().as_str()
                        );
                        error_response(
                            stream,
                            "Unsupported SHOW query against the admin database",
                            "58000",
                        )
                        .await
                    }
                }
            }
        }
        _ => {
            warn!(
                "unsupported admin command: {}",
                query_parts[0].to_ascii_uppercase().as_str()
            );
            error_response(
                stream,
                "Unsupported query against the admin database",
                "58000",
            )
            .await
        }
    }
}

/// Respond to psql tab-completion queries that reference pg_catalog.pg_settings.
/// psql sends these automatically when the user presses TAB after SET or SHOW.
async fn handle_tab_completion<T>(stream: &mut T, query: &str) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let query_lower = query.to_ascii_lowercase();
    let mut res = BytesMut::new();

    if query_lower.contains("unnest(enumvals)") {
        // SET log_level = <TAB> — return enum values for the parameter
        res.put(row_description(&vec![("val", DataType::Text)]));
        for val in &["error", "warn", "info", "debug", "trace", "off", "default"] {
            res.put(data_row(&[val.to_string()]));
        }
    } else if query_lower.contains("vartype") {
        // Type lookup — psql checks if parameter is enum/bool/string
        res.put(row_description(&vec![("vartype", DataType::Text)]));
        res.put(data_row(&["enum".to_string()]));
    } else if query_lower.contains("context") {
        // SET <TAB> — return settable parameters (filtered by context)
        res.put(row_description(&vec![("name", DataType::Text)]));
        res.put(data_row(&["log_level".to_string()]));
    } else {
        // SHOW <TAB> — return all SHOW subcommands from the canonical list
        res.put(row_description(&vec![("name", DataType::Text)]));
        for name in SHOW_SUBCOMMANDS {
            res.put(data_row(&[name.to_string()]));
        }
    }

    res.put(command_complete("SELECT"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Handle SET command. Currently supports: SET log_level = '<filter>'
async fn set_command<T>(stream: &mut T, query_parts: &[&str]) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    // Parse: SET log_level = 'value' or SET log_level 'value' or SET log_level value
    if query_parts.len() < 3 {
        return error_response(stream, "SET requires: SET <parameter> = '<value>'", "42601").await;
    }

    let param = query_parts[1].to_ascii_uppercase();
    // Collect value: skip "=" if present, join remaining parts
    let value_parts: Vec<&str> = query_parts[2..]
        .iter()
        .filter(|s| **s != "=")
        .copied()
        .collect();
    let value = value_parts.join(" ");
    let value = value.trim().trim_matches('\'').trim_matches('"');

    match param.as_str() {
        "LOG_LEVEL" => match log_level::set_log_level(value) {
            Ok(()) => {
                log::info!("SET log_level = '{}'", log_level::get_log_level());
                let mut res = BytesMut::new();
                res.put(command_complete("SET"));
                res.put_u8(b'Z');
                res.put_i32(5);
                res.put_u8(b'I');
                write_all_half(stream, &res).await
            }
            Err(err) => error_response(stream, &err, "42601").await,
        },
        _ => {
            error_response(
                stream,
                &format!("Unknown SET parameter: {param}. Supported: log_level"),
                "42601",
            )
            .await
        }
    }
}
