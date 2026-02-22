# Step 7: HBA integration

## Goal

Implement two-phase HBA check for auth_query users. Reject early when possible,
validate auth method after fetching password type from PG.

## Dependencies

- Step 4 (auth flow integration)
- Independent of Steps 5-6 (can be done in parallel)

## 7.1 Two-phase HBA check

### File: `src/auth/mod.rs`

Currently `eval_hba_for_pool_password()` (line 132) requires the pool password
to determine HBA outcome. For auth_query, we don't know the password type until
after fetching from PG.

#### Phase 1: Pre-check (before auth_query)

Add to `try_auth_query()`, before `cache.get_or_fetch()`:

```rust
/// HBA pre-check for auth_query users.
/// Rejects early if BOTH md5 and scram are denied by HBA.
/// Returns trust_mode flag if any HBA rule trusts this client.
fn hba_precheck(ci: &ClientIdentifier) -> Result<bool, Error> {
    let md5_denied = ci.hba_md5 == CheckResult::Deny || ci.hba_md5 == CheckResult::NotMatched;
    let scram_denied = ci.hba_scram == CheckResult::Deny || ci.hba_scram == CheckResult::NotMatched;

    if md5_denied && scram_denied {
        return Err(Error::HbaForbiddenError(format!(
            "HBA denies both MD5 and SCRAM for client from {:?}",
            ci.addr
        )));
    }

    let trust_mode = ci.hba_md5 == CheckResult::Trust || ci.hba_scram == CheckResult::Trust;
    Ok(trust_mode)
}
```

Usage in `try_auth_query()`:

```rust
// Phase 1: HBA pre-check
let trust_mode = match hba_precheck(client_identifier) {
    Ok(trust) => trust,
    Err(err) => {
        error_response_terminal(write, "Connection not permitted by HBA", "28000").await?;
        return Err(err);
    }
};

// ... fetch from cache/PG ...

// Phase 2: HBA post-check (now we know password type)
if !trust_mode {
    hba_postcheck(client_identifier, &cache_entry.password_hash)?;
}

// Phase: authenticate
if trust_mode {
    // Skip password challenge — but user existence verified by auth_query
} else if pool_password.starts_with(SCRAM_SHA_256) {
    // SCRAM auth ...
} else if pool_password.starts_with(MD5_PASSWORD_PREFIX) {
    // MD5 auth ...
}
```

#### Phase 2: Post-check (after auth_query returns password type)

```rust
/// HBA post-check for auth_query users.
/// Now we know the password type — check if this specific method is allowed.
fn hba_postcheck(ci: &ClientIdentifier, password_hash: &str) -> Result<(), Error> {
    if password_hash.starts_with(SCRAM_SHA_256) {
        if ci.hba_scram == CheckResult::Deny || ci.hba_scram == CheckResult::NotMatched {
            return Err(Error::HbaForbiddenError(format!(
                "SCRAM authentication not permitted by HBA for client from {:?}",
                ci.addr
            )));
        }
    } else if password_hash.starts_with(MD5_PASSWORD_PREFIX) {
        if ci.hba_md5 == CheckResult::Deny || ci.hba_md5 == CheckResult::NotMatched {
            return Err(Error::HbaForbiddenError(format!(
                "MD5 authentication not permitted by HBA for client from {:?}",
                ci.addr
            )));
        }
    }
    Ok(())
}
```

## 7.2 Trust mode for dynamic users

When HBA says `trust`:
1. Execute auth_query to verify user EXISTS in PG (don't skip this!)
2. Cache the password type for future use
3. Skip password challenge-response
4. Return pool to caller

```rust
if trust_mode {
    // User existence already verified by get_or_fetch() above
    // No password challenge needed
    info!("HBA trust for auth_query user '{username}' from {:?}", client_identifier.addr);
}
```

## 7.3 Talos integration

If `client_identifier.is_talos` is true, the client is already authenticated
upstream. Treat like trust — skip password challenge but still verify user
exists via auth_query.

```rust
if client_identifier.is_talos || trust_mode {
    // Already authenticated, just verify user exists
}
```

## 7.4 BDD tests

```gherkin
@auth-query @hba
Scenario: HBA denies both — no auth_query executed
  Given auth_query configured for pool "mydb"
  And pg_hba denies both md5 and scram for client IP
  When client connects as "alice"
  Then rejected immediately without auth_query call

@auth-query @hba
Scenario: HBA trust — user exists
  Given auth_query configured, HBA trusts client IP
  And "alice" exists in pg_shadow
  When client connects as "alice" without password
  Then auth_query verifies "alice" exists
  And authentication succeeds without password challenge

@auth-query @hba
Scenario: HBA trust — user does not exist
  Given auth_query configured, HBA trusts client IP
  When client connects as "nonexistent" without password
  Then auth_query finds no rows
  And authentication fails

@auth-query @hba
Scenario: HBA allows MD5, denies SCRAM — user has SCRAM password
  Given HBA allows md5 but denies scram
  And "alice" has SCRAM password in pg_shadow
  When client connects as "alice"
  Then Phase 2 check rejects: "auth method not allowed by HBA"
```

## Checklist

- [ ] `hba_precheck()` — reject early if both methods denied
- [ ] `hba_postcheck()` — validate specific method after password type known
- [ ] Trust mode: verify user exists, skip password challenge
- [ ] Talos integration
- [ ] BDD tests (4)
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
