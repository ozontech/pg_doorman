# Server-side TLS Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all issues found during deep review of server-side TLS: 3 critical bugs, 5 important fixes, hot reload, Prometheus metrics, 8 minor fixes.

**Architecture:** Atomic commits in priority order. Each task = one commit. Critical bugs first, then important fixes, then features, then minor cleanup.

**Tech Stack:** Rust, tokio, native-tls, prometheus crate, BDD/cucumber tests.

**Spec:** `docs/superpowers/specs/2026-04-24-server-tls-fixes-design.md`

**Important:** Do NOT add `docs/superpowers/` to git commits. The user explicitly excluded it from the PR.

---

## File Map

| File | Changes |
|------|---------|
| `src/pool/server_pool.rs` | Fix stats leak in allow-mode retry |
| `src/server/stream.rs` | Add async `write_all`, add `is_tls()`, fix ErrorResponse handling |
| `src/server/authentication.rs` | Replace `try_write` with async `write_all` |
| `src/server/server_backend.rs` | TLS-aware `check_server_alive`, update `claim()` for CancelTarget |
| `src/config/tls.rs` | CA bundle, `FromStr`, `cert_hash`, CA assertion, `requires_ca` guard |
| `src/config/address.rs` | Fix `Default` to use `Disable` |
| `src/pool/mod.rs` | Add `CancelTarget` struct, update `ClientServerMap` type |
| `src/client/transaction.rs` | Update `handle_cancel_mode` for `CancelTarget` |
| `src/pool/gc.rs` | Document magic number |
| `src/stats/server.rs` | Remove `#[inline(always)]` |
| `src/prometheus/mod.rs` | Register 3 new TLS metrics |
| `src/prometheus/metrics.rs` | Update server TLS metrics in export |
| `tests/bdd/features/server-tls.feature` | Add allow-mode scenarios |
| `tests/bdd/doorman_helper.rs` | Support allow-mode BDD infra if needed |
| `README.md` | Document prefer limitation, channel_binding, cipher suites |

---

## Task 1: Fix stats leak in allow-mode retry

**Files:**
- Modify: `src/pool/server_pool.rs:150-241`

- [ ] **Step 1: Fix the stats lifecycle in `create()` method**

In `src/pool/server_pool.rs`, replace the retry block and match block (lines 196-241) with proper stats cleanup:

```rust
        // libpq sslmode=allow retry: try plain first, retry with TLS on any failure.
        //
        // PostgreSQL has no protocol-level "TLS required" signal. The server
        // rejects non-TLS connections via pg_hba.conf AFTER StartupMessage
        // with FATAL 28000 ("no pg_hba.conf entry ... no encryption").
        // The connection is dead after FATAL, so retry requires a new TCP socket.
        //
        // We retry on ANY startup failure (not just SSL-related) because:
        // 1. libpq does the same: "first try a non-SSL connection; if that fails,
        //    try an SSL connection" — no message parsing.
        // 2. If the real error is unrelated to TLS (wrong password, DB not found),
        //    the TLS retry will fail with the same error, which we then return.
        //
        // Reference: PostgreSQL docs, "SSL Support" -> sslmode parameter.
        let (result, active_stats) =
            if result.is_err() && self.address.server_tls.mode.retries_with_tls() {
                info!(
                    "plain connection failed, retrying with tls, user={} pool={} host={} port={} server_tls_mode=allow",
                    self.address.username, self.address.pool_name,
                    self.address.host, self.address.port,
                );
                // Clean up stats from the failed plain attempt before retry.
                stats.disconnect();

                let mut retry_address = self.address.clone();
                retry_address.server_tls =
                    std::sync::Arc::new(crate::config::tls::ServerTlsConfig {
                        mode: crate::config::tls::ServerTlsMode::Require,
                        connector: self.address.server_tls.connector.clone(),
                    });
                let retry_stats = Arc::new(ServerStats::new(
                    self.address.clone(),
                    crate::utils::clock::now(),
                ));
                retry_stats.register(retry_stats.clone());
                let retry_result = Server::startup(
                    &retry_address,
                    &self.user,
                    &self.database,
                    self.client_server_map.clone(),
                    retry_stats.clone(),
                    self.cleanup_connections,
                    self.log_client_parameter_status_changes,
                    self.prepared_statement_cache_size,
                    self.application_name.clone(),
                    self.session_mode,
                )
                .await;
                (retry_result, retry_stats)
            } else {
                (result, stats)
            };

        match result {
            Ok(conn) => {
                // Permit is released automatically when _permit goes out of scope
                conn.stats.idle(0);
                Ok(conn)
            }
            Err(err) => {
                // Brief backoff on error to avoid hammering a failing server
                tokio::time::sleep(Duration::from_millis(10)).await;
                active_stats.disconnect();
                Err(err)
            }
        }
```

Key changes:
- `stats.disconnect()` called before retry (cleans up first registered stats)
- `active_stats` tracks which stats object is currently live
- On error: `active_stats.disconnect()` always disconnects the right one

- [ ] **Step 2: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 3: Commit**

```
Fix stats leak in allow-mode retry

Two ServerStats registered during allow-mode TLS retry: original for plain
attempt, second for TLS retry. On successful retry the original was never
disconnected, leaving ghost entries in SHOW SERVERS. On double failure the
retry stats leaked instead.

Now the plain stats is explicitly disconnected before retry, and the match
block always disconnects whichever stats object is currently active.
```

---

## Task 2: Fix try_write via Waker::noop() in auth path

**Files:**
- Modify: `src/server/stream.rs:86-102`
- Modify: `src/server/authentication.rs:1-10,148,201`

- [ ] **Step 1: Add async `write_all` method to `StreamInner`**

In `src/server/stream.rs`, add after the `try_write` method (after line 102):

```rust
    /// Async write that properly handles TLS back-pressure.
    /// Use this instead of try_write() when in an async context
    /// (e.g., server authentication). try_write() uses a noop waker
    /// for TLS which silently fails on Pending — this method awaits
    /// until the full buffer is written.
    pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        match self {
            StreamInner::TCPPlain { stream } => stream.write_all(buf).await,
            StreamInner::TCPTls { stream } => stream.write_all(buf).await,
            StreamInner::UnixSocket { stream } => stream.write_all(buf).await,
        }
    }
```

- [ ] **Step 2: Update authentication.rs to use async write_all**

In `src/server/authentication.rs`, replace the JWT try_write call (line 148):

Old:
```rust
            stream.try_write(&password_response).map_err(|err| {
                Error::ServerAuthError(
                    format!("jwt authentication on the server failed: {err:?}"),
                    server_identifier.clone(),
                )
            })?;
```

New:
```rust
            stream.write_all(&password_response).await.map_err(|err| {
                Error::ServerAuthError(
                    format!("jwt authentication on the server failed: {err:?}"),
                    server_identifier.clone(),
                )
            })?;
```

Replace the MD5 try_write call (line 201) with the same pattern:

Old:
```rust
            stream.try_write(&password_response).map_err(|err| {
                Error::ServerAuthError(
                    format!("md5 authentication on the server failed: {err:?}"),
                    server_identifier.clone(),
                )
            })?;
```

New:
```rust
            stream.write_all(&password_response).await.map_err(|err| {
                Error::ServerAuthError(
                    format!("md5 authentication on the server failed: {err:?}"),
                    server_identifier.clone(),
                )
            })?;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 4: Commit**

```
Fix TLS auth: use async write instead of noop-waker poll

try_write() for TLS streams uses poll_write with Waker::noop(). If the TCP
send buffer is full, poll returns Pending and try_write yields WouldBlock,
which the auth code interprets as a fatal error. Under load this causes
spurious authentication failures for MD5 and JWT on TLS connections.

Replaced with async write_all() in the authentication path, which properly
awaits until the full buffer is written. try_write() is kept for the Drop
path (Terminate message) where async is unavailable and loss is acceptable.
```

---

## Task 3: Fix readable()/try_read() TLS bypass

**Files:**
- Modify: `src/server/stream.rs:86-123` (add `is_tls()`)
- Modify: `src/server/server_backend.rs:275-281` (TLS-aware check_server_alive)

- [ ] **Step 1: Add `is_tls()` method to `StreamInner`**

In `src/server/stream.rs`, add after the `try_read` method (after line 123):

```rust
    /// Returns true if this stream uses TLS encryption.
    pub fn is_tls(&self) -> bool {
        matches!(self, StreamInner::TCPTls { .. })
    }
```

- [ ] **Step 2: Make `check_server_alive` TLS-aware**

In `src/server/server_backend.rs`, replace `check_server_alive` (lines 275-281):

Old:
```rust
    pub fn check_server_alive(&self) -> bool {
        let mut buf = [0u8; 1];
        matches!(
            self.stream.get_ref().try_read(&mut buf),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
        )
    }
```

New:
```rust
    pub fn check_server_alive(&self) -> bool {
        if self.stream.get_ref().is_tls() {
            // For TLS connections, readable() fires on raw TCP socket readiness.
            // Calling try_read() on the raw socket would consume bytes that the
            // TLS layer hasn't processed, corrupting the session.
            //
            // On an idle PostgreSQL connection, the raw socket should never become
            // readable (PostgreSQL does not send unsolicited data, and TLS
            // renegotiation is disabled since PG14). If readable() fired, the
            // server disconnected or sent an error — treat as dead.
            return false;
        }
        let mut buf = [0u8; 1];
        matches!(
            self.stream.get_ref().try_read(&mut buf),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
        )
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 4: Commit**

```
Fix idle check for TLS connections: skip raw socket read

check_server_alive() used try_read() on the raw TCP socket to distinguish
spurious readiness from real data. For TLS streams this bypasses the TLS
layer and can consume encrypted bytes, corrupting the session.

For TLS connections, if the raw socket becomes readable while idle, the
server has disconnected (PostgreSQL sends no unsolicited data, and TLS
renegotiation is disabled since PG14). Return dead immediately without
reading from the raw socket.
```

---

## Task 4: Support CA certificate bundles

**Files:**
- Modify: `src/config/tls.rs:147-164`

- [ ] **Step 1: Replace `load_certificate` with `load_certificates`**

In `src/config/tls.rs`, replace the `load_certificate` function (lines 147-164):

Old:
```rust
/// Load a certificate from a PEM file
fn load_certificate(path: &Path) -> Result<Certificate, Error> {
    let cert_data = read_file(path).map_err(|err| {
        Error::BadConfig(format!(
            "Failed to read certificate file {}: {}",
            path.display(),
            err
        ))
    })?;

    Certificate::from_pem(&cert_data).map_err(|err| {
        Error::BadConfig(format!(
            "Failed to parse certificate {}: {}",
            path.display(),
            err
        ))
    })
}
```

New:
```rust
/// Load all certificates from a PEM file (supports bundles with
/// intermediate CA chains — multiple PEM blocks in one file).
fn load_certificates(path: &Path) -> Result<Vec<Certificate>, Error> {
    let cert_data = read_file(path).map_err(|err| {
        Error::BadConfig(format!(
            "Failed to read certificate file {}: {}",
            path.display(),
            err
        ))
    })?;

    let pem_str = std::str::from_utf8(&cert_data).map_err(|err| {
        Error::BadConfig(format!(
            "Certificate file {} is not valid UTF-8: {}",
            path.display(),
            err
        ))
    })?;

    let mut certs = Vec::new();
    let mut start = 0;
    let begin_marker = "-----BEGIN CERTIFICATE-----";
    let end_marker = "-----END CERTIFICATE-----";

    while let Some(begin) = pem_str[start..].find(begin_marker) {
        let abs_begin = start + begin;
        let after_begin = abs_begin + begin_marker.len();
        match pem_str[after_begin..].find(end_marker) {
            Some(end_offset) => {
                let abs_end = after_begin + end_offset + end_marker.len();
                let pem_block = &pem_str[abs_begin..abs_end];
                let cert = Certificate::from_pem(pem_block.as_bytes()).map_err(|err| {
                    Error::BadConfig(format!(
                        "Failed to parse certificate #{} in {}: {}",
                        certs.len() + 1,
                        path.display(),
                        err
                    ))
                })?;
                certs.push(cert);
                start = abs_end;
            }
            None => {
                return Err(Error::BadConfig(format!(
                    "Unterminated PEM block in {}: found BEGIN without END",
                    path.display(),
                )));
            }
        }
    }

    if certs.is_empty() {
        return Err(Error::BadConfig(format!(
            "No certificates found in {}",
            path.display(),
        )));
    }

    Ok(certs)
}
```

- [ ] **Step 2: Update callers in `ServerTlsConfig::new()`**

In `src/config/tls.rs`, replace the two `load_certificate` calls in `ServerTlsConfig::new()` (inside the VerifyCa and VerifyFull arms, approximately lines 197-209):

Old (VerifyCa arm):
```rust
            ServerTlsMode::VerifyCa => {
                if let Some(ca_path) = ca_cert {
                    let ca = load_certificate(ca_path)?;
                    builder.add_root_certificate(ca);
                }
                builder.danger_accept_invalid_hostnames(true);
            }
```

New:
```rust
            ServerTlsMode::VerifyCa => {
                if let Some(ca_path) = ca_cert {
                    for ca in load_certificates(ca_path)? {
                        builder.add_root_certificate(ca);
                    }
                }
                builder.danger_accept_invalid_hostnames(true);
            }
```

Old (VerifyFull arm):
```rust
            ServerTlsMode::VerifyFull => {
                if let Some(ca_path) = ca_cert {
                    let ca = load_certificate(ca_path)?;
                    builder.add_root_certificate(ca);
                }
            }
```

New:
```rust
            ServerTlsMode::VerifyFull => {
                if let Some(ca_path) = ca_cert {
                    for ca in load_certificates(ca_path)? {
                        builder.add_root_certificate(ca);
                    }
                }
            }
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 4: Commit**

```
Support CA certificate bundles in server TLS config

Certificate::from_pem() loads only the first PEM block. CA files from
enterprise PKI often contain a chain: root CA + intermediate CAs in one
file. Without loading all blocks, TLS verification fails when the server
certificate is signed by an intermediate CA.

Replaced load_certificate() with load_certificates() that parses every
PEM block in the file and adds each as a root certificate.
```

---

## Task 5: Fix Address::default() and add CA assertion

**Files:**
- Modify: `src/config/address.rs:83-87`
- Modify: `src/config/tls.rs:174-187` (add requires_ca guard)

- [ ] **Step 1: Fix Address::default()**

In `src/config/address.rs`, replace the server_tls field in `Default` impl (lines 84-87):

Old:
```rust
            server_tls: Arc::new(crate::config::tls::ServerTlsConfig {
                mode: crate::config::tls::ServerTlsMode::Prefer,
                connector: None,
            }),
```

New:
```rust
            server_tls: Arc::new(crate::config::tls::ServerTlsConfig {
                mode: crate::config::tls::ServerTlsMode::Disable,
                connector: None,
            }),
```

- [ ] **Step 2: Add requires_ca guard in ServerTlsConfig::new()**

In `src/config/tls.rs`, add after the `Disable` early return (after line 187, before `let mut builder`):

```rust
        if mode.requires_ca() && ca_cert.is_none() {
            return Err(Error::BadConfig(format!(
                "server_tls_mode '{}' requires server_tls_ca_cert to be set",
                mode
            )));
        }
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 4: Commit**

```
Fix Address::default() TLS mode and add CA validation guard

Address::default() used Prefer with connector=None, an invalid combination
that would error if used for real connections. Changed to Disable (the only
valid mode without a connector). Only affects test code.

Added requires_ca() check inside ServerTlsConfig::new() so that direct
callers (bypassing config validation) get an error for verify-ca/verify-full
without a CA certificate path.
```

---

## Task 6: Replace 6-element tuple with CancelTarget struct

**Files:**
- Modify: `src/pool/mod.rs:44-61`
- Modify: `src/server/server_backend.rs:613-625`
- Modify: `src/client/transaction.rs:278-318`

- [ ] **Step 1: Define CancelTarget and update ClientServerMap**

In `src/pool/mod.rs`, replace the type alias block (lines 44-61):

Old:
```rust
pub type ProcessId = i32;
pub type SecretKey = i32;
pub type ServerHost = String;
pub type ServerPort = u16;

pub type ClientServerMap = Arc<
    DashMap<
        (ProcessId, SecretKey),
        (
            ProcessId,
            SecretKey,
            ServerHost,
            ServerPort,
            Arc<tls::ServerTlsConfig>,
            bool,
        ),
    >,
>;
```

New:
```rust
pub type ProcessId = i32;
pub type SecretKey = i32;
pub type ServerHost = String;
pub type ServerPort = u16;

/// Target information for forwarding a CancelRequest to the correct backend.
pub struct CancelTarget {
    pub process_id: ProcessId,
    pub secret_key: SecretKey,
    pub host: ServerHost,
    pub port: ServerPort,
    pub server_tls: Arc<tls::ServerTlsConfig>,
    pub connected_with_tls: bool,
}

pub type ClientServerMap = Arc<DashMap<(ProcessId, SecretKey), CancelTarget>>;
```

- [ ] **Step 2: Update `claim()` in server_backend.rs**

In `src/server/server_backend.rs`, replace the `claim()` method (lines 613-625):

Old:
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

New:
```rust
    pub fn claim(&mut self, process_id: i32, secret_key: i32) {
        self.client_server_map.insert(
            (process_id, secret_key),
            CancelTarget {
                process_id: self.process_id,
                secret_key: self.secret_key,
                host: self.address.host.clone(),
                port: self.address.port,
                server_tls: self.address.server_tls.clone(),
                connected_with_tls: self.connected_with_tls,
            },
        );
    }
```

Add import at the top of `server_backend.rs`:
```rust
use crate::pool::CancelTarget;
```

- [ ] **Step 3: Update `handle_cancel_mode()` in transaction.rs**

In `src/client/transaction.rs`, replace the destructuring (lines 278-307):

Old:
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

        Server::cancel(
            &address,
            port,
            process_id,
            secret_key,
            &server_tls,
            connected_with_tls,
        )
        .await
    }
```

New:
```rust
    async fn handle_cancel_mode(&self) -> Result<(), Error> {
        let target = match self
            .client_server_map
            .get(&(self.connection_id as i32, self.secret_key))
        {
            Some(entry) => {
                let t = entry.value();
                {
                    let mut cancel_guard = CANCELED_PIDS.lock();
                    cancel_guard.insert(t.process_id);
                }
                CancelTarget {
                    process_id: t.process_id,
                    secret_key: t.secret_key,
                    host: t.host.clone(),
                    port: t.port,
                    server_tls: t.server_tls.clone(),
                    connected_with_tls: t.connected_with_tls,
                }
            }
            None => return Ok(()),
        };

        Server::cancel(
            &target.host,
            target.port,
            target.process_id,
            target.secret_key,
            &target.server_tls,
            target.connected_with_tls,
        )
        .await
    }
```

Add import at the top of `transaction.rs`:
```rust
use crate::pool::CancelTarget;
```

- [ ] **Step 4: Run tests and fix any remaining compilation errors**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

There may be other files that destructure the old tuple — grep for the pattern and fix all occurrences.

Run grep: `rg "server_tls, connected_with_tls" src/` to find any remaining tuple destructuring.

- [ ] **Step 5: Commit**

```
Replace 6-element cancel tuple with CancelTarget struct

ClientServerMap stored cancel information as a 6-element positional tuple.
Positional access is error-prone and hard to read. Replaced with a named
CancelTarget struct with explicit field names.
```

---

## Task 7: Add BDD scenarios for allow mode

**Files:**
- Modify: `tests/bdd/features/server-tls.feature`

- [ ] **Step 1: Add allow-retry scenario**

Append before the last scenario in `tests/bdd/features/server-tls.feature` (before the existing `@server-tls-allow` scenario if it exists, or at the end):

```gherkin
@server-tls @server-tls-allow-retry
Scenario: allow mode retries with TLS when server requires encryption
  Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
    """
    hostssl all all 127.0.0.1/32 trust
    """
  And fixtures from "tests/fixture.sql" applied
  And pg_doorman started with config:
    """
    [general]
    host = "127.0.0.1"
    port = ${DOORMAN_PORT}
    admin_username = "admin"
    admin_password = "admin"
    server_tls_mode = "allow"
    pg_hba.content = "host all all 127.0.0.1/32 trust"

    [pools.example_db]
    server_host = "127.0.0.1"
    server_port = ${PG_PORT}

    [[pools.example_db.users]]
    username = "example_user_1"
    password = ""
    pool_size = 1
    """
  When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
  And we send SimpleQuery "SELECT 1" to session "s1" and store response
  Then session "s1" should receive DataRow with "1"
  When we close session "s1"

@server-tls @server-tls-allow-plain
Scenario: allow mode uses plain TCP when server accepts unencrypted
  Given PostgreSQL started with pg_hba.conf:
    """
    host all all 127.0.0.1/32 trust
    """
  And fixtures from "tests/fixture.sql" applied
  And pg_doorman started with config:
    """
    [general]
    host = "127.0.0.1"
    port = ${DOORMAN_PORT}
    admin_username = "admin"
    admin_password = "admin"
    server_tls_mode = "allow"
    pg_hba.content = "host all all 127.0.0.1/32 trust"

    [pools.example_db]
    server_host = "127.0.0.1"
    server_port = ${PG_PORT}

    [[pools.example_db.users]]
    username = "example_user_1"
    password = ""
    pool_size = 1
    """
  When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
  And we send SimpleQuery "SELECT 1" to session "s1" and store response
  Then session "s1" should receive DataRow with "1"
  When we close session "s1"
```

- [ ] **Step 2: Run BDD tests locally (if possible)**

Run: `cargo test --test bdd -- --tags @server-tls-allow-retry`
Run: `cargo test --test bdd -- --tags @server-tls-allow-plain`

If tests require Docker/CI environment, verify compilation only:
Run: `cargo test --test bdd --no-run`

- [ ] **Step 3: Commit**

```
Add BDD scenarios for allow mode TLS retry

allow is the default server_tls_mode with the most complex retry logic
(plain first, TLS on failure), but had zero BDD coverage.

Added two scenarios:
- allow-retry: server requires TLS (hostssl only), plain fails, retry succeeds
- allow-plain: server accepts plain, connection succeeds without retry
```

---

## Task 8: Document prefer mode limitation

**Files:**
- Modify: `src/server/stream.rs:208-218`
- Modify: `README.md` (server-side TLS section)

- [ ] **Step 1: Add comment in stream.rs**

In `src/server/stream.rs`, add a comment before the handshake error arm (before line 208):

```rust
                // Note: unlike libpq, we do NOT retry on a new plain TCP socket
                // when TLS handshake fails after server responded 'S'. The TCP
                // connection is already consumed by the partial handshake, and
                // this edge case (server accepts SSL but handshake fails due to
                // cipher/version mismatch) is rare in practice.
```

- [ ] **Step 2: Add notes to README.md**

Find the server-side TLS table/section in `README.md` and add after the mode table:

```markdown
**Known limitations:**

- **`prefer` mode fallback:** If the server accepts the SSL request but the TLS handshake fails (e.g., cipher mismatch), the connection is not retried on plain TCP (unlike libpq). This edge case is rare in practice.
- **`channel_binding = require`:** PostgreSQL's `channel_binding = require` setting is incompatible with pg_doorman. The pooler uses separate TLS sessions for client-to-pooler and pooler-to-server connections, so SCRAM channel binding cannot be forwarded.
- **Cipher suites:** Cipher suite selection is not currently configurable; system OpenSSL defaults are used. TLS 1.2 is the minimum protocol version.
```

- [ ] **Step 3: Commit**

```
Document server TLS known limitations

Added documentation for three known limitations:
- prefer mode does not retry plain TCP after TLS handshake failure
- PostgreSQL channel_binding=require is incompatible with connection pooling
- cipher suite selection is not configurable (uses system OpenSSL defaults)
```

---

## Task 9: Hot reload TLS certificates on SIGHUP

**Files:**
- Modify: `src/config/tls.rs:164-172` (add `cert_hash` field, `PartialEq`)
- Modify: `src/pool/mod.rs:94-125` (pass cert contents hash)
- Modify: `src/pool/mod.rs:319-335` (include TLS in pool comparison)

- [ ] **Step 1: Add cert_hash to ServerTlsConfig**

In `src/config/tls.rs`, update the `ServerTlsConfig` struct (lines 168-172):

Old:
```rust
pub struct ServerTlsConfig {
    pub mode: ServerTlsMode,
    pub connector: Option<tokio_native_tls::TlsConnector>,
}
```

New:
```rust
pub struct ServerTlsConfig {
    pub mode: ServerTlsMode,
    pub connector: Option<tokio_native_tls::TlsConnector>,
    /// SHA-256 hash of certificate file contents (ca + client cert + client key).
    /// Used to detect cert changes on SIGHUP reload without comparing opaque
    /// TlsConnector objects.
    pub cert_hash: Option<[u8; 32]>,
}
```

- [ ] **Step 2: Implement PartialEq for ServerTlsConfig**

In `src/config/tls.rs`, add after the struct definition:

```rust
impl PartialEq for ServerTlsConfig {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode && self.cert_hash == other.cert_hash
    }
}

impl Eq for ServerTlsConfig {}
```

- [ ] **Step 3: Compute cert_hash in ServerTlsConfig::new()**

In `src/config/tls.rs`, at the top add:
```rust
use sha2::{Digest, Sha256};
```

In `ServerTlsConfig::new()`, before the final `Ok(...)` (approximately line 226-232), compute the hash:

```rust
        let cert_hash = {
            let mut hasher = Sha256::new();
            if let Some(ca_path) = ca_cert {
                if let Ok(data) = read_file(ca_path) {
                    hasher.update(&data);
                }
            }
            if let Some(cert_path) = client_cert {
                if let Ok(data) = read_file(cert_path) {
                    hasher.update(&data);
                }
            }
            if let Some(key_path) = client_key {
                if let Ok(data) = read_file(key_path) {
                    hasher.update(&data);
                }
            }
            Some(hasher.finalize().into())
        };
```

Update the `Disable` early return to include `cert_hash: None`:
```rust
        if mode == ServerTlsMode::Disable {
            return Ok(ServerTlsConfig {
                mode,
                connector: None,
                cert_hash: None,
            });
        }
```

And the final return:
```rust
        Ok(ServerTlsConfig {
            mode,
            connector: Some(connector),
            cert_hash,
        })
```

Also update every place that constructs `ServerTlsConfig` directly (grep for `ServerTlsConfig {`):
- `src/config/address.rs` default: add `cert_hash: None`
- `src/pool/server_pool.rs` allow retry: add `cert_hash: self.address.server_tls.cert_hash`

- [ ] **Step 4: Add sha2 dependency**

Run: `cargo add sha2`

- [ ] **Step 5: Include TLS config in pool comparison**

In `src/pool/mod.rs`, in the pool reload loop (around line 328-335), the current comparison is:

```rust
if pool.config_hash == new_pool_hash_value {
    info!("[{}@{}] config unchanged", user.username, pool_name);
    new_pools.insert(identifier.clone(), pool.clone());
    continue;
}
```

Change to also compare TLS config:

```rust
if pool.config_hash == new_pool_hash_value
    && pool.address.server_tls.as_ref() == server_tls_config.as_ref()
{
    info!("[{}@{}] config unchanged", user.username, pool_name);
    new_pools.insert(identifier.clone(), pool.clone());
    continue;
}
```

Do the same for the dynamic pool reload path if it exists (check `src/pool/mod.rs` for similar patterns around line 619).

- [ ] **Step 6: Add logging for TLS cert change**

In the same reload block, after the comparison, add a log when TLS specifically changed:

```rust
if pool.config_hash == new_pool_hash_value
    && pool.address.server_tls.as_ref() != server_tls_config.as_ref()
{
    info!(
        "[{}@{}] tls certificates changed on disk, recreating pool",
        user.username, pool_name
    );
}
```

Place this before the existing `info!("[{}@{}] creating pool", ...)` line.

- [ ] **Step 7: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 8: Commit**

```
Hot reload TLS certificates on SIGHUP

Certificate files can change on disk (cert-manager, vault) without the
config file itself changing. Previously, only a config text change triggered
pool recreation.

Now ServerTlsConfig stores a SHA-256 hash of the certificate file contents.
On SIGHUP, cert files are re-read, hashed, and compared. If file contents
changed, the pool is recreated with a fresh TlsConnector. New connections
use updated certificates; active connections finish their lifecycle
naturally.
```

---

## Task 10: Add Prometheus metrics for server-side TLS

**Files:**
- Modify: `src/prometheus/mod.rs` (register 3 new metrics)
- Modify: `src/prometheus/metrics.rs` (update functions)
- Modify: `src/server/stream.rs` (observe handshake duration/errors)
- Modify: `src/stats/server.rs` (increment/decrement gauge)

- [ ] **Step 1: Register metrics in prometheus/mod.rs**

In `src/prometheus/mod.rs`, add after the existing metric definitions (find the last `Lazy<...>` block and add after it):

```rust
pub(crate) static SHOW_SERVER_TLS_CONNECTIONS: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_server_tls_connections",
            "Current number of backend connections using TLS encryption, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static SHOW_SERVER_TLS_HANDSHAKE_DURATION: Lazy<prometheus::HistogramVec> =
    Lazy::new(|| {
        let histogram = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new(
                "pg_doorman_server_tls_handshake_duration_seconds",
                "Duration of TLS handshakes to backend PostgreSQL servers, by pool.",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5]),
            &["pool"],
        )
        .unwrap();
        REGISTRY.register(Box::new(histogram.clone())).unwrap();
        histogram
    });

pub(crate) static SHOW_SERVER_TLS_HANDSHAKE_ERRORS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_server_tls_handshake_errors_total",
            "Total number of failed TLS handshakes to backend servers, by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});
```

Add `HistogramVec` to the prometheus import at the top if needed.

- [ ] **Step 2: Update server metrics export**

In `src/prometheus/metrics.rs`, in `update_server_metrics()` (lines 129-156), add TLS gauge update:

After the existing loop, add:

```rust
    // Server TLS connections gauge: count TLS connections per pool
    SHOW_SERVER_TLS_CONNECTIONS.reset();
    for (_, server) in &stats {
        if server.tls() {
            let pool_name = server.pool_name().to_string();
            SHOW_SERVER_TLS_CONNECTIONS
                .with_label_values(&[&pool_name])
                .inc();
        }
    }
```

Add import at the top of `metrics.rs`:
```rust
use super::{SHOW_SERVER_TLS_CONNECTIONS, SHOW_SERVER_TLS_HANDSHAKE_DURATION, SHOW_SERVER_TLS_HANDSHAKE_ERRORS};
```

- [ ] **Step 3: Record handshake metrics in stream.rs**

In `src/server/stream.rs`, the handshake timing and error is already measured. Update `create_tcp_stream_inner` to accept `pool_name: &str` parameter and record metrics.

Update function signature (line 142):
```rust
pub(crate) async fn create_tcp_stream_inner(
    host: &str,
    port: u16,
    server_tls: &ServerTlsConfig,
    pool_name: &str,
) -> Result<StreamInner, Error> {
```

In the 'S' match arm, after successful handshake (around line 200-206):
```rust
                Ok(tls_stream) => {
                    let elapsed = start.elapsed();
                    log::info!(
                        "tls connection established, host={host} port={port} server_tls_mode={} handshake_ms={:.1}",
                        server_tls.mode,
                        elapsed.as_secs_f64() * 1000.0
                    );
                    crate::prometheus::SHOW_SERVER_TLS_HANDSHAKE_DURATION
                        .with_label_values(&[pool_name])
                        .observe(elapsed.as_secs_f64());
                    Ok(StreamInner::TCPTls { stream: tls_stream })
                }
```

In the handshake error arm (around line 208-217):
```rust
                Err(err) => {
                    let elapsed = start.elapsed();
                    error!(
                        "tls handshake failed, host={host} port={port} server_tls_mode={} handshake_ms={:.1}: {err}",
                        server_tls.mode,
                        elapsed.as_secs_f64() * 1000.0
                    );
                    crate::prometheus::SHOW_SERVER_TLS_HANDSHAKE_ERRORS
                        .with_label_values(&[pool_name])
                        .inc();
                    Err(Error::SocketError(format!(
                        "tls handshake failed, host={host} port={port}: {err}"
                    )))
                }
```

In the 'N' + requires_tls error arm (around line 221-229):
```rust
                error!(
                    "tls required but server does not support tls, host={host} port={port} server_tls_mode={}",
                    server_tls.mode
                );
                crate::prometheus::SHOW_SERVER_TLS_HANDSHAKE_ERRORS
                    .with_label_values(&[pool_name])
                    .inc();
```

- [ ] **Step 4: Update callers of create_tcp_stream_inner**

Grep for `create_tcp_stream_inner(` and add the `pool_name` argument to each call site. The pool_name is available via `address.pool_name` in the calling context.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 6: Commit**

```
Add Prometheus metrics for server-side TLS

Three new metrics for backend TLS observability:
- pg_doorman_server_tls_connections: gauge of current TLS backend connections
- pg_doorman_server_tls_handshake_duration_seconds: histogram of handshake
  latency (buckets from 1ms to 2.5s covering LAN and WAN)
- pg_doorman_server_tls_handshake_errors_total: counter of failed handshakes
  and require-mode rejections

All labeled by pool name for per-pool alerting.
```

---

## Task 11: Implement FromStr for ServerTlsMode

**Files:**
- Modify: `src/config/tls.rs:57-67` (replace from_string with FromStr)
- Modify: `src/pool/mod.rs:103` (update call site)
- Modify: `src/config/mod.rs:448,497` (update call sites)

- [ ] **Step 1: Replace from_string with FromStr impl**

In `src/config/tls.rs`, replace the `from_string` method (lines 57-67):

Old:
```rust
    pub fn from_string(s: &str) -> Result<Self, Error> {
        match s.to_lowercase().as_str() {
            "disable" => Ok(Self::Disable),
            "allow" => Ok(Self::Allow),
            "prefer" => Ok(Self::Prefer),
            "require" => Ok(Self::Require),
            "verify-ca" => Ok(Self::VerifyCa),
            "verify-full" => Ok(Self::VerifyFull),
            _ => Err(Error::BadConfig(format!("invalid server_tls_mode: {s}"))),
        }
    }
```

New (add as a standalone impl block below the `impl ServerTlsMode` block):

```rust
impl std::str::FromStr for ServerTlsMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "disable" => Ok(Self::Disable),
            "allow" => Ok(Self::Allow),
            "prefer" => Ok(Self::Prefer),
            "require" => Ok(Self::Require),
            "verify-ca" => Ok(Self::VerifyCa),
            "verify-full" => Ok(Self::VerifyFull),
            _ => Err(Error::BadConfig(format!("invalid server_tls_mode: {s}"))),
        }
    }
}
```

Remove the `from_string` method from the `impl ServerTlsMode` block.

- [ ] **Step 2: Update all call sites**

Replace `ServerTlsMode::from_string(x)` with `x.parse::<ServerTlsMode>()` in:
- `src/pool/mod.rs:103` (in `build_server_tls_for_pool`)
- `src/config/mod.rs:448` (global validation)
- `src/config/mod.rs:497` (per-pool validation)

- [ ] **Step 3: Run tests**

Run: `cargo test --lib`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 4: Commit**

```
Use FromStr instead of custom from_string for ServerTlsMode

Idiomatic Rust: implement std::str::FromStr so callers can use
.parse::<ServerTlsMode>() instead of the non-standard from_string().
```

---

## Task 12: Minor fixes batch

**Files:**
- Modify: `src/pool/gc.rs:47-54` (comment already exists, verify or improve)
- Modify: `src/stats/server.rs:534-542` (remove inline(always))
- Modify: `src/server/stream.rs:238-241` (ErrorResponse handling)
- Modify: `src/server/stream.rs` (unify log style)
- Modify: `src/server/server_backend.rs` (unify log style if needed)

- [ ] **Step 1: Improve GC grace period comment**

In `src/pool/gc.rs`, verify the existing comment at lines 47-51. If it doesn't mention WAN/allow-mode, update to:

```rust
            // Grace period: TLS handshake adds 1-2 RTT to connection setup.
            // With allow-mode retry (two sequential connects), total setup
            // can take >1s over WAN. 2s covers this with margin while keeping
            // GC responsive for abandoned pools.
```

- [ ] **Step 2: Remove #[inline(always)] from trivial getters**

In `src/stats/server.rs`, check lines around 534-543 for `#[inline(always)]` on `set_tls` and `tls`. Remove the attribute if present — the compiler inlines trivial methods automatically.

- [ ] **Step 3: Add ErrorResponse ('E') handling in SSLRequest**

In `src/server/stream.rs`, replace the catch-all arm (lines 238-241):

Old:
```rust
        other => Err(Error::SocketError(format!(
            "unexpected tls negotiation response={} (0x{:02x}) host={host} port={port}",
            other, other as u8
        ))),
```

New:
```rust
        'E' => Err(Error::SocketError(format!(
            "server sent error response to ssl request, \
             likely does not support ssl or is not a postgresql server, \
             host={host} port={port}"
        ))),
        other => Err(Error::SocketError(format!(
            "unexpected tls negotiation response={} (0x{:02x}) host={host} port={port}",
            other, other as u8
        ))),
```

- [ ] **Step 4: Unify log style in stream.rs**

In `src/server/stream.rs`, ensure all logging uses `log::debug!`, `log::info!`, `log::error!` (fully qualified). Check for any bare `debug!`, `info!`, `error!` and prefix with `log::` if `use log::...` is not imported.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib`
Run: `cargo fmt`
Run: `cargo clippy -- --deny "warnings"`

- [ ] **Step 6: Commit**

```
Minor TLS fixes: GC comment, ErrorResponse handling, style cleanup

- Improved GC grace period comment explaining the 2s threshold rationale
- Removed unnecessary #[inline(always)] from trivial stats getters
- Added explicit 'E' (ErrorResponse) handling in SSLRequest negotiation
  for clearer diagnostics when server doesn't support SSL
- Unified logging to fully qualified log::debug!/info!/error! in stream.rs
```
