//! Resolve the real client IP for the access log when the listener
//! sits behind a trusted reverse proxy.
//!
//! The HTTP/1.1 layer hands us the TCP peer it accepted from. When that
//! peer is in the configured `[web].trusted_proxies` list, we walk the
//! `X-Forwarded-For` chain right-to-left and return the first IP that
//! is NOT itself trusted — that's the client's real address. When the
//! peer is not trusted, we ignore the headers (an untrusted client
//! could otherwise spoof anything) and return the peer.
//!
//! `Forwarded:` (RFC 7239) is parsed first when present; `X-Forwarded-For`
//! is the fallback. We only read the `for=` directive of `Forwarded`,
//! ignoring `host=`/`proto=`/`by=`.

use std::net::{IpAddr, SocketAddr};

use ipnet::IpNet;

/// Returns the rendered `peer=` value for the access log.
///
/// `peer_addr` is the raw TCP peer; `xff` and `forwarded` are the raw
/// header values (case-insensitive). When `peer_addr` is in
/// `trusted_proxies`, the function walks the chain to find the real
/// client. Otherwise the headers are ignored.
pub fn render_peer(
    peer_addr: Option<SocketAddr>,
    xff: Option<&str>,
    forwarded: Option<&str>,
    trusted_proxies: &[IpNet],
) -> String {
    let Some(peer) = peer_addr else {
        return "-".to_string();
    };
    if !is_trusted(peer.ip(), trusted_proxies) {
        return peer.to_string();
    }

    if let Some(real) = walk_forwarded(forwarded, trusted_proxies) {
        return real.to_string();
    }
    if let Some(real) = walk_xff(xff, trusted_proxies) {
        return real.to_string();
    }
    peer.to_string()
}

fn is_trusted(addr: IpAddr, trusted: &[IpNet]) -> bool {
    trusted.iter().any(|net| net.contains(&addr))
}

/// Walks `X-Forwarded-For` right-to-left, skipping trusted IPs.
/// Returns the first untrusted address found, or `None`.
fn walk_xff(xff: Option<&str>, trusted: &[IpNet]) -> Option<IpAddr> {
    let chain = xff?;
    for entry in chain.rsplit(',') {
        let candidate = entry.trim();
        if candidate.is_empty() {
            continue;
        }
        let ip = candidate.parse::<IpAddr>().ok()?;
        if !is_trusted(ip, trusted) {
            return Some(ip);
        }
    }
    None
}

/// Walks RFC 7239 `Forwarded:` right-to-left, looking for `for=`
/// values. Strips IPv6 brackets and the optional `:port` suffix.
fn walk_forwarded(forwarded: Option<&str>, trusted: &[IpNet]) -> Option<IpAddr> {
    let chain = forwarded?;
    for hop in chain.rsplit(',') {
        for kv in hop.split(';') {
            let kv = kv.trim();
            let Some(value) = kv.strip_prefix("for=").or_else(|| kv.strip_prefix("For=")) else {
                continue;
            };
            let cleaned = value.trim_matches('"');
            // IPv6 addresses come bracketed: `for="[2001:db8::1]:port"`.
            let cleaned = cleaned
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or(cleaned);
            // Drop the optional `:port` suffix on IPv4.
            let candidate = match cleaned.parse::<IpAddr>() {
                Ok(ip) => ip,
                Err(_) => match cleaned.rsplit_once(':') {
                    Some((host, _port)) => match host.parse::<IpAddr>() {
                        Ok(ip) => ip,
                        Err(_) => continue,
                    },
                    None => continue,
                },
            };
            if !is_trusted(candidate, trusted) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn net(s: &str) -> IpNet {
        IpNet::from_str(s).unwrap()
    }

    fn sock(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn untrusted_peer_keeps_peer() {
        let out = render_peer(
            Some(sock("203.0.113.1:443")),
            Some("10.0.0.5"),
            None,
            &[net("10.0.0.0/8")],
        );
        assert_eq!(out, "203.0.113.1:443");
    }

    #[test]
    fn trusted_peer_walks_xff() {
        let out = render_peer(
            Some(sock("10.0.0.1:443")),
            Some("203.0.113.7, 10.0.0.5"),
            None,
            &[net("10.0.0.0/8")],
        );
        assert_eq!(out, "203.0.113.7");
    }

    #[test]
    fn trusted_peer_skips_trusted_in_xff() {
        let out = render_peer(
            Some(sock("10.0.0.1:443")),
            // Two trusted proxies, then the real client at the head.
            Some("198.51.100.42, 10.0.0.4, 10.0.0.5"),
            None,
            &[net("10.0.0.0/8")],
        );
        assert_eq!(out, "198.51.100.42");
    }

    #[test]
    fn falls_back_to_peer_when_chain_is_all_trusted() {
        let out = render_peer(
            Some(sock("10.0.0.1:443")),
            Some("10.0.0.5"),
            None,
            &[net("10.0.0.0/8")],
        );
        assert_eq!(out, "10.0.0.1:443");
    }

    #[test]
    fn forwarded_for_takes_precedence_over_xff() {
        let out = render_peer(
            Some(sock("10.0.0.1:443")),
            Some("198.51.100.42"),
            Some(r#"for="203.0.113.7";proto=https"#),
            &[net("10.0.0.0/8")],
        );
        assert_eq!(out, "203.0.113.7");
    }

    #[test]
    fn forwarded_for_handles_ipv6_brackets() {
        let out = render_peer(
            Some(sock("10.0.0.1:443")),
            None,
            Some(r#"for="[2001:db8::1]""#),
            &[net("10.0.0.0/8")],
        );
        assert_eq!(out, "2001:db8::1");
    }

    #[test]
    fn no_peer_addr_returns_dash() {
        let out = render_peer(None, None, None, &[]);
        assert_eq!(out, "-");
    }

    #[test]
    fn empty_xff_entry_is_ignored() {
        let out = render_peer(
            Some(sock("10.0.0.1:443")),
            Some(", 198.51.100.42"),
            None,
            &[net("10.0.0.0/8")],
        );
        assert_eq!(out, "198.51.100.42");
    }
}
