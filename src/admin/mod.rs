//! Admin interface for PgDoorman.
//!
//! This module provides administrative commands for managing the connection pooler,
//! including SHOW commands for statistics and RELOAD/SHUTDOWN commands.

mod commands;
mod show;

use bytes::{Buf, BytesMut};
use log::{debug, error};

use crate::errors::Error;
use crate::messages::protocol::error_response;
use crate::pool::ClientServerMap;

use commands::{reload, shutdown};
#[cfg(target_os = "linux")]
use show::show_sockets;
use show::{
    show_clients, show_config, show_connections, show_databases, show_help, show_lists, show_pools,
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

    let query_parts: Vec<&str> = query.trim_end_matches(';').split_whitespace().collect();

    match query_parts[0].to_ascii_uppercase().as_str() {
        "RELOAD" => reload(stream, client_server_map).await,
        "SHUTDOWN" => shutdown(stream).await,
        "SHOW" => {
            if query_parts.len() != 2 {
                error!("unsupported admin subcommand for SHOW: {query_parts:?}");
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
                    #[cfg(target_os = "linux")]
                    "SOCKETS" => show_sockets(stream).await,
                    _ => {
                        error!(
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
            error!(
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
