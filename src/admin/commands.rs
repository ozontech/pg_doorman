//! Admin commands implementation (reload, shutdown, pause, resume, reconnect).

use bytes::{BufMut, BytesMut};
use log::{error, info};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

use crate::config::{get_config, reload_config};
use crate::errors::Error;
use crate::messages::protocol::{command_complete, data_row, row_description};
use crate::messages::socket::write_all_half;
use crate::messages::types::DataType;
use crate::pool::{get_all_pools, ClientServerMap, PoolMap};

/// Reload the configuration file without restarting the process.
pub async fn reload<T>(stream: &mut T, client_server_map: ClientServerMap) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    info!("Reloading config");

    reload_config(client_server_map).await?;

    get_config().show();

    let mut res = BytesMut::new();

    res.put(command_complete("RELOAD"));

    // ReadyForQuery
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');

    write_all_half(stream, &res).await
}

/// Send response packets for shutdown.
pub async fn shutdown<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();

    res.put(row_description(&vec![("success", DataType::Text)]));

    let mut shutdown_success = "t";

    let pid = std::process::id();
    if signal::kill(Pid::from_raw(pid.try_into().unwrap()), Signal::SIGINT).is_err() {
        error!("Unable to send SIGINT to PID: {pid}");
        shutdown_success = "f";
    }

    res.put(data_row(&[shutdown_success.to_string()]));

    res.put(command_complete("SHUTDOWN"));

    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');

    write_all_half(stream, &res).await
}

/// Trigger binary upgrade via SIGUSR2 (graceful shutdown + spawn new process).
#[cfg(not(windows))]
pub async fn upgrade<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut res = BytesMut::new();

    res.put(row_description(&vec![("success", DataType::Text)]));

    let mut upgrade_success = "t";

    let pid = std::process::id();
    info!("UPGRADE command: sending SIGUSR2 to PID {pid}");
    if signal::kill(Pid::from_raw(pid.try_into().unwrap()), Signal::SIGUSR2).is_err() {
        error!("Unable to send SIGUSR2 to PID: {pid}");
        upgrade_success = "f";
    }

    res.put(data_row(&[upgrade_success.to_string()]));

    res.put(command_complete("UPGRADE"));

    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');

    write_all_half(stream, &res).await
}

/// Send an ERROR-severity response (non-fatal — keeps the admin session open).
async fn admin_error_response<T>(stream: &mut T, message: &str, code: &str) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let mut error = BytesMut::new();
    error.put_u8(b'S');
    error.put_slice(b"ERROR\0");
    error.put_u8(b'V');
    error.put_slice(b"ERROR\0");
    error.put_u8(b'C');
    error.put_slice(format!("{code}\0").as_bytes());
    error.put_u8(b'M');
    error.put_slice(format!("{message}\0").as_bytes());
    error.put_u8(0);

    let mut res = BytesMut::new();
    res.put_u8(b'E');
    res.put_i32(error.len() as i32 + 4);
    res.put(error);

    // ReadyForQuery — session stays open
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');

    write_all_half(stream, &res).await
}

/// Check that the specified database has at least one pool.
/// Returns `Ok(true)` if pools exist (or no db filter was given).
/// Returns `Ok(false)` after sending an error response if db was specified but no pools matched.
async fn check_db_has_pools<T>(
    stream: &mut T,
    db: &Option<String>,
    pools: &PoolMap,
) -> Result<bool, Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    if let Some(ref db_name) = db {
        if !pools.keys().any(|id| id.db == *db_name) {
            admin_error_response(
                stream,
                &format!("No pool for database \"{}\"", db_name),
                "3D000",
            )
            .await?;
            return Ok(false);
        }
    }
    Ok(true)
}

/// Pause connection pools — blocks new backend connection acquisition.
/// Active transactions continue to work.
/// If `db` is Some, only pools for that database are paused.
pub async fn pause<T>(stream: &mut T, db: Option<String>) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pools = get_all_pools();
    if !check_db_has_pools(stream, &db, &pools).await? {
        return Ok(());
    }
    for (identifier, pool) in pools.iter() {
        if let Some(ref db_name) = db {
            if identifier.db != *db_name {
                continue;
            }
        }
        pool.database.pause();
        info!("PAUSE: paused pool {}", identifier);
    }

    let mut res = BytesMut::new();
    res.put(command_complete("PAUSE"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Resume connection pools — unblocks clients waiting due to PAUSE.
/// If `db` is Some, only pools for that database are resumed.
pub async fn resume<T>(stream: &mut T, db: Option<String>) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pools = get_all_pools();
    if !check_db_has_pools(stream, &db, &pools).await? {
        return Ok(());
    }
    for (identifier, pool) in pools.iter() {
        if let Some(ref db_name) = db {
            if identifier.db != *db_name {
                continue;
            }
        }
        pool.database.resume();
        info!("RESUME: resumed pool {}", identifier);
    }

    let mut res = BytesMut::new();
    res.put(command_complete("RESUME"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}

/// Reconnect connection pools — bumps epoch and drains idle connections.
/// Active connections are rejected when returned to the pool.
/// If `db` is Some, only pools for that database are reconnected.
pub async fn reconnect<T>(stream: &mut T, db: Option<String>) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pools = get_all_pools();
    if !check_db_has_pools(stream, &db, &pools).await? {
        return Ok(());
    }
    for (identifier, pool) in pools.iter() {
        if let Some(ref db_name) = db {
            if identifier.db != *db_name {
                continue;
            }
        }
        let new_epoch = pool.database.reconnect();
        info!(
            "RECONNECT: reconnected pool {} (new epoch: {})",
            identifier, new_epoch
        );
    }

    let mut res = BytesMut::new();
    res.put(command_complete("RECONNECT"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}
