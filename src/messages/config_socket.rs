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
        Err(err) => error!("failed to set SO_LINGER on Unix socket: {err}"),
    }
    match sock_ref.set_send_buffer_size(conf.general.unix_socket_buffer_size.as_usize()) {
        Ok(_) => {}
        Err(err) => error!("failed to set send buffer size on Unix socket: {err}"),
    }
    match sock_ref.set_recv_buffer_size(conf.general.unix_socket_buffer_size.as_usize()) {
        Ok(_) => {}
        Err(err) => error!("failed to set recv buffer size on Unix socket: {err}"),
    }
}

pub fn configure_tcp_socket_for_cancel(stream: &TcpStream) {
    let sock_ref = SockRef::from(stream);
    match sock_ref.set_linger(None) {
        Ok(_) => {}
        Err(err) => error!("failed to set SO_LINGER(none) on cancel TCP socket: {err}"),
    }
    match sock_ref.set_tcp_nodelay(false) {
        Ok(_) => {}
        Err(err) => error!("failed to disable TCP_NODELAY on cancel TCP socket: {err}"),
    }
}

/// Configure TCP socket parameters.
pub fn configure_tcp_socket(stream: &TcpStream) {
    let sock_ref = SockRef::from(stream);
    let conf = get_config();

    match sock_ref.set_linger(Some(Duration::from_secs(conf.general.tcp_so_linger))) {
        Ok(_) => {}
        Err(err) => error!("failed to set SO_LINGER on TCP socket: {err}"),
    }

    match sock_ref.set_tcp_nodelay(conf.general.tcp_no_delay) {
        Ok(_) => {}
        Err(err) => error!("failed to set TCP_NODELAY on TCP socket: {err}"),
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
                Err(err) => error!("failed to set TCP keepalive parameters on socket: {err}"),
            }
        }
        Err(err) => error!("failed to enable SO_KEEPALIVE on TCP socket: {err}"),
    }

    // TCP_USER_TIMEOUT is only supported on Linux
    #[cfg(target_os = "linux")]
    if conf.general.tcp_user_timeout > 0 {
        match sock_ref
            .set_tcp_user_timeout(Some(Duration::from_secs(conf.general.tcp_user_timeout)))
        {
            Ok(_) => (),
            Err(err) => error!("failed to set TCP_USER_TIMEOUT on socket: {err}"),
        }
    }
}
