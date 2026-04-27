//! Transport descriptor used by the client authentication pipeline.
//!
//! Replaces the `(SocketAddr, ssl: bool, is_unix: bool)` triplet that used
//! to be threaded through the HBA matcher and `Client::startup`. Having a
//! single enum removes the "two booleans in a row" footgun and gives us a
//! natural place to hang transport-specific helpers like a display string
//! for logs.

use std::net::SocketAddr;

/// How a client reached the pooler.
#[derive(Debug, Clone)]
pub enum ClientTransport {
    /// Classic TCP (optionally over TLS).
    Tcp {
        peer: SocketAddr,
        /// True when the client completed the TLS handshake before sending
        /// its startup packet. Drives hostssl rule matching and the
        /// `ClientStats::is_tls` counter.
        ssl: bool,
    },
    /// Unix domain socket. Peer address is not meaningful for these
    /// connections — the kernel does not expose a remote endpoint and
    /// `SO_PEERCRED` is not currently threaded through.
    Unix,
}

impl ClientTransport {
    /// True when the client is connected over a TLS-upgraded TCP socket.
    pub fn is_tls(&self) -> bool {
        matches!(self, ClientTransport::Tcp { ssl: true, .. })
    }

    /// True when the client is connected over a Unix domain socket.
    pub fn is_unix(&self) -> bool {
        matches!(self, ClientTransport::Unix)
    }

    /// Short display string used in logs and in `ClientStats` / `SHOW
    /// CLIENTS` rows. TCP clients carry their `peer.to_string()`; Unix
    /// clients render as `unix:` so operators can tell them apart from
    /// localhost TCP at a glance.
    pub fn peer_display(&self) -> String {
        match self {
            ClientTransport::Tcp { peer, .. } => peer.to_string(),
            ClientTransport::Unix => "unix:".to_string(),
        }
    }

    /// IP that the HBA matcher should use when checking `host`/`hostssl`
    /// rules. Unix transport has no meaningful IP, so we return a sentinel
    /// loopback value — the matcher ignores the IP for Unix clients
    /// anyway (see `src/auth/hba.rs`).
    pub fn hba_ip(&self) -> std::net::IpAddr {
        match self {
            ClientTransport::Tcp { peer, .. } => peer.ip(),
            ClientTransport::Unix => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn tcp_is_tls_reflects_ssl_flag() {
        let peer = SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 5432));
        assert!(!ClientTransport::Tcp { peer, ssl: false }.is_tls());
        assert!(ClientTransport::Tcp { peer, ssl: true }.is_tls());
        assert!(!ClientTransport::Tcp { peer, ssl: true }.is_unix());
    }

    #[test]
    fn unix_is_unix_and_never_tls() {
        assert!(ClientTransport::Unix.is_unix());
        assert!(!ClientTransport::Unix.is_tls());
    }

    #[test]
    fn peer_display_distinguishes_transports() {
        let peer = SocketAddr::from((Ipv4Addr::new(127, 0, 0, 1), 54321));
        assert_eq!(
            ClientTransport::Tcp { peer, ssl: false }.peer_display(),
            "127.0.0.1:54321"
        );
        assert_eq!(ClientTransport::Unix.peer_display(), "unix:");
    }

    #[test]
    fn hba_ip_for_unix_is_loopback_sentinel() {
        // The HBA matcher drops the IP entirely for Unix clients, so the
        // exact value does not matter — but we pin loopback here so a
        // regression is easy to spot.
        assert_eq!(
            ClientTransport::Unix.hba_ip(),
            std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
    }
}
