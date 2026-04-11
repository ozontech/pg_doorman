# TLS connection migration during graceful reload

## Problem

On SIGUSR2, the old pg_doorman spawns a new process and passes the listener socket fd via `--inherit-fd`. New connections go to the new process. Existing connections stay on the old process until clients disconnect.

Plain TCP connections can stay on the old process indefinitely. TLS connections force the old process to keep running until every TLS client disconnects:

- Long-lived connections (connection pools, monitoring, admin sessions) block the old process from exiting
- Two processes run in parallel, doubling memory and backend connection usage
- If the reload was triggered by a security fix, the old vulnerable binary stays alive

**Goal:** transfer established TLS client connections to the new process so the old process can exit after handoff.

## Current architecture

```
Old process                              New process
─────────────                            ───────────
listener fd ──── --inherit-fd ─────────► listener fd
                                         (accepts new connections)

TLS client A ──► stays on old process
TLS client B ──► stays on old process
plain client C ─► stays on old process    (no client migration today)
```

The listener fd passes via fork+exec with `FD_CLOEXEC` cleared. The old process waits for readiness via a pipe (`PG_DOORMAN_READY_FD`). See `src/app/server.rs`.

## Why this is hard

A TLS connection has two layers of state:

1. **TCP socket** in the kernel. Passes between processes via `SCM_RIGHTS` or fd inheritance.
2. **TLS crypto state** in userspace (OpenSSL). Opaque structures, no public API for serialization.

An active TLS 1.3 AES-256-GCM connection needs per direction:

| Field | Size | Where in OpenSSL |
|-------|------|-------------------|
| Symmetric key | 32 B | `EVP_CIPHER_CTX` internals (key schedule, not raw key) |
| IV | 12 B | `EVP_CIPHER_CTX_get_updated_iv()` (OpenSSL 3.x) |
| Record sequence number | 8 B | `ssl->s3->write_sequence` / `read_sequence` (private) |
| Traffic secret | 48 B | `ssl_st` internals (for KeyUpdate) |

~200 bytes per connection. OpenSSL has no API to extract or inject any of it.

## Approaches considered

### A. Drain (status quo)

HAProxy, Envoy, PgBouncer all drain. None migrate TLS state.

- **Pro:** zero complexity
- **Con:** old process lingers, dual resource usage, does not solve the problem

### B. kTLS (kernel TLS) + fd passing

`SSL_OP_ENABLE_KTLS` pushes crypto state into the kernel. The fd carries TLS state through `SCM_RIGHTS`. The new process reads/writes plaintext via `send()`/`recv()`.

**Tested, rejected.** kTLS creates one TLS record per `write()`. PostgreSQL wire protocol produces many small messages (5-byte headers, short responses). Each becomes a TLS record with 5B header + 16B GCM tag. Measured throughput dropped ~30% on a pgbench workload.

`TCP_CORK` / `MSG_MORE` / `writev()` do not help: kTLS operates per-write at the TLS layer, before TCP coalescing.

Application-level buffering (coalesce messages into one `write()`) would fix the overhead but requires rewriting the write path and adds latency.

- **Pro:** no OpenSSL patch
- **Con:** per-write TLS record overhead; write path rewrite

### C. Userspace record layer

Extract keys from OpenSSL (patch required), pass fd + keys, implement TLS record encode/decode with `EVP_AEAD`. No `SSL` object in the new process.

- **Pro:** full control over buffering, export-only patch
- **Con:** ~200 lines of record layer to maintain, must handle alerts and KeyUpdate (TLS 1.3)

### D. OpenSSL patch: export/import crypto state (chosen)

Add two functions to OpenSSL that serialize and deserialize the symmetric cipher state of an established connection. The new process gets an `SSL` object ready for `SSL_read`/`SSL_write`, identical to one that completed a handshake.

- **Pro:** pg_doorman proxy code unchanged; migrated connection is a normal `SslStream`
- **Con:** touches OpenSSL internals, must track struct layout changes across versions

## Design: OpenSSL crypto state export/import

### High-level flow

```
Old process                                    New process
─────────────                                  ───────────

1. SSL_export_crypto_state(ssl)
   → {tx_key, tx_iv, tx_seq,
      rx_key, rx_iv, rx_seq,
      cipher, version}

2. sendmsg(unix_socket):
   - SCM_RIGHTS: client tcp fd
   - payload: serialized crypto state
   + pg_doorman app state (pool, user, params)

                                    3. recvmsg(unix_socket):
                                       - fd + crypto state + app state

                                    4. SSL_import_crypto_state(ctx, fd, state)
                                       → SSL* ready for SSL_read/SSL_write

                                    5. Wrap in tokio SslStream, continue proxying
```

### What the patch does

#### Key retention problem

`EVP_CipherInit_ex()` runs AES key expansion and stores only the **key schedule** (round keys), not the original key. The raw key is discarded. Migration needs the raw key.

Two extraction points where the raw key is still available:

1. `ktls_configure_crypto()` (`ssl/record/methods/ktls_meth.c`, OpenSSL 3.x) extracts the raw key into `tls_crypto_info_all` right before `setsockopt()`. The export function follows the same extraction logic.
2. `tls13_change_cipher_state()` / `tls1_change_cipher_state()` derive the key. A side field on `SSL_CONNECTION` can capture it at derivation time.

Option 2 is more reliable: it works whether or not kTLS code paths are compiled in.

#### Export function

```c
struct tls_migration_state {
    uint16_t tls_version;        // TLS_1_2 or TLS_1_3
    uint16_t cipher_id;          // e.g. TLS1_3_CK_AES_256_GCM_SHA384
    struct {
        uint8_t key[32];         // raw symmetric key
        uint8_t iv[12];          // current IV
        uint8_t rec_seq[8];      // record sequence number
        uint8_t traffic_secret[48]; // TLS 1.3 only, for KeyUpdate
    } tx, rx;
    // buffered data that OpenSSL has read from socket but not yet returned
    uint32_t pending_app_data_len;
    uint8_t  pending_app_data[];
};

int SSL_export_migration_state(SSL *ssl,
                               struct tls_migration_state **out,
                               size_t *out_len);
```

Extraction steps (following `ktls_configure_crypto()` pattern):
1. Cipher: `s->s3.tmp.new_cipher` or from the record layer
2. IV: `EVP_CIPHER_CTX_get_updated_iv(ctx, iv, sizeof(iv))`
3. Sequence numbers: `memcpy(seq, s->s3.read_sequence, 8)` (internal access)
4. Key: from side-storage field (see key retention above)
5. Traffic secret (TLS 1.3): `s->client_app_traffic_secret` / `s->server_app_traffic_secret`
6. Pending data: `SSL_pending()` + `SSL_peek()`

#### Import function

```c
SSL *SSL_import_migration_state(SSL_CTX *ctx,
                                int fd,
                                const struct tls_migration_state *state);
```

Steps:

```c
// 1. Create SSL object, attach socket
SSL *ssl = SSL_new(ctx);
SSL_set_fd(ssl, fd);

// 2. Initialize cipher contexts with raw keys
EVP_CipherInit_ex(ssl->enc_write_ctx,
                   cipher_by_id(state->cipher_id),
                   NULL, state->tx.key, state->tx.iv, /*encrypt=*/1);
EVP_CipherInit_ex(ssl->enc_read_ctx,
                   cipher_by_id(state->cipher_id),
                   NULL, state->rx.key, state->rx.iv, /*encrypt=*/0);

// 3. Restore sequence numbers
memcpy(ssl->s3.write_sequence, state->tx.rec_seq, 8);
memcpy(ssl->s3.read_sequence, state->rx.rec_seq, 8);

// 4. Mark handshake complete
ssl->statem.hand_state = TLS_ST_OK;
ssl->version = state->tls_version;

// 5. Inject pending app data into read buffer (if any)
// 6. Return ready SSL*
```

### Wire protocol: old process → new process

The old process sends each client over a Unix domain socket via `sendmsg()`/`recvmsg()`:

```
┌─────────────────────────────────────────────────────┐
│ sendmsg ancillary data (cmsg):                      │
│   SCM_RIGHTS: [client_tcp_fd]                       │
├─────────────────────────────────────────────────────┤
│ Message payload:                                    │
│                                                     │
│ ┌─ Header ────────────────────────────────────────┐ │
│ │ magic: u32 = 0x50474430  ("PGD0")              │ │
│ │ version: u16 = 1                               │ │
│ │ total_len: u32                                 │ │
│ ├─ TLS State ─────────────────────────────────────┤ │
│ │ has_tls: u8 (0 = plain, 1 = tls)              │ │
│ │ tls_version: u16                               │ │
│ │ cipher_id: u16                                 │ │
│ │ tx_key: [u8; 32]                               │ │
│ │ tx_iv: [u8; 12]                                │ │
│ │ tx_seq: [u8; 8]                                │ │
│ │ rx_key: [u8; 32]                               │ │
│ │ rx_iv: [u8; 12]                                │ │
│ │ rx_seq: [u8; 8]                                │ │
│ │ tx_traffic_secret: [u8; 48] (TLS 1.3 only)    │ │
│ │ rx_traffic_secret: [u8; 48] (TLS 1.3 only)    │ │
│ │ pending_data_len: u32                          │ │
│ │ pending_data: [u8; pending_data_len]           │ │
│ ├─ App State ─────────────────────────────────────┤ │
│ │ username_len: u16                              │ │
│ │ username: [u8]                                 │ │
│ │ database_len: u16                              │ │
│ │ database: [u8]                                 │ │
│ │ transaction_mode: u8                           │ │
│ │ server_parameters_count: u16                   │ │
│ │   [key_len: u16][key][value_len: u16][value].. │ │
│ │ process_id: u32   (BackendKeyData sent to      │ │
│ │ secret_key: u32    client, for cancel protocol) │ │
│ ├─ Prepared Statements ──────────────────────────┤ │
│ │ prepared_enabled: u8                           │ │
│ │ async_client: u8                               │ │
│ │ cache_count: u32                               │ │
│ │   for each entry:                              │ │
│ │     key_type: u8 (0=Named, 1=Anonymous)        │ │
│ │     key_name/key_hash: variable                │ │
│ │     hash: u64                                  │ │
│ │     query_len: u32, query: [u8]                │ │
│ │     num_params: i16, params: [i32; N]          │ │
│ │ addr_port: u16, addr_ip_len: u8, addr_ip: [u8] │ │
│ └─────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────┘
```

### Rust API (`patches/rust-native-tls`)

```rust
impl TlsStream<TcpStream> {
    /// Extracts TLS crypto state. Consumes the stream; the SSL object
    /// is invalidated after this call.
    pub fn export_migration_state(self) -> Result<(TcpStream, TlsMigrationState), Error>;

    /// Reconstructs a TLS stream from migrated state. No handshake occurs.
    pub fn import_migration_state(
        acceptor: &TlsAcceptor,
        stream: TcpStream,
        state: &TlsMigrationState,
    ) -> Result<TlsStream<TcpStream>, Error>;
}
```

Migrated connections enter `src/client/entrypoint.rs` through a separate path that skips authentication and pool lookup:

```rust
async fn client_entrypoint_migrated(
    stream: TcpStream,
    tls_state: Option<TlsMigrationState>,
    app_state: MigratedClientState,
) {
    let stream = match tls_state {
        Some(tls) => TlsStream::import_migration_state(&tls_acceptor, stream, &tls)?,
        None => stream,  // plain TCP
    };
    // From here: identical to normal client handling.
    // Client is already authenticated, pool is assigned.
}
```

### Supported ciphers

AEAD ciphers only. Non-AEAD ciphers (CBC-mode) are not worth supporting; they are rare with TLS 1.2+ and pg_doorman already requires TLS 1.2 minimum.

| Cipher | TLS version | Key | IV |
|--------|-------------|-----|-----|
| AES-128-GCM | 1.2, 1.3 | 16B | 12B |
| AES-256-GCM | 1.2, 1.3 | 32B | 12B |
| ChaCha20-Poly1305 | 1.2, 1.3 | 32B | 12B |

These three cover >99% of real connections.

### TLS 1.2 vs 1.3 differences

**TLS 1.2 GCM:** nonce = 4-byte salt || 8-byte explicit nonce (= sequence number). Keys and IV are derived once during handshake, never change.

**TLS 1.3 GCM:** nonce = 12-byte IV XOR 8-byte sequence number (zero-padded). Traffic secrets support KeyUpdate (RFC 8446 §4.6.3). Either side can rotate keys at any time.

### KeyUpdate (TLS 1.3)

The migration state includes traffic secrets for this reason. On KeyUpdate:

1. `new_secret = HKDF-Expand-Label(current_secret, "traffic upd", "", Hash.length)`
2. New key and IV derived from `new_secret`
3. Sequence number resets to 0

OpenSSL handles this automatically once the `SSL` object has the traffic secrets in the right internal fields. The import function places them there.

## Migration sequence

```
Old process                                         New process
─────────────                                       ───────────

 SIGUSR2 received
 │
 ├─ Validate new binary config
 ├─ Create Unix domain socket pair
 ├─ Fork + exec new binary with:
 │    --inherit-fd <listener_fd>
 │    --migration-socket <unix_fd>
 │    PG_DOORMAN_READY_FD=<pipe_fd>
 │
 │                                        Start up, bind listener
 │                                        Signal readiness via pipe
 │
 ├─ Receive readiness signal
 ├─ Stop accepting new connections
 │
 ├─ For each connected client:
 │    ├─ Wait for idle state (between queries)
 │    ├─ SSL_export_migration_state(ssl)
 │    │   → TcpStream + TlsMigrationState
 │    ├─ Serialize app state (user, db, params...)
 │    ├─ sendmsg(migration_socket):
 │    │    ancillary: SCM_RIGHTS [tcp_fd]
 │    │    payload: tls_state + app_state
 │    ├─ Close local fd
 │    └─ Remove client from tracking
 │                                        recvmsg(migration_socket)
 │                                        SSL_import_migration_state()
 │                                        Register client, resume proxying
 │
 ├─ All clients migrated
 ├─ Close migration socket
 ├─ Drain backend connections
 └─ Exit
```

### When to migrate a client

A client is eligible for migration at the same point where the pool would check in a server connection:

- `transaction` mode: after `ReadyForQuery` with status `'I'` (idle, no open transaction)
- `session` mode: at any `ReadyForQuery`
- Never during an active query or extended query pipeline

Clients in `session` mode with an open transaction wait for the transaction to complete. If `shutdown_timeout` expires first, they get force-closed (existing behavior).

## Risks

| Risk | Mitigation |
|------|------------|
| OpenSSL struct layout changes on upgrade | Pin OpenSSL version. CI test: handshake → export → import → echo → verify. |
| Sequence number desync | Same CI test. A single wrong byte corrupts the GCM tag; the test catches it. |
| Pending data in OpenSSL read buffer | Export drains via `SSL_pending()` + `SSL_peek()`. Import injects into read buffer. |
| Client sends data during migration | Migration only at idle point. `TCP_CORK` on the fd before export prevents sends in flight. |
| TLS 1.3 KeyUpdate in transit | Drain OpenSSL read buffer fully before export to process any pending KeyUpdate. |

## Implementation phases

### Phase 1: OpenSSL patch

1. Add `tls_migration_state` struct to public headers
2. Key retention: save raw key material in `tls1_change_cipher_state()` / `tls13_change_cipher_state()` into a new field on `SSL_CONNECTION`
3. `SSL_export_migration_state()` following `ktls_configure_crypto()` pattern
4. `SSL_import_migration_state()` with `EVP_CipherInit_ex` + internal state setup
5. Test: handshake → export → import on new fd → bidirectional echo → verify

### Phase 2: native-tls FFI

1. FFI bindings for export/import in `patches/rust-native-tls`
2. `TlsMigrationState` Rust struct with `Serialize`/`Deserialize`
3. `TlsStream<TcpStream>::export_migration_state()` / `import_migration_state()`

### Phase 3: migration protocol

1. Unix domain socket pair created during reload
2. `sendmsg`/`recvmsg` with `SCM_RIGHTS`
3. Serialization format for TLS state + app state
4. Receiver loop in new process

### Phase 4: client migration

1. Detect idle clients eligible for migration
2. `TCP_CORK` the fd, stop reading
3. Export TLS + app state, send over migration socket, close local fd
4. New process: reconstruct `Client`, resume proxying

### Phase 5: plain TCP migration (implement first)

Plain TCP connections use the same protocol without TLS state. Implement this first as a test of the migration socket, app state serialization, and client reconstruction.

## Prepared statements

Prepared statements exist in three caches simultaneously:

1. **Client cache** (`PreparedStatementCache` per client) maps client-facing name → `CachedStatement` (internal name, query text, param types, hash)
2. **Pool cache** (`DashMap<u64, Arc<Parse>>` per pool) deduplicates statements across clients by query hash
3. **Server cache** (`LruCache<String, ()>` per backend connection) tracks which statements are registered on the PostgreSQL backend

On `Bind("my_stmt")`, pg_doorman looks up the client cache (1), rewrites the name to the internal `DOORMAN_N`, and checks if it exists on the current server (3). If not, it sends `Parse` to register it.

### Why client cache must be serialized

Without the client cache, `process_bind_immediate()` returns `"prepared statement does not exist"` for any Bind referencing a previously prepared statement. The client receives an error it does not expect.

### What to serialize per client

For each entry in the client cache:
- Key: `Named(String)` or `Anonymous(u64)`
- Query text (`Arc<str>` in `Parse`)
- Parameter types (`Vec<i32>` in `Parse`)
- Hash (`u64`)

NOT serialized:
- Internal statement names (`DOORMAN_N`) — the new process assigns its own via `register_parse_to_cache()`
- `async_name` — async clients get fresh unique names
- Batch state (`skipped_parses`, `batch_operations`) — empty at idle point

### Reconstruction in new process

1. Deserialize each entry into `(key, query, param_types, hash)`
2. Build a `Parse` struct
3. Call `pool.register_parse_to_cache(hash, &parse)` — pool assigns a new internal name (`DOORMAN_M`), returns shared `Arc<Parse>`
4. Store in client's `PreparedStatementCache` under the original key as `CachedStatement`

The pool-level cache and `PREPARED_STATEMENT_COUNTER` are not migrated. The new process has its own counter and cache. Internal names diverge (`DOORMAN_N` in old vs `DOORMAN_M` in new), but client-facing names stay the same. The Bind rewriting layer maps client names to internal names transparently.

### Session mode

The client holds a dedicated backend connection with statements registered on PostgreSQL. Without migrating the backend connection, those server-side statements are gone. After migration, the client gets a new backend. On the first `Bind`, `ensure_prepared_statement_is_on_server()` detects the statement is missing on the new server and sends `Parse` to register it. The client cache provides the query text needed.

### Async clients

Async clients (those that sent `Flush`) have caching disabled (`prepared.enabled = false`). All `Parse`/`Bind` messages pass through without rewriting. Migration serializes the `async_client` flag. In the new process, the flag is restored and caching stays disabled.

## Open questions

- **Backend connections.** Migrate the server-side PostgreSQL connections too, or check out fresh ones? Migrating avoids re-auth but doubles the complexity. If `server_tls` is ever implemented, it hits the same export/import problem. For now: fresh backend connections in the new process.
- **Cancel protocol.** The `BackendKeyData` (process_id, secret_key) the client received refers to the old process. After migration, cancel requests arrive at the new process with the old mapping. `ClientServerMap` in the new process must import these entries.
- **Metrics.** Connection age, query count, bytes transferred. Transfer them for observability continuity, or reset to zero? Leaning toward transfer.
