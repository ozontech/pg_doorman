# Authentication

PgDoorman authenticates clients before forwarding them to PostgreSQL. It supports six methods, dispatched in priority order based on what the client sends and what the pool config defines.

This page explains how PgDoorman picks an authentication method. For setup details, follow the per-method links below.

## Methods at a glance

| Method | When to use | Stores secret in config? |
| --- | --- | --- |
| [Passthrough](passthrough.md) (MD5 / SCRAM) | Default. Pool user matches PostgreSQL user. | MD5 hash or SCRAM ClientKey, never plaintext |
| [auth_query](auth-query.md) | Many users, dynamic onboarding. Lookup credentials from PostgreSQL itself. | One service-user secret only |
| [PAM](pam.md) | OS-level authentication (LDAP via `pam_ldap`, Kerberos, local accounts). Linux only. | No |
| [JWT](jwt.md) | Service-to-database with short-lived tokens signed by an external IdP. | Public key only |
| [Talos](talos.md) | JWT with role extraction baked in. Used at Ozon. | Public key only |
| [pg_hba.conf](hba.md) | Restrict who can connect from where (network ACL), independent of credential method. | No |

LDAP, Kerberos GSSAPI, certificate-based auth, and SCRAM channel binding (`scram-sha-256-plus`) are not supported. See [Comparison](../comparison.md#authentication).

## Dispatch order

`pg_hba.conf` is evaluated first, before any credential check. A `reject` rule terminates the connection; a `trust` rule skips the credential check entirely.

After HBA, PgDoorman picks a credential method in this order:

1. **Talos.** Activated when the client connects with username `talos`. The client's password is parsed as a JWT, the role (`owner` / `read_write` / `read_only`) is extracted, and the connection continues under that derived identity.
2. **HBA Trust.** If `pg_hba.conf` matched a `trust` rule, no credential check happens.
3. **PAM.** If the matched user has `auth_pam_service` set, credentials go to PAM (Linux only). PAM wins over a static password.
4. **SCRAM static.** If the user's `password` in config starts with `SCRAM-SHA-256$`, PgDoorman runs SCRAM authentication.
5. **MD5 static.** If the user's `password` starts with `md5`, PgDoorman runs MD5 authentication.
6. **JWT.** If the user's `password` starts with `jwt-pkey-fpath:`, the client's password is verified as a JWT against the public key on disk.

`auth_query` is not in this dispatch list — it runs **before** the dispatch to populate the pool's user list with hashes pulled from PostgreSQL. After `auth_query` returns a `passwd` value, dispatch picks the right method based on that value's prefix (`SCRAM-SHA-256$` or `md5`).

If none of the methods matches the password format, PgDoorman returns "Authentication method not supported" and closes the connection.

## Talking to PostgreSQL: passthrough vs configured

PgDoorman has to authenticate twice: once as the gateway (client → PgDoorman) and once as the backend (PgDoorman → PostgreSQL). Three patterns:

- **Passthrough (default).** The client's MD5 hash or SCRAM `ClientKey` is reused to authenticate to PostgreSQL. No plaintext password in config. Requires `server_username` to be unset (or equal to the client username).
- **Configured backend user.** Set `server_username` and `server_password` in the user block. PgDoorman authenticates to PostgreSQL with these instead. Use this when the pool username is decoupled from the database user (Talos, JWT, name remapping).
- **auth_query in dedicated mode.** Set `server_user` inside the `auth_query` block. All dynamically-discovered users share one backend pool authenticated as `server_user`. Trades per-user backend identity for pool reuse efficiency.

See [Passthrough](passthrough.md) for details and [auth_query](auth-query.md) for dedicated mode.

## Restricting connections

`pg_hba.conf` is enforced before credentials are checked. Common patterns:

- Reject everything except localhost: `host all all 0.0.0.0/0 reject` followed by `host all all 127.0.0.1/32 trust`.
- Require TLS for non-local connections: `hostssl all all 0.0.0.0/0 scram-sha-256` and `hostnossl all all 127.0.0.1/32 trust`.
- Per-database ACL: `host mydb appuser 10.0.0.0/8 scram-sha-256`.

See [pg_hba.conf](hba.md).

## Where to next

- New deployment? Read [Passthrough](passthrough.md) and [Basic usage](../tutorials/basic-usage.md).
- Many users with rotating credentials? Use [auth_query](auth-query.md).
- Token-based service identity? Use [JWT](jwt.md).
- OS-integrated authentication? Use [PAM](pam.md).
- Network-level restrictions? Configure [pg_hba.conf](hba.md).
