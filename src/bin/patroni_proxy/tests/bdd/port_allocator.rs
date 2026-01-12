use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicU16, Ordering};

/// Starting port for allocation (ephemeral port range)
const START_PORT: u16 = 30000;

/// Maximum port number to try
const MAX_PORT: u16 = 40000;

/// Global atomic counter for port allocation
static NEXT_PORT: AtomicU16 = AtomicU16::new(START_PORT);

/// Allocate a free port by atomically incrementing counter and verifying bind works.
/// This approach avoids race conditions in CI/CD where multiple tests run concurrently.
///
/// Returns a port number that was successfully bound (and immediately released).
pub fn allocate_port() -> u16 {
    loop {
        // Atomically get and increment the port counter
        let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);

        // Wrap around if we exceed max port
        if port >= MAX_PORT {
            NEXT_PORT.store(START_PORT, Ordering::SeqCst);
            continue;
        }

        // Try to bind to verify the port is actually free
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
        if TcpListener::bind(addr).is_ok() {
            // Port is free and we successfully bound to it
            // The listener is dropped here, releasing the port
            return port;
        }

        // Port is in use, try next one
    }
}

#[cfg(test)]
mod tests {
    use super::allocate_port;
    use std::net::{SocketAddr, TcpListener};

    #[test]
    fn test_allocate_port_returns_different_ports() {
        let port1 = allocate_port();
        let port2 = allocate_port();
        let port3 = allocate_port();

        assert_ne!(port1, port2);
        assert_ne!(port2, port3);
        assert_ne!(port1, port3);
    }

    #[test]
    fn test_allocated_port_is_bindable() {
        let port = allocate_port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        // Should be able to bind to the allocated port
        let listener = TcpListener::bind(addr);
        assert!(listener.is_ok());
    }
}
