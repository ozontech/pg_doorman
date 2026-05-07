//! TCP bind and accept loop. The listener owns the lifetime of the
//! [`WebServerOptions`] slot — it seeds the slot on bind so every spawned
//! connection task reads the same reload-aware view via
//! [`current_options`].

use std::net::SocketAddr;
use std::sync::Arc;

use log::{error, info};
use tokio::net::{TcpListener, TcpSocket};

use super::http::handle_connection;
use super::state::{current_options, install_options, WebServerOptions};

/// Bind the listener synchronously and return it. Used by callers that
/// want to fail fast when the configured port is taken: the daemon's
/// readiness signal must wait until the web subsystem is verifiably
/// listening, otherwise systemd / a binary-upgrade parent treats the
/// pooler as healthy while `/metrics` and the UI are silently down.
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

/// Test-only convenience that binds and serves in one call. Production
/// code uses [`bind_web_listener`] + [`serve_on`] so a port collision
/// fails the whole startup instead of leaving the listener task
/// panicked behind a successful readiness signal. Gated on `cfg(test)`
/// so external embedders cannot trip the panic by accident.
#[cfg(test)]
pub(crate) async fn start_web_server(host: &str, opts: WebServerOptions) {
    let listener = bind_web_listener(host)
        .unwrap_or_else(|e| panic!("Failed to bind web listener on {host}: {e}"));
    serve_on(listener, opts).await;
}

/// Drive the accept loop on a pre-bound listener. Used by both
/// [`start_web_server`] and the production startup path that binds
/// synchronously before spawning.
pub async fn serve_on(listener: TcpListener, opts: WebServerOptions) {
    install_options(Arc::new(opts));

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let opts = current_options();
                tokio::spawn(async move {
                    handle_connection(stream, opts).await;
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}
