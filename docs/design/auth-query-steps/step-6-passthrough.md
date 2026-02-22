# Step 6: Passthrough mode (per-user pools + backend auth)

## Goal

When `server_user` is NOT set, each dynamic user gets their own data pool.
Backend auth uses MD5 pass-the-hash or SCRAM passthrough with extracted ClientKey.

## Dependencies

- Step 5 (SCRAM ClientKey extraction)

## 6.1 Dynamic pool creation

### File: `src/pool/mod.rs`

Add function to create a dynamic pool for an auth_query user:

```rust
/// Create a dynamic data pool for an auth_query user (passthrough mode).
/// Called on first successful auth for a user not in static config.
pub async fn create_dynamic_pool(
    pool_name: &str,
    username: &str,
    pool_config: &crate::config::Pool,
    aq_config: &AuthQueryConfig,
    client_server_map: ClientServerMap,
) -> Result<ConnectionPool, Error> {
    let config = get_config();
    let identifier = PoolIdentifier::new(pool_name, username);

    // Check if pool was already created (race condition protection)
    if let Some(existing) = get_pool(pool_name, username) {
        return Ok(existing);
    }

    let server_database = pool_config.server_database.clone()
        .unwrap_or_else(|| pool_name.to_string());

    let address = Address {
        database: pool_name.to_string(),
        host: pool_config.server_host.clone(),
        port: pool_config.server_port,
        username: username.to_string(),
        password: String::new(), // Not used for backend auth — handled separately
        pool_name: pool_name.to_string(),
        stats: Arc::new(AddressStats::default()),
    };

    // Create pool with dynamic user's settings
    let user = User {
        username: username.to_string(),
        password: String::new(), // Client auth handled by auth_query
        pool_size: aq_config.default_pool_size,
        min_pool_size: None,
        pool_mode: None,
        server_lifetime: None,
        server_username: None,    // Passthrough: connect as the user themselves
        server_password: None,
        auth_pam_service: None,
    };

    // ... build ServerPool, ConnectionPool similar to from_config() ...
    // ... insert into POOLS atomically ...

    // Atomic insert into global POOLS map
    let pools = POOLS.load();
    let mut new_pools = (**pools).clone();
    new_pools.insert(identifier, pool.clone());
    POOLS.store(Arc::new(new_pools));

    Ok(pool)
}
```

**Race protection:** Same per-username lock from AuthQueryCache ensures only
one pool is created per user. After lock release, others call `get_pool()` and
find it.

## 6.2 MD5 pass-the-hash

### File: `src/server/server_backend.rs` (or wherever backend auth is handled)

When PG backend requests MD5 auth and we're in passthrough mode:

```
PG sends: AuthenticationMD5Password { salt }
We have: md5_hash from pg_shadow (e.g., "md5abc123...")
Compute: md5(md5_hash_without_prefix + salt_from_server)
Send: PasswordMessage with the result
```

This works because MD5 auth in PG is two-pass:
1. `pg_shadow.passwd = "md5" + md5(password + username)`  ← stored hash
2. Wire protocol: `md5(stored_hash_without_prefix + server_salt)` ← what we send

We have the stored hash from auth_query. We compute the second pass with the
server's salt. PG verifies it the same way.

```rust
/// Authenticate to PG backend using MD5 pass-the-hash.
/// Uses the md5 hash from pg_shadow to compute the wire-protocol response.
pub fn md5_pass_the_hash(stored_hash: &str, server_salt: &[u8; 4]) -> Vec<u8> {
    // stored_hash = "md5" + 32-hex-chars
    let hash_without_prefix = &stored_hash[3..]; // strip "md5" prefix
    md5_hash_second_pass(hash_without_prefix, server_salt)
}
```

## 6.3 SCRAM passthrough

### File: `src/server/server_backend.rs`

When PG backend requests SCRAM auth and we have ClientKey from client auth:

```
1. PG sends: AuthenticationSASL (requesting SCRAM-SHA-256)
2. We send: SASLInitialResponse with ClientFirstMessage
   - Generate our own client nonce
   - gs2-header + username + nonce
3. PG sends: AuthenticationSASLContinue with ServerFirstMessage
   - Contains: server nonce, salt, iterations
4. We compute:
   - SaltedPassword = ... (NOT needed — we have ClientKey directly)
   - ClientSignature = HMAC(StoredKey, AuthMessage)
     where StoredKey = H(ClientKey), AuthMessage = client_first_bare + "," + server_first + "," + client_final_without_proof
   - ClientProof = ClientKey XOR ClientSignature
5. We send: SASLResponse with ClientFinalMessage containing proof
6. PG sends: AuthenticationSASLFinal with ServerSignature
7. We verify ServerSignature (optional but recommended)
```

```rust
/// Authenticate to PG backend using SCRAM passthrough.
/// Uses ClientKey extracted from the client's SCRAM proof.
pub async fn scram_passthrough_auth(
    stream: &mut TcpStream,
    client_key: &[u8],
    username: &str,
) -> Result<(), Error> {
    // This is essentially a SCRAM client implementation
    // using a known ClientKey instead of deriving from password.

    // Use existing scram_client.rs module, modified to accept
    // ClientKey directly instead of password.
}
```

The existing `src/auth/scram_client.rs` likely has SCRAM client logic for
backend auth. Modify it to accept `ClientKey` as an alternative to password.

## 6.4 Backend auth dispatch

When establishing a new server connection for a dynamic pool in passthrough mode,
the backend auth flow needs to know HOW to authenticate:

```rust
enum BackendAuthMethod {
    /// Use password directly (static users, server_password set)
    Password(String),
    /// MD5 pass-the-hash (auth_query, MD5 hash from pg_shadow)
    Md5PassTheHash(String),  // the "md5..." hash
    /// SCRAM passthrough (auth_query, ClientKey from client's proof)
    ScramPassthrough(Vec<u8>),  // ClientKey
}
```

Store in `User` or in a per-connection session context that `ServerPool::create()`
can access.

**Challenge:** `ServerPool::create()` and `Server::startup()` are called by the
pool manager (deadpool/bb8), not directly by the auth code. The backend auth
method needs to be accessible when the pool creates a new connection.

**Solution options:**
- A) Store auth method in `User` struct (add new field)
- B) Store in `Address` struct
- C) Thread-local / task-local storage
- D) New field on `ServerPool` that holds the auth context

Option A or B is simplest — the `User` already has `server_password` for static
users. Add `backend_auth: Option<BackendAuthMethod>` to `User` or a parallel
struct.

## 6.5 Incompatible auth detection

| Client auth | Backend auth | Works? |
|-------------|-------------|--------|
| SCRAM → SCRAM | Yes (passthrough) |
| MD5 → MD5 | Yes (pass-the-hash) |
| SCRAM → MD5 | No ClientKey derivable for MD5 format |
| MD5 → SCRAM | No ClientKey available |
| Any → trust | Always works |

Detect at connection time and fail with clear error:

```rust
if backend_requests_scram && client_key.is_none() {
    return Err(Error::AuthError(
        "Backend requires SCRAM but client authenticated with MD5. \
         Cannot passthrough — consider using server_user mode.".into()
    ));
}
```

## 6.6 Server parameters for passthrough mode

First dynamic user has no data pool yet → get server params from executor pool
connections (same PG server, same params). Cache at pool level.

```rust
// In AuthQueryState:
pub server_parameters: Arc<tokio::sync::Mutex<Option<ServerParameters>>>,

// When first dynamic user needs server params:
if let Some(params) = aq_state.server_parameters.lock().await.as_ref() {
    return Ok(params.clone());
}
// Fetch from executor connection
let params = fetch_server_params_from_executor(&aq_state.executor).await?;
*aq_state.server_parameters.lock().await = Some(params.clone());
Ok(params)
```

## 6.7 BDD tests

```gherkin
@auth-query @passthrough
Scenario: MD5 passthrough — dynamic user as themselves
  Given auth_query configured WITHOUT server_user, PG uses md5
  When "alice" authenticates with MD5 password
  Then backend connection authenticates as "alice"
  And pg_stat_activity shows "alice"

@auth-query @passthrough
Scenario: SCRAM passthrough — dynamic user as themselves
  Given auth_query configured WITHOUT server_user, PG uses scram-sha-256
  When "alice" authenticates with SCRAM password
  Then backend connection authenticates as "alice" using SCRAM passthrough
  And pg_stat_activity shows "alice"

@auth-query @passthrough
Scenario: Incompatible auth — MD5 client, SCRAM backend
  Given auth_query configured WITHOUT server_user, PG uses scram-sha-256
  And user "alice" has MD5 hash in pg_shadow
  When "alice" authenticates with MD5
  Then client auth succeeds but backend connection fails with clear error

@auth-query @passthrough
Scenario: Idle pool garbage collection
  Given dynamic pool for "alice" with idle_timeout 5s
  When no activity for 6 seconds
  Then "alice" pool is destroyed
  When "alice" connects again
  Then new pool is created
```

## Checklist

- [ ] `create_dynamic_pool()` with atomic POOLS insert
- [ ] Race protection via per-username lock
- [ ] MD5 pass-the-hash in backend auth
- [ ] SCRAM passthrough using stored ClientKey
- [ ] Backend auth method dispatch (Password / Md5PassTheHash / ScramPassthrough)
- [ ] Incompatible auth detection with clear error
- [ ] Server parameters from executor pool
- [ ] BDD tests (4+)
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
