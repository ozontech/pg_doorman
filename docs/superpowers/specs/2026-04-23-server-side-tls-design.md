# Server-side TLS: pg_doorman → PostgreSQL

pg_doorman supports TLS for client-facing connections (frontend) but not for
server-facing connections (backend).  The `server_tls` / `verify_server_certificate`
config flags are parsed but non-functional — `stream.rs` returns an error when the
server accepts TLS.  This spec covers full backend TLS support.

## Scope

Encrypted connections from pg_doorman to PostgreSQL servers, including:

- Five SSL modes matching libpq semantics: `disable`, `prefer`, `require`,
  `verify-ca`, `verify-full`
- Mutual TLS (client certificate presented by pg_doorman to the server)
- TLS for cancel requests matching the main connection's TLS state
- Per-pool TLS configuration with global defaults

Out of scope: Patroni failover, TLS migration for server connections,
server-side TLS for the Patroni proxy binary.

## Configuration

### Parameters

All parameters live in `[general]` (global defaults) and can be overridden
per-pool in `[pools.<name>]`.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `server_tls_mode` | string | `"prefer"` | TLS mode for backend connections |
| `server_tls_ca_cert` | string (path) | none | CA certificate for server verification |
| `server_tls_certificate` | string (path) | none | Client certificate for mTLS |
| `server_tls_private_key` | string (path) | none | Client private key for mTLS |

### Modes

| Mode | SSLRequest sent | Server says 'S' | Server says 'N' |
|------|----------------|-----------------|-----------------|
| `disable` | no | — | plain TCP |
| `prefer` | yes | TLS handshake | plain TCP |
| `require` | yes | TLS handshake | **error** |
| `verify-ca` | yes | TLS handshake + CA check | **error** |
| `verify-full` | yes | TLS handshake + CA + hostname check | **error** |

### Removed parameters

`server_tls: bool` and `verify_server_certificate: bool` are removed.  They
were non-functional — removing them is a breaking config change but not a
behavioral change.

### Validation (at startup)

- `verify-ca` or `verify-full` without `server_tls_ca_cert` → error
- `server_tls_certificate` without `server_tls_private_key` (or vice versa) → error
- Certificate/key files unreadable or unparseable → error
- Validation runs for the effective value of each pool (after merging with
  general defaults)

### Config reload (SIGHUP)

TLS config is re-read.  A new `TlsConnector` is built.  New server connections
use the new connector.  Existing connections continue with the old one until
they are closed by `server_lifetime` or eviction.

### Example

```toml
[general]
server_tls_mode = "verify-ca"
server_tls_ca_cert = "/etc/ssl/pg-ca.crt"
server_tls_certificate = "/etc/ssl/doorman.crt"
server_tls_private_key = "/etc/ssl/doorman.key"

[pools.production]
server_host = "pg-primary.internal"
# inherits server_tls_mode = "verify-ca" from general

[pools.legacy]
server_host = "old-pg.internal"
server_tls_mode = "disable"
```

## Architecture

### StreamInner

New variant added to the enum in `src/server/stream.rs`:

```
TCPPlain  { stream: TcpStream }
TCPTls    { stream: tokio_native_tls::TlsStream<TcpStream> }
UnixSocket { stream: UnixStream }
```

All trait implementations (`AsyncRead`, `AsyncWrite`, `try_write`, `readable`,
`try_read`) are extended with the third arm.

### ServerTlsConfig

```rust
pub struct ServerTlsConfig {
    pub mode: ServerTlsMode,
    pub connector: Option<tokio_native_tls::TlsConnector>,
}
```

- Built once per pool during config load/reload
- Stored as `Arc<ServerTlsConfig>` on `Address`
- `connector` is `None` when `mode == Disable` (no TLS material needed)
- For `verify-ca`: connector built with `danger_accept_invalid_hostnames(true)`
- For `verify-full`: connector built with hostname verification enabled

### TlsConnector construction

Uses `native_tls::TlsConnector::builder()`:

- `.min_protocol_version(Some(Protocol::Tlsv12))` — same minimum as frontend
- `.root_certificates(ca)` — when `verify-ca` or `verify-full`
- `.identity(identity)` — when mTLS certificate/key provided
- `.danger_accept_invalid_hostnames(true)` — only for `verify-ca`
- `.danger_accept_invalid_certs(true)` — only for `require` (no verification)

### Connection flow

`Address.server_tls: Arc<ServerTlsConfig>` → `Server::startup()` →
`create_tcp_stream_inner()`.

`create_tcp_stream_inner` signature changes from
`(host, port, tls: bool, _verify: bool)` to
`(host, port, server_tls: &ServerTlsConfig)`.

The function no longer reads global config — TLS config comes from the address.

### Cancel requests

`Server` stores `connected_with_tls: bool`, set after handshake.

`cancel()` signature becomes:

```rust
pub async fn cancel(
    host: &str,
    port: u16,
    process_id: i32,
    secret_key: i32,
    server_tls: &ServerTlsConfig,
    connected_with_tls: bool,
) -> Result<(), Error>
```

- `connected_with_tls == false` → plain TCP cancel (current behavior)
- `connected_with_tls == true` → SSLRequest + TLS handshake + cancel message,
  using the same `TlsConnector` from `server_tls`

Cancel connections are one-shot (16 bytes, then close), so a TLS handshake per
cancel is acceptable.

## Error handling

### Handshake failure

For all modes including `prefer`: if the server responds `'S'` but the TLS
handshake fails, the connection fails with an error.  No silent fallback to
plain TCP — a handshake failure signals misconfiguration (wrong CA, expired
cert), and hiding it is dangerous.

### Log messages

Connection errors include: host, port, TLS mode, and the underlying error
(certificate expired, hostname mismatch, unknown CA, etc.).

- `prefer` + server says `'N'` → info-level log, plain TCP connection
- `require`+ server says `'N'` → error: "server does not support TLS but
  server_tls_mode is require"

## Testing

### Unit tests

- `ServerTlsMode::from_string()` — all modes + invalid values
- Config validation — all error combinations
- Per-pool merge logic — pool override vs general fallback

### BDD scenarios (server-tls.feature)

Certificates are generated on the fly in test steps (no pre-existing cert
files).  Each test creates its own CA, server cert, and client cert.

1. `prefer` + server supports TLS → TLS connection, queries work
2. `prefer` + server does not support TLS → plain TCP, queries work
3. `require` + server supports TLS → TLS connection, queries work
4. `require` + server does not support TLS → client gets error
5. `verify-ca` + correct CA → works
6. `verify-ca` + wrong CA → error
7. `verify-full` + correct CA + correct hostname → works
8. `verify-full` + hostname mismatch → error
9. `disable` → plain TCP regardless of server TLS support
10. mTLS — pg_doorman presents client certificate to server
11. Cancel via TLS — cancel uses TLS when main connection used TLS
12. Per-pool override — one pool `require`, another `disable`, both work
