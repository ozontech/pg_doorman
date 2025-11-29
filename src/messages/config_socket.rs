// Standard library imports
use std::time::Duration;

// External crate imports
use log::error;
use socket2::{SockRef, TcpKeepalive};
use tokio::net::{TcpStream, UnixStream};

// Internal crate imports
use crate::config::get_config;

/// Configure Unix socket parameters.
pub fn configure_unix_socket(stream: &UnixStream) {
    let sock_ref = SockRef::from(stream);
    let conf = get_config();

    match sock_ref.set_linger(Some(Duration::from_secs(conf.general.tcp_so_linger))) {
        Ok(_) => {}
        Err(err) => error!("Could not configure unix_so_linger for socket: {err}"),
    }
    match sock_ref.set_send_buffer_size(conf.general.unix_socket_buffer_size) {
        Ok(_) => {}
        Err(err) => error!("Could not configure set_send_buffer_size for socket: {err}"),
    }
    match sock_ref.set_recv_buffer_size(conf.general.unix_socket_buffer_size) {
        Ok(_) => {}
        Err(err) => error!("Could not configure set_recv_buffer_size for socket: {err}"),
    }
}

pub fn configure_tcp_socket_for_cancel(stream: &TcpStream) {
    let sock_ref = SockRef::from(stream);
    match sock_ref.set_linger(None) {
        Ok(_) => {}
        Err(err) => error!("Could not configure tcp_so_linger (node) for socket: {err}"),
    }
    match sock_ref.set_nodelay(false) {
        Ok(_) => {}
        Err(err) => error!("Could not configure no delay (false) for socket: {err}"),
    }
}

/// Configure TCP socket parameters.
pub fn configure_tcp_socket(stream: &TcpStream) {
    let sock_ref = SockRef::from(stream);
    let conf = get_config();

    match sock_ref.set_linger(Some(Duration::from_secs(conf.general.tcp_so_linger))) {
        Ok(_) => {}
        Err(err) => error!("Could not configure tcp_so_linger for socket: {err}"),
    }

    match sock_ref.set_nodelay(conf.general.tcp_no_delay) {
        Ok(_) => {}
        Err(err) => error!("Could not configure no delay for socket: {err}"),
    }

    match sock_ref.set_keepalive(true) {
        Ok(_) => {
            match sock_ref.set_tcp_keepalive(
                &TcpKeepalive::new()
                    .with_interval(Duration::from_secs(conf.general.tcp_keepalives_interval))
                    .with_retries(conf.general.tcp_keepalives_count)
                    .with_time(Duration::from_secs(conf.general.tcp_keepalives_idle)),
            ) {
                Ok(_) => (),
                Err(err) => error!("Could not configure tcp_keepalive for socket: {err}"),
            }
        }
        Err(err) => error!("Could not configure socket: {err}"),
    }
}
