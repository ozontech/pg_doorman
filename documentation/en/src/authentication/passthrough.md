# Passthrough Authentication (Default)

PgDoorman reuses the client's cryptographic proof — MD5 hash or SCRAM `ClientKey` — to authenticate to PostgreSQL. The plaintext password never leaves the client and is never stored in the pool config.

This is the recommended setup when the pool username matches the PostgreSQL user.

## How it works

### MD5

PostgreSQL's MD5 password protocol stores `md5(password + username)` server-side. The client also hashes the password the same way and sends `md5(stored_hash + salt)`. PgDoorman:

1. Receives the client's hashed response.
2. Looks up the stored MD5 hash in its config (or via `auth_query`).
3. Verifies the client response matches.
4. Forwards the **stored hash** to PostgreSQL as the password during backend authentication. PostgreSQL accepts it because the hash is what `pg_authid` actually stores.

The `password` field in the pool config holds the stored hash, formatted as `md5XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX` (the 32-character MD5 of `password + username`, prefixed with literal `md5`).

### SCRAM-SHA-256

SCRAM verifies the client without sending any password-equivalent material. PgDoorman:

1. Performs SCRAM handshake with the client, validating the `ClientProof`.
2. Extracts the `ClientKey` from a successful exchange.
3. Performs SCRAM handshake with PostgreSQL, replaying the same `ClientKey` to compute a fresh `ClientProof` for the backend nonce.

The `password` field in the pool config holds the SCRAM verifier from `pg_authid.rolpassword`, formatted as `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>`.

PgDoorman does not support SCRAM channel binding (`scram-sha-256-plus`).

## Configuration

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "app"
        password: "md5d41d8cd98f00b204e9800998ecf8427e"
        pool_size: 40
```

Note what is **not** there: no `server_username`, no `server_password`. PgDoorman infers passthrough from the absence of these fields.

For SCRAM, the password field looks like:

```yaml
password: "SCRAM-SHA-256$4096:random_salt$stored_key:server_key"
```

## Getting the hash

Connect as a superuser to PostgreSQL and read `pg_shadow` (or `pg_authid`):

```sql
SELECT usename, passwd FROM pg_shadow WHERE usename = 'app';
```

The `passwd` column contains either an MD5 hash (`md5...`) or a SCRAM verifier (`SCRAM-SHA-256$...`), depending on `password_encryption` setting at the time the password was set.

To force MD5 storage: `SET password_encryption = 'md5'; ALTER ROLE app PASSWORD 'plaintext';`
To force SCRAM: `SET password_encryption = 'scram-sha-256'; ALTER ROLE app PASSWORD 'plaintext';`

## When passthrough is not enough

Set `server_username` and `server_password` explicitly when:

- The pool user differs from the backend user (username remapping).
- The client authenticates with [JWT](jwt.md) — there is no MD5 hash or SCRAM key to pass through.
- The client authenticates with [Talos](talos.md) and you want a fixed backend identity per role.
- You use [auth_query](auth-query.md) in dedicated mode.

```yaml
users:
  - username: "external_app"
    password: "jwt-pkey-fpath:/etc/pg_doorman/jwt.pub"
    server_username: "app"
    server_password: "md5..."
    pool_size: 40
```

## Auto-generated config

`pg_doorman generate --host your-pg-host --user your-admin-user` introspects PostgreSQL and produces a config with hashes from `pg_shadow` filled in automatically. Use this for new deployments to avoid copy-paste mistakes.

```bash
pg_doorman generate --host db.example.com --user postgres --output pg_doorman.yaml
```

See [Basic Usage](../tutorials/basic-usage.md) for the full generate flow.
