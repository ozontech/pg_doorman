# Password-Free Backend Auth for Static Users

## Problem

Static users connecting to PostgreSQL backend as themselves require a **plaintext** `server_password` in the config:

```toml
[[pools.mydb.users]]
username = "alice"
password = "SCRAM-SHA-256$4096:salt$StoredKey:ServerKey"  # secure hash
server_username = "alice"
server_password = "alice_plaintext_password"               # plaintext!
```

## Solution

Extend the auth_query passthrough mechanism (already implemented for dynamic users) to static users, eliminating `server_password` for the common case where client and backend identities match.

**Target config (no plaintext passwords):**
```toml
[[pools.mydb.users]]
username = "alice"
password = "SCRAM-SHA-256$4096:salt$StoredKey:ServerKey"
# No server_username/server_password needed!
# pg_doorman authenticates to backend using passthrough
```

## When Passthrough Activates (auto-detection)

A static user is "passthrough-eligible" when:
1. `server_password` is NOT set, AND
2. `server_username` is None OR equals `username` (identity match)

When `server_password` IS set, existing behavior is preserved.

## MD5 Path

- Config: `password = "md5abc..."` (same format as pg_shadow)
- At pool creation: set `Address.backend_auth = Md5PassTheHash(password)`
- Backend MD5 auth: existing passthrough code handles it
- No timing issues: hash available immediately from config

## SCRAM Path (lazy ClientKey)

- Config: `password = "SCRAM-SHA-256$iter:salt$StoredKey:ServerKey"`
- StoredKey = H(ClientKey): one-way, cannot recover ClientKey from verifier
- ClientKey is extracted from client's SCRAM proof during auth
- Previously discarded for static users (`_client_key`)
- Solution: save ClientKey on pool's Address after first client SCRAM auth

### Lifecycle

```
Config load -> Pool created -> backend_auth = ScramPending
First client SCRAM auth -> ClientKey extracted -> backend_auth = ScramPassthrough(key)
Backend connection created -> uses ClientKey (skip PBKDF2)
```

No race condition: pools grow on demand, first backend connection is after first client auth.

## SCRAM Salt Matching Constraint

The SCRAM verifier in config **must match** `pg_authid` on the backend (same salt/iterations). If they differ, ClientKey won't work. This is the same constraint as auth_query passthrough.

**Practical implication**: copy the verifier from `SELECT rolpassword FROM pg_authid WHERE rolname = 'alice'` into the config, OR use the same `ALTER USER ... PASSWORD` on both sides.

## When Passthrough Doesn't Work

| Scenario | Why | Solution |
|----------|-----|----------|
| `server_username != username` | MD5 hash includes username; SCRAM salt per-user | Use `server_password` |
| Client MD5, Backend SCRAM | MD5 auth doesn't produce ClientKey | Use `server_password` |
| Client SCRAM, Backend MD5 | Can't derive MD5 hash from SCRAM verifier | Use `server_password` |
| Different SCRAM salts | ClientKey won't match | Sync verifier from pg_authid |

## Implementation

### BackendAuthMethod::ScramPending

New enum variant (no payload) representing "SCRAM passthrough configured but ClientKey not yet available".

### Config Validation Relaxation

- Old: both `server_username` and `server_password` must be set together
- New: only reject `server_password` without `server_username`
- Allow: `server_username` alone (trust/passthrough) and both-None (passthrough)

### Pool Creation

When passthrough-eligible:
- MD5 password: `backend_auth = Md5PassTheHash(password)`
- SCRAM password: `backend_auth = ScramPending`

### SCRAM Auth Flow

1. `authenticate_with_scram()` returns `Option<Vec<u8>>` (ClientKey)
2. After SCRAM auth, if pool has `ScramPending`, update to `ScramPassthrough(client_key)` via `Arc<RwLock<>>`

### Backend Connection

- `ScramPending`: fall through to `server_password` if available, else return error (backend connection before any client auth)
- `ScramPassthrough(key)`: existing passthrough code handles it
- `Md5PassTheHash(hash)`: existing passthrough code handles it
