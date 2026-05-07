//! Admin commands implementation (reload, shutdown, pause, resume, reconnect).

use bytes::{BufMut, BytesMut};
use log::{error, info};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

use crate::admin::operations::{pause_now, reconnect_now, resume_now, AdminEffect};
use crate::config::{get_config, reload_config};
use crate::errors::Error;
use crate::messages::protocol::{command_complete, data_row, row_description};
use crate::messages::socket::write_all_half;
use crate::messages::types::DataType;
use crate::pool::ClientServerMap;

/// Reload the configuration file without restarting the process.
pub async fn reload<T>(stream: &mut T, client_server_map: ClientServerMap) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    info!("Reloading config");

    reload_config(client_server_map).await?;
    crate::admin::events::push_event("RELOAD", "config reloaded".to_string());

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

/// Map an [`AdminEffect`] to either a postgres-protocol error response
/// (for `NoMatchingDb`) or a `CommandComplete` reply (for `Applied`).
async fn render_effect<T>(stream: &mut T, command: &str, effect: AdminEffect) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    match effect {
        AdminEffect::NoMatchingDb { db } => {
            admin_error_response(stream, &format!("No pool for database \"{db}\""), "3D000").await
        }
        AdminEffect::Applied { .. } => {
            let mut res = BytesMut::new();
            res.put(command_complete(command));
            res.put_u8(b'Z');
            res.put_i32(5);
            res.put_u8(b'I');
            write_all_half(stream, &res).await
        }
    }
}

/// Pause connection pools — blocks new backend connection acquisition.
/// Active transactions continue to work.
/// If `db` is Some, only pools for that database are paused.
pub async fn pause<T>(stream: &mut T, db: Option<String>) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    render_effect(stream, "PAUSE", pause_now(db)).await
}

/// Resume connection pools — unblocks clients waiting due to PAUSE.
/// If `db` is Some, only pools for that database are resumed.
pub async fn resume<T>(stream: &mut T, db: Option<String>) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    render_effect(stream, "RESUME", resume_now(db)).await
}

/// Reconnect connection pools — bumps epoch and drains idle connections.
/// Active connections are rejected when returned to the pool.
/// If `db` is Some, only pools for that database are reconnected.
pub async fn reconnect<T>(stream: &mut T, db: Option<String>) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    render_effect(stream, "RECONNECT", reconnect_now(db)).await
}
