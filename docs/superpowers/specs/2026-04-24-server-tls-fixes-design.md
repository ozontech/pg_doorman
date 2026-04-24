# Server-side TLS: fixes, hardening, observability

Fixes for issues found during deep review of the `feature/server-side-tls` branch by three independent agents (Rust architect, DevOps, DBA).

## Scope

18 items total: 3 critical bugs, 5 important fixes, 1 feature (hot reload), full Prometheus metrics, 8 minor fixes.

### Out of scope

- `server_tls_ciphers` configuration (deferred to separate PR)
- `prefer` mode retry on new socket after TLS handshake failure (documented as known limitation)

## Commit strategy

Approach B: atomic commits in current branch, ordered by priority. Each commit is self-contained and independently revertable.

---

## 1. Critical bugs

### 1.1 Stats leak in allow-mode retry

**File:** `src/pool/server_pool.rs:160-240`

**Bug:** Two `ServerStats` objects are registered in global `SERVER_STATS` during allow-mode retry. On successful retry, the original `stats` is never disconnected (ghost entry in SHOW SERVERS, memory leak). On double failure, `retry_stats` is never disconnected.

**Fix:**
- Before retry: call `stats.disconnect()` to clean up the first registered stats
- Lift `retry_stats` into an `Option<Arc<ServerStats>>` visible in the `match result` block
- In `Err` branch: disconnect whichever stats was last active (`retry_stats` if retry happened, `stats` otherwise)

### 1.2 try_write via Waker::noop() in auth path

**Files:** `src/server/stream.rs:90-99`, `src/server/authentication.rs:148,201`

**Bug:** `try_write` for TCPTls uses `poll_write` with `Waker::noop()`. If the TLS write returns `Pending` (TCP buffer full), authentication fails with `WouldBlock`. Under load this causes spurious auth failures.

**Fix:**
- Add `pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>` to `StreamInner` using `tokio::io::AsyncWriteExt::write_all` for all variants
- Replace `stream.try_write(...)` with `stream.write_all(...).await` in `authentication.rs` (JWT line 148, MD5 line 201)
- Keep `try_write` for `Drop` path (`server_backend.rs:995`, Terminate message) where async is impossible and message loss is acceptable

### 1.3 readable()/try_read() bypass TLS layer

**File:** `src/server/stream.rs:109,120`, `src/server/server_backend.rs:275-280`

**Bug:** For TCPTls, `readable()` and `try_read()` operate on raw TCP socket via `.get_ref().get_ref().get_ref()`. `try_read` on raw socket consumes bytes that TLS layer has not processed, potentially corrupting the TLS session.

**Fix:**
- Add `pub fn is_tls(&self) -> bool` to `StreamInner`
- `readable()`: keep on raw socket (if raw TCP becomes readable on idle connection, something happened)
- `try_read()`: for TLS, do NOT call. In `check_server_alive()`, if `is_tls()` and `readable()` fired, return `false` (dead) without `try_read()` verification
- Add comment explaining: PostgreSQL does not send unsolicited data on idle connections, and TLS renegotiation is disabled since PG14. Raw socket readiness on idle TLS connection means server disconnect.

---

## 2. Important fixes

### 2.1 CA bundle support

**File:** `src/config/tls.rs:148-164`

**Problem:** `Certificate::from_pem()` loads only the first PEM block. CA files with intermediate chains are not fully loaded.

**Fix:**
- Replace `load_certificate()` with `load_certificates() -> Vec<Certificate>`
- Split PEM file by `-----BEGIN CERTIFICATE-----` / `-----END CERTIFICATE-----` markers
- Call `builder.add_root_certificate()` for each certificate
- Error if file contains zero valid PEM blocks

### 2.2 Address::default() uses Prefer with connector=None

**File:** `src/config/address.rs:84-87`

**Problem:** Default is `Prefer` (mismatches documented `Allow` default). `connector: None` with `Prefer` is an invalid combination.

**Fix:** Change to `ServerTlsMode::Disable` with `connector: None`. Only used in tests where TLS is not needed.

### 2.3 CancelTarget struct

**File:** `src/pool/mod.rs:49-60`

**Problem:** 6-element positional tuple in `ClientServerMap` value type.

**Fix:**
```rust
pub struct CancelTarget {
    pub process_id: ProcessId,
    pub secret_key: SecretKey,
    pub host: ServerHost,
    pub port: ServerPort,
    pub server_tls: Arc<ServerTlsConfig>,
    pub connected_with_tls: bool,
}
```
Update: `transaction.rs` (destructuring in `handle_cancel_mode`), `server_backend.rs` (`claim()`).

### 2.4 BDD scenarios for allow mode

**File:** `tests/bdd/features/server-tls.feature`

**Problem:** Default mode with the most complex retry logic has zero BDD tests.

**Fix:** Add 2 scenarios:
1. `@server-tls-allow-retry` — server with `hostssl only`, doorman in allow mode, plain fails, TLS retry succeeds
2. `@server-tls-allow-plain` — server accepts plain, doorman in allow mode, plain succeeds, no retry

### 2.5 Document prefer mode handshake fallback limitation

**Problem:** `prefer` does not retry on plain TCP when TLS handshake fails after server responds 'S'. Diverges from libpq behavior.

**Fix:**
- Comment in `stream.rs` next to handshake error handling
- Note in README server-side TLS table
- Line in changelog

---

## 3. Hot reload TLS certificates

### Problem

Certificates are read once at startup/reload. If cert-manager/vault updates files on disk, pg_doorman keeps using old certs until restart.

### Mechanism

Use existing SIGHUP reload infrastructure. On reload, compute SHA256 hash of cert file contents and include it in pool comparison. If contents changed, pool hash changes, pool is recreated with fresh TlsConnector.

### Implementation

1. In `build_server_tls_for_pool()`: after building `ServerTlsConfig`, compute SHA256 of `ca_cert_contents || client_cert_contents || client_key_contents`
2. Store as `cert_hash: Option<[u8; 32]>` in `ServerTlsConfig`
3. Implement `PartialEq` for `ServerTlsConfig` via `mode + cert_hash`
4. Address comparison for pool recreation considers `server_tls` equality

### Result

- SIGHUP triggers config re-read, cert files re-read, hash comparison
- Changed certs cause pool recreation
- New connections use new certificates
- Existing idle connections are evicted (pool recreation)
- Active connections finish their lifecycle naturally

### Logging

```
info!("tls certificates changed on disk, pool={pool_name}")
```

### Not included

- Periodic file polling (inotify/fswatch) — only on SIGHUP
- In-place connector swap (ArcSwap) — pool recreation is sufficient

---

## 4. Prometheus metrics for server-side TLS

### New metrics

**1. `pg_doorman_server_tls_connections_total`** (IntGaugeVec)
- Labels: `pool`
- Semantics: current number of TLS backend connections per pool
- Increment: on successful TLS handshake
- Decrement: on server disconnect

**2. `pg_doorman_server_tls_handshake_duration_seconds`** (HistogramVec)
- Labels: `pool`
- Buckets: `[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5]`
- Observation point: `stream.rs`, after handshake (measurement already exists via `start.elapsed()`)

**3. `pg_doorman_server_tls_handshake_errors_total`** (IntCounterVec)
- Labels: `pool`
- Increment: on handshake failure and on require-mode server rejection

### Integration

- Metrics registered in `src/prometheus/metrics.rs` via `lazy_static!` (existing pattern)
- Pool name passed to stream creation or metrics recorded at `server_pool.rs`/`server_backend.rs` level where pool name is available
- Gauge increment/decrement via `ServerStats` (already tracks TLS status)

---

## 5. Minor fixes

### 5.1 FromStr for ServerTlsMode

**File:** `src/config/tls.rs:60`

Replace `from_string(s: &str) -> Result<Self, Error>` with `impl FromStr for ServerTlsMode`. Update call sites to `.parse::<ServerTlsMode>()`.

### 5.2 GC grace period comment

**File:** `src/pool/gc.rs:52`

Add comment explaining rationale:
```
// Grace period: TLS handshake adds 1-2 RTT to connection setup.
// With allow-mode retry (two sequential connects), total setup
// can take >1s over WAN. 2s covers this with margin while keeping
// GC responsive for abandoned pools.
```

### 5.3 Assertion in ServerTlsConfig::new()

**File:** `src/config/tls.rs:197-209`

Add early check:
```rust
if mode.requires_ca() && ca_cert.is_none() {
    return Err(Error::BadConfig(
        "verify-ca/verify-full requires server_tls_ca_cert".into()
    ));
}
```

### 5.4 Document channel_binding incompatibility

**File:** `README.md`

Add to server-side TLS section:
> **Limitation:** PostgreSQL `channel_binding = require` is incompatible with pg_doorman. The pooler uses separate TLS sessions for client-to-pooler and pooler-to-server, so SCRAM channel binding cannot be forwarded.

### 5.5 ErrorResponse ('E') handling in SSLRequest

**File:** `src/server/stream.rs:238-241`

Add explicit `'E'` branch:
```rust
'E' => Err(Error::SocketError(format!(
    "server sent error response to ssl request, \
     likely does not support ssl or is not a postgresql server, \
     host={host} port={port}"
)))
```

### 5.6 Document cipher suites limitation

**File:** `README.md`

Add to server-side TLS section:
> **Note:** Cipher suite selection is not currently configurable; system OpenSSL defaults are used. TLS 1.2 is the minimum protocol version.

### 5.7 Remove #[inline(always)] from trivial getters

**File:** `src/stats/server.rs:537,543`

Remove `#[inline(always)]` from `set_tls()` and `tls()`. Compiler inlines trivial methods automatically.

### 5.8 Unify logging style

**Files:** `src/server/stream.rs`, `src/server/server_backend.rs`

Standardize on `log::debug!` (fully qualified) in these files, matching the predominant pattern in `stream.rs`.
