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
use crate::pool::{get_all_pools, ClientServerMap};

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

/// Pause connection pools — blocks new backend connection acquisition.
/// Active transactions continue to work.
/// If `db` is Some, only pools for that database are paused.
pub async fn pause<T>(stream: &mut T, db: Option<String>) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let pools = get_all_pools();
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
