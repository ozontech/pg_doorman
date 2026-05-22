//! TCP bind and accept loop. The listener owns the lifetime of the
//! [`WebServerOptions`] slot — it seeds the slot on bind so every spawned
//! connection task reads the same reload-aware view via
//! [`current_options`].

use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{error, info};
use tokio::net::{TcpListener, TcpSocket};

use crate::messages::configure_web_tcp_socket;

use super::http::handle_connection;
use super::state::{current_options, install_options, WebServerOptions};

static WEB_ACCEPT_RESOURCE_LOG_LAST: AtomicI64 = AtomicI64::new(0);
const WEB_ACCEPT_RESOURCE_LOG_INTERVAL_SECS: i64 = 5;

#[cfg(unix)]
fn is_fd_exhaustion_io(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE),)
}

#[cfg(not(unix))]
fn is_fd_exhaustion_io(_e: &std::io::Error) -> bool {
    false
}

fn should_log_web_accept_resource_now() -> bool {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let last = WEB_ACCEPT_RESOURCE_LOG_LAST.load(Ordering::Relaxed);
    now_secs.saturating_sub(last) >= WEB_ACCEPT_RESOURCE_LOG_INTERVAL_SECS
        && WEB_ACCEPT_RESOURCE_LOG_LAST
            .compare_exchange(last, now_secs, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

/// Bind synchronously so startup/readiness fails if the web port cannot bind.
pub fn bind_web_listener(host: &str) -> std::io::Result<TcpListener> {
    info!("binding web listener on {host}");
    let addr: SocketAddr = host.parse().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Failed to parse socket address '{host}': {e}"),
        )
    })?;

    let listen_socket = if addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }?;

    listen_socket.set_reuseaddr(true)?;
    listen_socket.set_reuseport(true)?;
    listen_socket.bind(addr)?;
    let listener = listen_socket.listen(1024)?;
    info!("web listener bound on {addr}");
    Ok(listener)
}

/// Drive the accept loop on a pre-bound listener.
pub async fn serve_on(listener: TcpListener, opts: WebServerOptions) {
    install_options(Arc::new(opts));

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                configure_web_tcp_socket(&stream);
                let opts = current_options();
                tokio::spawn(async move {
                    handle_connection(stream, opts).await;
                });
            }
            Err(e) => {
                if is_fd_exhaustion_io(&e) {
                    if should_log_web_accept_resource_now() {
                        error!(
                            "Failed to accept connection: {e} \
                             (process fd table exhausted; backing off)"
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                } else {
                    error!("Failed to accept connection: {e}");
                }
            }
        }
    }
}
