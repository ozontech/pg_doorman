# Server-side TLS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable encrypted TLS connections from pg_doorman to PostgreSQL servers with five SSL modes, mTLS, TLS-aware cancel, and per-pool configuration.

**Architecture:** Add `ServerTlsMode` enum and `ServerTlsConfig` struct. Extend `StreamInner` with a `TCPTls` variant. Store resolved `Arc<ServerTlsConfig>` on `Address`. TLS handshake happens in `create_tcp_stream_inner()` after SSLRequest. Cancel requests inherit TLS state from the main connection.

**Tech Stack:** `native-tls` / `tokio-native-tls` (already in project), `pin-project-lite` for enum projection.

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/config/tls.rs` | Modify | Add `ServerTlsMode` enum, `ServerTlsConfig` struct, `build_server_connector()` |
| `src/config/general.rs` | Modify | Replace `server_tls`/`verify_server_certificate` with `server_tls_mode`/`server_tls_ca_cert`/`server_tls_certificate`/`server_tls_private_key` |
| `src/config/pool.rs` | Modify | Add per-pool TLS override fields |
| `src/config/mod.rs` | Modify | Validate server TLS config, build `ServerTlsConfig`, log it |
| `src/config/address.rs` | Modify | Add `server_tls: Arc<ServerTlsConfig>` field |
| `src/server/stream.rs` | Modify | Add `TCPTls` variant, change `create_tcp_stream_inner` signature and TLS handshake logic |
| `src/server/server_backend.rs` | Modify | Pass `address.server_tls` to stream creation, store `connected_with_tls`, update cancel call |
| `src/server/startup_cancel.rs` | Modify | Accept `ServerTlsConfig` + `connected_with_tls`, TLS handshake for cancel |
| `src/client/transaction.rs` | Modify | Pass TLS info through cancel path |
| `src/pool/mod.rs` | Modify | Build `ServerTlsConfig` when creating `Address`, store on address |
| `src/pool/dynamic.rs` | Modify | Build `ServerTlsConfig` for dynamic pool addresses |

---

### Task 1: ServerTlsMode enum and parsing

**Files:**
- Modify: `src/config/tls.rs:26-60`

- [ ] **Step 1: Write failing tests for ServerTlsMode parsing**

Add to the existing `#[cfg(test)] mod tests` in `src/config/tls.rs`:

```rust
#[test]
fn test_server_tls_mode_from_string() {
    assert_eq!(ServerTlsMode::from_string("disable").unwrap(), ServerTlsMode::Disable);
    assert_eq!(ServerTlsMode::from_string("prefer").unwrap(), ServerTlsMode::Prefer);
    assert_eq!(ServerTlsMode::from_string("require").unwrap(), ServerTlsMode::Require);
    assert_eq!(ServerTlsMode::from_string("verify-ca").unwrap(), ServerTlsMode::VerifyCa);
    assert_eq!(ServerTlsMode::from_string("verify-full").unwrap(), ServerTlsMode::VerifyFull);
    assert!(ServerTlsMode::from_string("invalid").is_err());
    assert!(ServerTlsMode::from_string("").is_err());
}

#[test]
fn test_server_tls_mode_display() {
    assert_eq!(ServerTlsMode::Disable.to_string(), "disable");
    assert_eq!(ServerTlsMode::Prefer.to_string(), "prefer");
    assert_eq!(ServerTlsMode::Require.to_string(), "require");
    assert_eq!(ServerTlsMode::VerifyCa.to_string(), "verify-ca");
    assert_eq!(ServerTlsMode::VerifyFull.to_string(), "verify-full");
}

#[test]
fn test_server_tls_mode_requires_ca() {
    assert!(!ServerTlsMode::Disable.requires_ca());
    assert!(!ServerTlsMode::Prefer.requires_ca());
    assert!(!ServerTlsMode::Require.requires_ca());
    assert!(ServerTlsMode::VerifyCa.requires_ca());
    assert!(ServerTlsMode::VerifyFull.requires_ca());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::tls::tests::test_server_tls_mode`
Expected: compilation errors — `ServerTlsMode` not defined.

- [ ] **Step 3: Implement ServerTlsMode**

Add after the existing `TLSMode` enum in `src/config/tls.rs`:

```rust
/// TLS mode for server-facing (backend) connections.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone)]
pub enum ServerTlsMode {
    /// Do not use TLS
    Disable,
    /// Try TLS, fall back to plain if server doesn't support it
    Prefer,
    /// Require TLS, fail if server doesn't support it
    Require,
    /// Require TLS and verify server certificate against CA
    VerifyCa,
    /// Require TLS, verify CA, and verify hostname matches certificate
    VerifyFull,
}

impl Default for ServerTlsMode {
    fn default() -> Self {
        ServerTlsMode::Prefer
    }
}

impl std::fmt::Display for ServerTlsMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerTlsMode::Disable => write!(f, "disable"),
            ServerTlsMode::Prefer => write!(f, "prefer"),
            ServerTlsMode::Require => write!(f, "require"),
            ServerTlsMode::VerifyCa => write!(f, "verify-ca"),
            ServerTlsMode::VerifyFull => write!(f, "verify-full"),
        }
    }
}

impl ServerTlsMode {
    pub fn from_string(s: &str) -> Result<Self, Error> {
        match s {
            "disable" => Ok(ServerTlsMode::Disable),
            "prefer" => Ok(ServerTlsMode::Prefer),
            "require" => Ok(ServerTlsMode::Require),
            "verify-ca" => Ok(ServerTlsMode::VerifyCa),
            "verify-full" => Ok(ServerTlsMode::VerifyFull),
            _ => Err(Error::BadConfig(format!("Invalid server_tls_mode: {s}"))),
        }
    }

    /// Whether this mode requires a CA certificate to be configured.
    pub fn requires_ca(&self) -> bool {
        matches!(self, ServerTlsMode::VerifyCa | ServerTlsMode::VerifyFull)
    }

    /// Whether this mode sends an SSLRequest to the server.
    pub fn sends_ssl_request(&self) -> bool {
        !matches!(self, ServerTlsMode::Disable)
    }

    /// Whether this mode requires the server to support TLS.
    pub fn requires_tls(&self) -> bool {
        matches!(
            self,
            ServerTlsMode::Require | ServerTlsMode::VerifyCa | ServerTlsMode::VerifyFull
        )
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config::tls::tests::test_server_tls_mode`
Expected: all 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/config/tls.rs
git commit -m "Add ServerTlsMode enum with parsing and display"
```

---

### Task 2: ServerTlsConfig and TlsConnector builder

**Files:**
- Modify: `src/config/tls.rs`

- [ ] **Step 1: Write failing test for build_server_connector**

Add to tests in `src/config/tls.rs`:

```rust
#[test]
fn test_server_tls_config_disable() {
    let config = ServerTlsConfig::new(ServerTlsMode::Disable, None, None, None).unwrap();
    assert_eq!(config.mode, ServerTlsMode::Disable);
    assert!(config.connector.is_none());
}

#[test]
fn test_server_tls_config_prefer_no_certs() {
    let config = ServerTlsConfig::new(ServerTlsMode::Prefer, None, None, None).unwrap();
    assert_eq!(config.mode, ServerTlsMode::Prefer);
    assert!(config.connector.is_some());
}

#[test]
fn test_server_tls_config_require_no_certs() {
    let config = ServerTlsConfig::new(ServerTlsMode::Require, None, None, None).unwrap();
    assert_eq!(config.mode, ServerTlsMode::Require);
    assert!(config.connector.is_some());
}

#[test]
fn test_server_tls_config_verify_ca_with_cert() {
    let ca_path = PathBuf::from("tests/data/ssl/root.crt");
    if !ca_path.exists() {
        return; // skip if test certs not available
    }
    let config =
        ServerTlsConfig::new(ServerTlsMode::VerifyCa, Some(&ca_path), None, None).unwrap();
    assert_eq!(config.mode, ServerTlsMode::VerifyCa);
    assert!(config.connector.is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::tls::tests::test_server_tls_config`
Expected: compilation error — `ServerTlsConfig` not defined.

- [ ] **Step 3: Implement ServerTlsConfig**

Add to `src/config/tls.rs`:

```rust
/// Resolved TLS configuration for server-facing connections.
/// Built once per pool during config load, shared via Arc.
#[derive(Debug)]
pub struct ServerTlsConfig {
    pub mode: ServerTlsMode,
    pub connector: Option<tokio_native_tls::TlsConnector>,
}

impl ServerTlsConfig {
    /// Build a ServerTlsConfig from mode and optional certificate paths.
    ///
    /// - `ca_cert`: CA certificate for verify-ca / verify-full
    /// - `client_cert` + `client_key`: client identity for mTLS
    pub fn new(
        mode: ServerTlsMode,
        ca_cert: Option<&Path>,
        client_cert: Option<&Path>,
        client_key: Option<&Path>,
    ) -> Result<Self, Error> {
        if mode == ServerTlsMode::Disable {
            return Ok(ServerTlsConfig {
                mode,
                connector: None,
            });
        }

        let mut builder = native_tls::TlsConnector::builder();
        builder.min_protocol_version(Some(Protocol::Tlsv12));

        match mode {
            ServerTlsMode::Prefer | ServerTlsMode::Require => {
                // Accept any certificate — no verification
                builder.danger_accept_invalid_certs(true);
                builder.danger_accept_invalid_hostnames(true);
            }
            ServerTlsMode::VerifyCa => {
                // Verify certificate chain but not hostname
                if let Some(ca_path) = ca_cert {
                    let ca = load_certificate(ca_path)?;
                    builder.add_root_certificate(ca);
                }
                builder.danger_accept_invalid_hostnames(true);
            }
            ServerTlsMode::VerifyFull => {
                // Verify certificate chain and hostname
                if let Some(ca_path) = ca_cert {
                    let ca = load_certificate(ca_path)?;
                    builder.add_root_certificate(ca);
                }
            }
            ServerTlsMode::Disable => unreachable!(),
        }

        // mTLS: present client certificate to server
        if let (Some(cert_path), Some(key_path)) = (client_cert, client_key) {
            let identity = load_identity(cert_path, key_path).map_err(|err| {
                Error::BadConfig(format!(
                    "Failed to load server TLS client identity from {} and {}: {}",
                    cert_path.display(),
                    key_path.display(),
                    err
                ))
            })?;
            builder.identity(identity);
        }

        let connector = builder
            .build()
            .map(tokio_native_tls::TlsConnector::from)
            .map_err(|err| {
                Error::BadConfig(format!("Failed to create server TLS connector: {err}"))
            })?;

        Ok(ServerTlsConfig {
            mode,
            connector: Some(connector),
        })
    }
}
```

- [ ] **Step 4: Add re-export in `src/config/mod.rs`**

Add `ServerTlsConfig` and `ServerTlsMode` to the `pub use` block:

In the existing line `pub use address::{Address, BackendAuthMethod, PoolMode};` area, the tls module is already `pub mod tls;`. Add a re-export:

```rust
pub use tls::{ServerTlsConfig, ServerTlsMode};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib config::tls::tests::test_server_tls_config`
Expected: all 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/config/tls.rs src/config/mod.rs
git commit -m "Add ServerTlsConfig with TlsConnector builder"
```

---

### Task 3: Config fields — General and Pool

**Files:**
- Modify: `src/config/general.rs:159-163, 392-452`
- Modify: `src/config/pool.rs:64-146, 316-339`

- [ ] **Step 1: Replace server_tls/verify_server_certificate in General**

In `src/config/general.rs`, replace the two fields:

```rust
// REMOVE these two fields:
//   pub server_tls: bool,
//   pub verify_server_certificate: bool,

// ADD these four fields (in the same location, before admin_username):
#[serde(default = "General::default_server_tls_mode")]
pub server_tls_mode: String,

#[serde(skip_serializing_if = "Option::is_none")]
pub server_tls_ca_cert: Option<String>,

#[serde(skip_serializing_if = "Option::is_none")]
pub server_tls_certificate: Option<String>,

#[serde(skip_serializing_if = "Option::is_none")]
pub server_tls_private_key: Option<String>,
```

Add the default method:

```rust
pub fn default_server_tls_mode() -> String {
    "prefer".to_string()
}
```

Update `Default for General`: replace `server_tls: false, verify_server_certificate: false,` with:

```rust
server_tls_mode: Self::default_server_tls_mode(),
server_tls_ca_cert: None,
server_tls_certificate: None,
server_tls_private_key: None,
```

- [ ] **Step 2: Add per-pool TLS override fields to Pool**

In `src/config/pool.rs`, add before the `auth_query` field (before the comment about TOML ordering):

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub server_tls_mode: Option<String>,

#[serde(skip_serializing_if = "Option::is_none")]
pub server_tls_ca_cert: Option<String>,

#[serde(skip_serializing_if = "Option::is_none")]
pub server_tls_certificate: Option<String>,

#[serde(skip_serializing_if = "Option::is_none")]
pub server_tls_private_key: Option<String>,
```

Update `Default for Pool` to add:

```rust
server_tls_mode: None,
server_tls_ca_cert: None,
server_tls_certificate: None,
server_tls_private_key: None,
```

- [ ] **Step 3: Fix all compilation errors**

The old fields `config.general.server_tls` and `config.general.verify_server_certificate` are referenced in:
- `src/server/server_backend.rs:732-733` — will be fixed in Task 6
- `src/config/general.rs` Default impl — already fixed above

For now, make it compile by temporarily using a placeholder in `server_backend.rs`:

Replace:
```rust
create_tcp_stream_inner(
    &address.host,
    address.port,
    config.general.server_tls,
    config.general.verify_server_certificate,
)
```
With:
```rust
create_tcp_stream_inner(
    &address.host,
    address.port,
    false,
    false,
)
```

- [ ] **Step 4: Run `cargo check`**

Run: `cargo check`
Expected: compiles without errors.

- [ ] **Step 5: Commit**

```bash
git add src/config/general.rs src/config/pool.rs src/server/server_backend.rs
git commit -m "Replace server_tls/verify_server_certificate with server_tls_mode config"
```

---

### Task 4: Config validation for server TLS

**Files:**
- Modify: `src/config/mod.rs:342-461`

- [ ] **Step 1: Add server TLS validation to Config::validate()**

In `src/config/mod.rs`, inside the `validate()` method, after the existing frontend TLS validation block (after the closing `}` at the end of the TLS section, around line 435), add:

```rust
// Validate server-facing TLS
{
    // Validate global server_tls_mode
    let global_mode =
        tls::ServerTlsMode::from_string(&self.general.server_tls_mode)?;

    if global_mode.requires_ca() && self.general.server_tls_ca_cert.is_none() {
        return Err(Error::BadConfig(format!(
            "server_tls_mode is '{global_mode}' but server_tls_ca_cert is not set"
        )));
    }

    // Certificate and key must be specified together
    match (
        &self.general.server_tls_certificate,
        &self.general.server_tls_private_key,
    ) {
        (Some(_), None) => {
            return Err(Error::BadConfig(
                "server_tls_certificate is set but server_tls_private_key is not"
                    .to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(Error::BadConfig(
                "server_tls_private_key is set but server_tls_certificate is not"
                    .to_string(),
            ));
        }
        _ => {}
    }

    // Validate that certificate files are readable at startup
    if global_mode != tls::ServerTlsMode::Disable {
        tls::ServerTlsConfig::new(
            global_mode,
            self.general.server_tls_ca_cert.as_deref().map(Path::new),
            self.general.server_tls_certificate.as_deref().map(Path::new),
            self.general.server_tls_private_key.as_deref().map(Path::new),
        )?;
    }

    // Validate per-pool overrides
    for (pool_name, pool_config) in &self.pools {
        let effective_mode = pool_config
            .server_tls_mode
            .as_deref()
            .unwrap_or(&self.general.server_tls_mode);
        let mode = tls::ServerTlsMode::from_string(effective_mode).map_err(|_| {
            Error::BadConfig(format!(
                "pool '{pool_name}': invalid server_tls_mode '{effective_mode}'"
            ))
        })?;

        let effective_ca = pool_config
            .server_tls_ca_cert
            .as_ref()
            .or(self.general.server_tls_ca_cert.as_ref());
        let effective_cert = pool_config
            .server_tls_certificate
            .as_ref()
            .or(self.general.server_tls_certificate.as_ref());
        let effective_key = pool_config
            .server_tls_private_key
            .as_ref()
            .or(self.general.server_tls_private_key.as_ref());

        if mode.requires_ca() && effective_ca.is_none() {
            return Err(Error::BadConfig(format!(
                "pool '{pool_name}': server_tls_mode is '{mode}' but no server_tls_ca_cert"
            )));
        }

        match (&effective_cert, &effective_key) {
            (Some(_), None) => {
                return Err(Error::BadConfig(format!(
                    "pool '{pool_name}': server_tls_certificate without server_tls_private_key"
                )));
            }
            (None, Some(_)) => {
                return Err(Error::BadConfig(format!(
                    "pool '{pool_name}': server_tls_private_key without server_tls_certificate"
                )));
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 2: Add server TLS info to Config::show()**

In the `show()` method, after the frontend TLS info block, add:

```rust
info!("Server TLS mode: {}", self.general.server_tls_mode);
if let Some(ref ca) = self.general.server_tls_ca_cert {
    info!("Server TLS CA cert: {ca}");
}
if let Some(ref cert) = self.general.server_tls_certificate {
    info!("Server TLS certificate: {cert}");
}
```

- [ ] **Step 3: Run `cargo check`**

Run: `cargo check`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add src/config/mod.rs
git commit -m "Add server TLS config validation and startup logging"
```

---

### Task 5: Address gets server_tls field

**Files:**
- Modify: `src/config/address.rs:49-69, 71-84, 96-117`
- Modify: `src/pool/mod.rs:327-336, 485-494`
- Modify: `src/pool/dynamic.rs:68-77`

- [ ] **Step 1: Add server_tls field to Address**

In `src/config/address.rs`, add field to `Address` struct:

```rust
/// Resolved TLS config for server-facing connections.
pub server_tls: Arc<crate::config::tls::ServerTlsConfig>,
```

Add import at top: `use std::sync::Arc;` (already imported for `AddressStats`).

Update `Default for Address`:

```rust
server_tls: Arc::new(crate::config::tls::ServerTlsConfig {
    mode: crate::config::tls::ServerTlsMode::Prefer,
    connector: None,
}),
```

Note: `Address` derives `PartialEq` and `Hash` manually — `server_tls` should NOT be included in equality/hash comparisons (like `stats` and `backend_auth`). The existing manual impls already skip non-identity fields, so `server_tls` is automatically excluded.

- [ ] **Step 2: Build ServerTlsConfig when creating Address in pool/mod.rs**

In `src/pool/mod.rs`, in the `from_config` method where `Address` structs are created, build `ServerTlsConfig` from the effective per-pool config.

Add a helper function at module level or inside `from_config`:

```rust
fn build_server_tls_for_pool(
    pool_config: &Pool,
    general: &General,
) -> Result<Arc<tls::ServerTlsConfig>, Error> {
    let mode_str = pool_config
        .server_tls_mode
        .as_deref()
        .unwrap_or(&general.server_tls_mode);
    let mode = tls::ServerTlsMode::from_string(mode_str)?;

    let ca = pool_config
        .server_tls_ca_cert
        .as_ref()
        .or(general.server_tls_ca_cert.as_ref());
    let cert = pool_config
        .server_tls_certificate
        .as_ref()
        .or(general.server_tls_certificate.as_ref());
    let key = pool_config
        .server_tls_private_key
        .as_ref()
        .or(general.server_tls_private_key.as_ref());

    let config = tls::ServerTlsConfig::new(
        mode,
        ca.map(|s| Path::new(s.as_str())),
        cert.map(|s| Path::new(s.as_str())),
        key.map(|s| Path::new(s.as_str())),
    )?;

    Ok(Arc::new(config))
}
```

Add necessary imports to `src/pool/mod.rs`:
```rust
use crate::config::tls;
use std::path::Path;
```

At each `Address { ... }` construction site (there are 3 in `pool/mod.rs`), call `build_server_tls_for_pool` once per pool (before the user loop), and add `server_tls: server_tls_config.clone()` to the struct literal.

- [ ] **Step 3: Update dynamic.rs Address construction**

In `src/pool/dynamic.rs`, the `Address` construction also needs `server_tls`. This function receives pool config, so call the same helper. Add the field:

```rust
server_tls: build_server_tls_for_pool(pool_config, &config.general)?,
```

Import the helper and necessary types.

- [ ] **Step 4: Run `cargo check`**

Run: `cargo check`
Expected: compiles. (The temporary placeholder in `server_backend.rs` still uses `false, false` — will be fixed in Task 6.)

- [ ] **Step 5: Commit**

```bash
git add src/config/address.rs src/pool/mod.rs src/pool/dynamic.rs
git commit -m "Add server_tls field to Address, build config per pool"
```

---

### Task 6: StreamInner TCPTls variant and TLS handshake

**Files:**
- Modify: `src/server/stream.rs` (entire file)

- [ ] **Step 1: Add TCPTls variant to StreamInner**

In `src/server/stream.rs`, add import:

```rust
use tokio_native_tls::TlsStream;
use crate::config::tls::{ServerTlsConfig, ServerTlsMode};
```

Add the new variant to the enum:

```rust
pin_project! {
    #[project = StreamInnerProj]
    #[derive(Debug)]
    pub enum StreamInner {
        TCPPlain {
            #[pin]
            stream: TcpStream,
        },
        TCPTls {
            #[pin]
            stream: TlsStream<TcpStream>,
        },
        UnixSocket {
            #[pin]
            stream: UnixStream,
        },
    }
}
```

- [ ] **Step 2: Extend all trait implementations**

Add `TCPTls` arm to each method in `AsyncWrite`, `AsyncRead`, and the inherent methods:

For `AsyncWrite`:
```rust
StreamInnerProj::TCPTls { stream } => stream.poll_write(cx, buf),
// Same pattern for poll_flush, poll_shutdown
```

For `AsyncRead`:
```rust
StreamInnerProj::TCPTls { stream } => stream.poll_read(cx, buf),
```

For `try_write`:
```rust
StreamInner::TCPTls { stream } => {
    use tokio::io::AsyncWrite;
    // TlsStream doesn't have try_write, use poll-based approach
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    match std::pin::Pin::new(stream).poll_write(&mut cx, buf) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => Err(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
    }
}
```

For `readable`:
```rust
StreamInner::TCPTls { stream } => stream.get_ref().get_ref().get_ref().readable().await,
```

For `try_read`:
```rust
StreamInner::TCPTls { stream } => stream.get_ref().get_ref().get_ref().try_read(buf),
```

Note: `TlsStream<TcpStream>` wraps the TCP stream. To access the inner `TcpStream` for readiness checks, use `get_ref()` chain. The exact depth depends on `tokio_native_tls::TlsStream` internals — check the patched version. The structure is: `TlsStream` → `native_tls::TlsStream` → `TcpStream`. Use `.get_ref().get_ref().get_ref()` to reach the inner `TcpStream`.

- [ ] **Step 3: Rewrite create_tcp_stream_inner for TLS handshake**

Replace the current `create_tcp_stream_inner` function:

```rust
pub(crate) async fn create_tcp_stream_inner(
    host: &str,
    port: u16,
    server_tls: &ServerTlsConfig,
) -> Result<StreamInner, Error> {
    let mut stream = match TcpStream::connect(&format!("{host}:{port}")).await {
        Ok(stream) => stream,
        Err(err) => {
            error!("Failed to connect to TCP {host}:{port}: {err}");
            return Err(Error::SocketError(format!(
                "Could not connect to {host}:{port}: {err}"
            )));
        }
    };

    configure_tcp_socket(&stream);

    if !server_tls.mode.sends_ssl_request() {
        return Ok(StreamInner::TCPPlain { stream });
    }

    // Send SSLRequest
    ssl_request(&mut stream).await?;

    let response = match stream.read_u8().await {
        Ok(response) => response as char,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to read TLS response from {host}:{port}: {err}"
            )));
        }
    };

    match response {
        'S' => {
            // Server supports TLS — perform handshake
            let connector = server_tls.connector.as_ref().ok_or_else(|| {
                Error::SocketError(format!(
                    "Server {host}:{port} supports TLS but no TLS connector configured"
                ))
            })?;

            match connector.connect(host, stream).await {
                Ok(tls_stream) => {
                    log::info!("TLS connection established to {host}:{port} (mode: {})", server_tls.mode);
                    Ok(StreamInner::TCPTls { stream: tls_stream })
                }
                Err(err) => {
                    error!(
                        "TLS handshake failed with {host}:{port} (mode: {}): {err}",
                        server_tls.mode
                    );
                    Err(Error::SocketError(format!(
                        "TLS handshake failed with {host}:{port}: {err}"
                    )))
                }
            }
        }
        'N' => {
            // Server does not support TLS
            if server_tls.mode.requires_tls() {
                error!(
                    "Server {host}:{port} does not support TLS but server_tls_mode is {}",
                    server_tls.mode
                );
                Err(Error::SocketError(format!(
                    "Server {host}:{port} does not support TLS but server_tls_mode is {}",
                    server_tls.mode
                )))
            } else {
                log::info!(
                    "Server {host}:{port} does not support TLS, using plain TCP (mode: {})",
                    server_tls.mode
                );
                Ok(StreamInner::TCPPlain { stream })
            }
        }
        other => Err(Error::SocketError(format!(
            "Unexpected TLS response '{}' (ASCII: {}) from {host}:{port}",
            other, other as u8
        ))),
    }
}
```

- [ ] **Step 4: Run `cargo check`**

Run: `cargo check`
Expected: compilation errors in `server_backend.rs` — the old `create_tcp_stream_inner` signature is gone. Fix in next task.

- [ ] **Step 5: Commit**

```bash
git add src/server/stream.rs
git commit -m "Add TCPTls variant to StreamInner with TLS handshake"
```

---

### Task 7: Wire up Server::startup() and cancel

**Files:**
- Modify: `src/server/server_backend.rs:44-123, 693-702, 712-736`
- Modify: `src/server/startup_cancel.rs` (entire file)
- Modify: `src/client/transaction.rs:278-302`
- Modify: `src/pool/mod.rs:47-48` (ClientServerMap type)

- [ ] **Step 1: Add connected_with_tls to Server struct**

In `src/server/server_backend.rs`, add field to `Server`:

```rust
/// Whether this connection is using TLS. Needed for cancel requests.
connected_with_tls: bool,
```

- [ ] **Step 2: Update Server::startup() to use address.server_tls**

In `Server::startup()`, replace the stream creation block:

```rust
let mut stream = if address.host.starts_with('/') {
    create_unix_stream_inner(&address.host, address.port).await?
} else {
    create_tcp_stream_inner(
        &address.host,
        address.port,
        &address.server_tls,
    )
    .await?
};
```

After the stream is created, determine if TLS was used:

```rust
let connected_with_tls = matches!(&stream, StreamInner::TCPTls { .. });
```

Pass `connected_with_tls` when constructing the `Server` struct (in the `Ok(Server { ... })` block at the end of `startup()`).

- [ ] **Step 3: Update ClientServerMap type to include TLS info**

In `src/pool/mod.rs`, change the `ClientServerMap` type:

```rust
pub type ClientServerMap = Arc<
    DashMap<
        (ProcessId, SecretKey),
        (ProcessId, SecretKey, ServerHost, ServerPort, Arc<tls::ServerTlsConfig>, bool),
    >,
>;
```

The last two fields: `Arc<ServerTlsConfig>` (the connector) and `bool` (connected_with_tls).

- [ ] **Step 4: Update Server::claim() to store TLS info**

In `src/server/server_backend.rs`, update `claim()`:

```rust
pub fn claim(&mut self, process_id: i32, secret_key: i32) {
    self.client_server_map.insert(
        (process_id, secret_key),
        (
            self.process_id,
            self.secret_key,
            self.address.host.clone(),
            self.address.port,
            self.address.server_tls.clone(),
            self.connected_with_tls,
        ),
    );
}
```

- [ ] **Step 5: Update startup_cancel.rs**

Replace `src/server/startup_cancel.rs`:

```rust
use bytes::{BufMut, BytesMut};
use log::warn;

use crate::config::tls::ServerTlsConfig;
use crate::errors::Error;
use crate::messages::constants::CANCEL_REQUEST_CODE;
use crate::messages::write_all_flush;

use super::stream::{create_tcp_stream_inner, create_unix_stream_inner};

pub(crate) async fn cancel(
    host: &str,
    port: u16,
    process_id: i32,
    secret_key: i32,
    server_tls: &ServerTlsConfig,
    connected_with_tls: bool,
) -> Result<(), Error> {
    let cancel_tls = if connected_with_tls {
        server_tls
    } else {
        // Use a static disable config for plain cancel
        &ServerTlsConfig {
            mode: crate::config::tls::ServerTlsMode::Disable,
            connector: None,
        }
    };

    let mut stream = if host.starts_with('/') {
        create_unix_stream_inner(host, port).await?
    } else {
        create_tcp_stream_inner(host, port, cancel_tls).await?
    };

    warn!("cancel request forwarded to {host}:{port} pid={process_id}");

    let mut bytes = BytesMut::with_capacity(16);
    bytes.put_i32(16);
    bytes.put_i32(CANCEL_REQUEST_CODE);
    bytes.put_i32(process_id);
    bytes.put_i32(secret_key);

    write_all_flush(&mut stream, &bytes).await
}
```

- [ ] **Step 6: Update Server::cancel() wrapper**

In `src/server/server_backend.rs`:

```rust
pub async fn cancel(
    host: &str,
    port: u16,
    process_id: i32,
    secret_key: i32,
    server_tls: &ServerTlsConfig,
    connected_with_tls: bool,
) -> Result<(), Error> {
    startup_cancel::cancel(host, port, process_id, secret_key, server_tls, connected_with_tls)
        .await
}
```

- [ ] **Step 7: Update client/transaction.rs cancel path**

In `handle_cancel_mode()`, update to extract and pass TLS info:

```rust
async fn handle_cancel_mode(&self) -> Result<(), Error> {
    let (process_id, secret_key, address, port, server_tls, connected_with_tls) = {
        match self
            .client_server_map
            .get(&(self.connection_id as i32, self.secret_key))
        {
            Some(entry) => {
                let (process_id, secret_key, address, port, server_tls, connected_with_tls) =
                    entry.value();
                {
                    let mut cancel_guard = CANCELED_PIDS.lock();
                    cancel_guard.insert(*process_id);
                }
                (
                    *process_id,
                    *secret_key,
                    address.clone(),
                    *port,
                    server_tls.clone(),
                    *connected_with_tls,
                )
            }
            None => return Ok(()),
        }
    };

    Server::cancel(&address, port, process_id, secret_key, &server_tls, connected_with_tls).await
}
```

- [ ] **Step 8: Run `cargo check`**

Run: `cargo check`
Expected: compiles.

- [ ] **Step 9: Run `cargo fmt && cargo clippy -- --deny "warnings"`**

Fix any warnings.

- [ ] **Step 10: Commit**

```bash
git add src/server/server_backend.rs src/server/startup_cancel.rs \
       src/client/transaction.rs src/pool/mod.rs
git commit -m "Wire up server TLS through startup, cancel, and pool paths"
```

---

### Task 8: Remove global config dependency from Server::startup()

**Files:**
- Modify: `src/server/server_backend.rs:712-736`

- [ ] **Step 1: Verify Server::startup() no longer reads config.general.server_tls**

Check that the temporary placeholder from Task 3 is gone and `address.server_tls` is used.

Run: `grep -n "config.general.server_tls\|config.general.verify_server" src/server/server_backend.rs`
Expected: no matches.

- [ ] **Step 2: Run full test suite**

Run: `cargo test`
Expected: all existing tests pass.

- [ ] **Step 3: Commit (if any fixups needed)**

```bash
git add -u
git commit -m "Clean up: remove global config reads from server startup path"
```

---

### Task 9: BDD test infrastructure — certificate generation

**Files:**
- Check: `tests/bdd/` directory structure
- Modify: BDD step definitions for cert generation

- [ ] **Step 1: Explore existing BDD infrastructure**

Examine how existing BDD tests start PostgreSQL and how TLS-related tests work:

Run: `ls tests/bdd/features/*.feature | head -20`
Run: `grep -r "ssl\|tls\|certificate" tests/bdd/ --include="*.rs" -l`

Understand the step definition pattern and how PostgreSQL instances are configured in tests.

- [ ] **Step 2: Add certificate generation step definitions**

The exact implementation depends on the existing BDD framework. The steps should:

1. Generate a self-signed CA key+cert
2. Generate a server key+cert signed by the CA (with SAN for localhost/127.0.0.1)
3. Generate a client key+cert signed by the CA (for mTLS tests)
4. Generate a separate "wrong" CA (for negative tests)
5. Write all files to a temp directory

Use `openssl` CLI commands in step definitions — the project vendors OpenSSL, and CLI is simpler than adding a Rust cert generation dependency.

- [ ] **Step 3: Add PostgreSQL with TLS startup step**

Add a step that starts PostgreSQL with `ssl = on`, pointing to the generated server cert/key. The step should modify `postgresql.conf` to include:

```
ssl = on
ssl_cert_file = '<tempdir>/server.crt'
ssl_key_file = '<tempdir>/server.key'
ssl_ca_file = '<tempdir>/ca.crt'  # for mTLS
```

- [ ] **Step 4: Commit**

```bash
git add tests/bdd/
git commit -m "Add BDD infrastructure for server TLS certificate generation"
```

---

### Task 10: BDD scenarios — server TLS modes

**Files:**
- Create: `tests/bdd/features/server-tls.feature`

- [ ] **Step 1: Write BDD scenarios**

Create `tests/bdd/features/server-tls.feature`:

```gherkin
Feature: Server-side TLS connections
  pg_doorman should establish TLS-encrypted connections to PostgreSQL
  servers when configured to do so.

  Background:
    Given we generate TLS certificates for server tests
    And PostgreSQL is running with TLS enabled

  Scenario: prefer mode with TLS-capable server connects via TLS
    Given pg_doorman config with server_tls_mode "prefer"
    When a client connects and runs "SELECT 1"
    Then the query succeeds
    And the server connection uses TLS

  Scenario: prefer mode with non-TLS server falls back to plain TCP
    Given PostgreSQL is running without TLS
    And pg_doorman config with server_tls_mode "prefer"
    When a client connects and runs "SELECT 1"
    Then the query succeeds
    And the server connection does not use TLS

  Scenario: require mode with TLS-capable server connects via TLS
    Given pg_doorman config with server_tls_mode "require"
    When a client connects and runs "SELECT 1"
    Then the query succeeds
    And the server connection uses TLS

  Scenario: require mode with non-TLS server fails
    Given PostgreSQL is running without TLS
    And pg_doorman config with server_tls_mode "require"
    When a client connects and runs "SELECT 1"
    Then the connection fails

  Scenario: verify-ca with correct CA succeeds
    Given pg_doorman config with server_tls_mode "verify-ca" and correct CA
    When a client connects and runs "SELECT 1"
    Then the query succeeds

  Scenario: verify-ca with wrong CA fails
    Given pg_doorman config with server_tls_mode "verify-ca" and wrong CA
    When a client connects and runs "SELECT 1"
    Then the connection fails

  Scenario: verify-full with correct hostname succeeds
    Given pg_doorman config with server_tls_mode "verify-full" and correct CA
    When a client connects and runs "SELECT 1"
    Then the query succeeds

  Scenario: verify-full with hostname mismatch fails
    Given pg_doorman config with server_tls_mode "verify-full" and wrong-hostname cert
    When a client connects and runs "SELECT 1"
    Then the connection fails

  Scenario: disable mode uses plain TCP
    Given pg_doorman config with server_tls_mode "disable"
    When a client connects and runs "SELECT 1"
    Then the query succeeds
    And the server connection does not use TLS

  Scenario: mTLS — pg_doorman presents client certificate
    Given PostgreSQL requires client certificates
    And pg_doorman config with server_tls_mode "verify-ca" and client certificate
    When a client connects and runs "SELECT 1"
    Then the query succeeds

  Scenario: cancel request uses TLS when main connection uses TLS
    Given pg_doorman config with server_tls_mode "require"
    When a client starts a long-running query
    And the client sends a cancel request
    Then the query is cancelled

  Scenario: per-pool TLS override
    Given pg_doorman config with two pools:
      | pool    | server_tls_mode |
      | pool_a  | require         |
      | pool_b  | disable         |
    When a client connects to "pool_a" and runs "SELECT 1"
    Then the server connection uses TLS
    When a client connects to "pool_b" and runs "SELECT 1"
    Then the server connection does not use TLS
```

- [ ] **Step 2: Implement step definitions**

Implement the Rust step definitions to match the scenarios above. The exact code depends on the existing BDD framework patterns discovered in Task 9 Step 1.

- [ ] **Step 3: Run the BDD tests**

Run: `cargo test --test bdd -- server-tls` (or the project's BDD test command)
Expected: all scenarios pass.

- [ ] **Step 4: Commit**

```bash
git add tests/bdd/
git commit -m "Add BDD scenarios for server-side TLS"
```

---

### Task 11: Final quality checks

**Files:**
- All modified files

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: all tests pass (existing + new).

- [ ] **Step 2: Run formatting and linting**

Run: `cargo fmt && cargo clippy -- --deny "warnings"`
Expected: no warnings, no formatting changes.

- [ ] **Step 3: Check for leftover references to old config fields**

Run: `grep -rn "server_tls:" src/ --include="*.rs" | grep -v server_tls_mode | grep -v server_tls_ca | grep -v server_tls_certificate | grep -v server_tls_private`
Expected: only the `server_tls` field on `Address` (the new `Arc<ServerTlsConfig>`).

Run: `grep -rn "verify_server_certificate" src/ --include="*.rs"`
Expected: no matches.

- [ ] **Step 4: Final commit (if any fixups)**

```bash
git add -u
git commit -m "Final cleanup for server-side TLS"
```
