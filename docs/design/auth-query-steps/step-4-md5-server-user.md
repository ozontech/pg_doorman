# Step 4: MD5 auth + server_user mode (MVP)

## Goal

First end-to-end working auth_query: client authenticates with MD5 via auth_query,
gets a connection from a shared data pool (server_user mode). This is the MVP.

## Dependencies

- Step 1 (config), Step 2 (executor), Step 3 (cache)

## 4.1 Executor startup

### File: `src/pool/mod.rs`

In `ConnectionPool::from_config()`, after creating static pools (line ~258),
create auth_query executors and caches for pools that have `auth_query` configured.

Need a new global to store executors and caches per pool:

```rust
use crate::auth::auth_query::{AuthQueryCache, AuthQueryExecutor};

/// Global auth_query state per database pool.
pub static AUTH_QUERY_STATE: Lazy<ArcSwap<HashMap<String, Arc<AuthQueryState>>>> =
    Lazy::new(|| ArcSwap::from_pointee(HashMap::new()));

pub struct AuthQueryState {
    pub cache: AuthQueryCache,
    pub config: AuthQueryConfig,
    // For server_user mode: the shared data pool identifier
    pub shared_pool_id: Option<PoolIdentifier>,
}

/// Get auth_query state for a database.
pub fn get_auth_query_state(db: &str) -> Option<Arc<AuthQueryState>> {
    AUTH_QUERY_STATE.load().get(db).cloned()
}
```

In `from_config()`:

```rust
let mut auth_query_states = HashMap::new();

for (pool_name, pool_config) in &config.pools {
    // ... existing pool creation ...

    // Create auth_query executor if configured
    if let Some(ref aq_config) = pool_config.auth_query {
        let executor = AuthQueryExecutor::new(
            aq_config,
            pool_name,
            &pool_config.server_host,
            pool_config.server_port,
        ).await?;

        let executor = Arc::new(executor);
        let cache = AuthQueryCache::new(executor.clone(), aq_config);

        // If server_user mode: create shared data pool
        let shared_pool_id = if aq_config.is_dedicated_mode() {
            let su = aq_config.server_user.as_ref().unwrap();
            let sp = aq_config.server_password.as_ref().unwrap();
            let identifier = PoolIdentifier::new(pool_name, &format!("__aq_{su}"));
            // Create the shared pool (same as static user pool creation)
            // ... create ConnectionPool with server_user credentials ...
            Some(identifier)
        } else {
            None
        };

        auth_query_states.insert(pool_name.clone(), Arc::new(AuthQueryState {
            cache,
            config: aq_config.clone(),
            shared_pool_id,
        }));
    }
}

AUTH_QUERY_STATE.store(Arc::new(auth_query_states));
```

## 4.2 Auth flow integration

### File: `src/auth/mod.rs`

Modify `authenticate_normal_user()`. Current flow (line 192):

```rust
let mut pool = match get_pool(pool_name, client_identifier.username.as_str()) {
    Some(pool) => pool,
    None => { /* reject */ }
};
```

New flow:

```rust
let mut pool = match get_pool(pool_name, client_identifier.username.as_str()) {
    Some(pool) => pool,
    None => {
        // Static user not found — try auth_query
        match try_auth_query(
            read, write, client_identifier, pool_name, username_from_parameters
        ).await {
            Ok(pool) => pool,
            Err(err) => return Err(err),
        }
    }
};
```

New function `try_auth_query()`:

```rust
async fn try_auth_query<S, T>(
    read: &mut S,
    write: &mut T,
    client_identifier: &ClientIdentifier,
    pool_name: &str,
    username: &str,
) -> Result<ConnectionPool, Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    use crate::pool::{get_auth_query_state, get_pool};

    // 1. Check if auth_query is configured for this pool
    let aq_state = match get_auth_query_state(pool_name) {
        Some(state) => state,
        None => {
            error_response(write, "No pool configured for ...", "3D000").await?;
            return Err(Error::AuthError("No pool configured".into()));
        }
    };

    // 2. Get password hash from cache or PG
    let cache_entry = match aq_state.cache.get_or_fetch(username).await {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            // User not found
            wrong_password(write, username).await?;
            return Err(Error::AuthError(format!("auth_query: user '{username}' not found")));
        }
        Err(err) => {
            error_response(write, "Authentication service unavailable", "58000").await?;
            return Err(err);
        }
    };

    let pool_password = &cache_entry.password_hash;

    // 3. Authenticate client (MD5 only in this step)
    if pool_password.starts_with(MD5_PASSWORD_PREFIX) {
        authenticate_with_md5_aq(read, write, pool_password, username).await?;
    } else {
        // SCRAM support added in Step 5
        error_response_terminal(write, "Auth method not yet supported", "28P01").await?;
        return Err(Error::AuthError("Unsupported auth method for auth_query".into()));
    }

    // 4. Get the data pool
    //    server_user mode: return shared pool
    //    passthrough mode: create/get per-user pool (Step 6)
    if let Some(ref shared_id) = aq_state.shared_pool_id {
        match get_pool(&shared_id.db, &shared_id.user) {
            Some(pool) => Ok(pool),
            None => {
                error_response(write, "Internal pool error", "58000").await?;
                Err(Error::AuthError("Shared auth_query pool not found".into()))
            }
        }
    } else {
        // Passthrough mode — Step 6
        error_response(write, "Passthrough mode not yet implemented", "58000").await?;
        Err(Error::AuthError("Passthrough not implemented".into()))
    }
}
```

### MD5 auth with re-fetch on failure

```rust
async fn authenticate_with_md5_aq<S, T>(
    read: &mut S,
    write: &mut T,
    pool_password: &str,
    username: &str,
    // TODO: pass aq_state for re-fetch
) -> Result<(), Error> {
    let salt = md5_challenge(write).await?;
    let password_response = read_password(read).await?;
    let expected = md5_hash_second_pass(pool_password.strip_prefix("md5").unwrap(), &salt);

    if expected == password_response {
        return Ok(());
    }

    // Auth failed — attempt re-fetch (password may have changed)
    // ... re-fetch logic using aq_state.cache.refetch_on_failure(username)
    // ... if new hash differs, recompute expected and compare again
    // ... for MD5, re-check works within same connection (same salt)

    wrong_password(write, username).await?;
    Err(Error::AuthError(format!("MD5 auth failed for user: {username}")))
}
```

## 4.3 Server parameters

For server_user mode, the shared pool is created eagerly and has server params.
The existing `pool.get_server_parameters()` works as-is.

## 4.4 BDD tests

These require Docker PG setup. Tag: `@auth-query`.

```gherkin
@auth-query
Scenario: Auth query MD5 — valid password
  Given pg_doorman is configured with auth_query for pool "testdb"
  And user "aq_user" exists in PostgreSQL with MD5 password "secret"
  When client connects as "aq_user" with password "secret"
  Then authentication succeeds
  And client can execute queries

@auth-query
Scenario: Auth query MD5 — wrong password
  Given pg_doorman is configured with auth_query for pool "testdb"
  And user "aq_user" exists in PostgreSQL with MD5 password "secret"
  When client connects as "aq_user" with password "wrong"
  Then authentication fails

@auth-query
Scenario: Auth query — user not found
  Given pg_doorman is configured with auth_query for pool "testdb"
  When client connects as "nonexistent" with password "any"
  Then authentication fails with "not found" or "password failed"

@auth-query
Scenario: Auth query — static user takes priority
  Given pg_doorman is configured with auth_query AND static user "static_user"
  When client connects as "static_user"
  Then static authentication is used

@auth-query
Scenario: Auth query MD5 — password rotation
  Given user "aq_user" password hash is cached in pg_doorman
  When "aq_user" password is changed in PostgreSQL via ALTER ROLE
  And client connects as "aq_user" with new password
  Then pg_doorman re-fetches hash and authentication succeeds
```

## Checklist

- [ ] `AUTH_QUERY_STATE` global with `ArcSwap`
- [ ] Executor creation in `ConnectionPool::from_config()`
- [ ] Shared pool creation for `server_user` mode
- [ ] `try_auth_query()` in auth flow
- [ ] MD5 auth with cache + re-fetch on failure
- [ ] Server parameters from shared pool
- [ ] BDD tests (5+)
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
- [ ] All existing tests pass
