use std::time::Duration;

use log::{debug, error, log_enabled, Level};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::{TcpStream, UnixStream};

use crate::config::{get_config, Config};

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

    configure_tcp_socket_without_linger(&sock_ref, &conf, "TCP socket");
}

/// Configure accepted web TCP socket parameters.
///
/// Web HTTP sockets must not inherit the pooler client `SO_LINGER` policy:
/// `tcp_so_linger = 0` can turn a normal HTTP close into a reset.
pub fn configure_web_tcp_socket(stream: &TcpStream) {
    let sock_ref = SockRef::from(stream);
    let conf = get_config();

    match sock_ref.set_tcp_nodelay(conf.general.tcp_no_delay) {
        Ok(_) => {}
        Err(err) => error!("failed to set TCP_NODELAY on web TCP socket: {err}"),
    }

    configure_tcp_socket_without_linger(&sock_ref, &conf, "web TCP socket");
}

fn configure_tcp_socket_without_linger(sock_ref: &SockRef<'_>, conf: &Config, label: &str) {
    // Opt-in SO_RCVBUF/SO_SNDBUF. A non-zero value disables Linux TCP
    // autotuning for this socket and sets fixed send/receive buffer
    // limits. Linux doubles the requested values internally and may
    // clamp them by net.core.rmem_max / net.core.wmem_max.
    //
    // SIGHUP does not resize sockets that are already open.
    let buffer_size = conf.general.tcp_socket_buffer_size.as_usize();
    if buffer_size > 0 {
        match sock_ref.set_send_buffer_size(buffer_size) {
            Ok(_) => {
                // `net.core.wmem_max` silently caps the requested value;
                // the kernel also doubles it internally (man 7 socket).
                // Surface the kernel-side number so operators can verify
                // their sysctl ceiling is not below the configured size.
                if log_enabled!(Level::Debug) {
                    if let Ok(applied) = sock_ref.send_buffer_size() {
                        debug!(
                            "{label}: SO_SNDBUF requested={buffer_size} bytes, kernel applied={applied} bytes \
                             (kernel doubles; ceiling: net.core.wmem_max)"
                        );
                    }
                }
            }
            Err(err) => error!("failed to set SO_SNDBUF on {label}: {err}"),
        }
        match sock_ref.set_recv_buffer_size(buffer_size) {
            Ok(_) => {
                if log_enabled!(Level::Debug) {
                    if let Ok(applied) = sock_ref.recv_buffer_size() {
                        debug!(
                            "{label}: SO_RCVBUF requested={buffer_size} bytes, kernel applied={applied} bytes \
                             (kernel doubles; ceiling: net.core.rmem_max)"
                        );
                    }
                }
            }
            Err(err) => error!("failed to set SO_RCVBUF on {label}: {err}"),
        }
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
                Err(err) => error!("failed to set TCP keepalive parameters on {label}: {err}"),
            }
        }
        Err(err) => error!("failed to enable SO_KEEPALIVE on {label}: {err}"),
    }

    // TCP_USER_TIMEOUT is only supported on Linux
    #[cfg(target_os = "linux")]
    if conf.general.tcp_user_timeout > 0 {
        match sock_ref
            .set_tcp_user_timeout(Some(Duration::from_secs(conf.general.tcp_user_timeout)))
        {
            Ok(_) => (),
            Err(err) => error!("failed to set TCP_USER_TIMEOUT on {label}: {err}"),
        }
    }
}
