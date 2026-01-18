//! Admin commands implementation (reload, shutdown).

use bytes::{BufMut, BytesMut};
use log::{error, info};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

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
