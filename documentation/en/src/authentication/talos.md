# Talos Authentication

Talos is a JWT-based authentication scheme developed at Ozon. The token carries a role assignment per database in its `resource_access` claim, and PgDoorman extracts the highest role to pick the backend identity. Multiple signing keys are supported via the `kid` header.

If you operate inside Ozon's Talos identity stack, this is the integration. Outside, prefer plain [JWT](jwt.md).

## How it works

1. A client connects with username `talos` and a JWT as the password.
2. PgDoorman reads the `kid` field from the JWT header and looks up the matching public key in `general.talos.keys`.
3. The token is verified (RS256, `exp`, `nbf`).
4. PgDoorman walks `resource_access` keys, splits each on `:`, and matches the part **after the colon** against `general.talos.databases`. So a key like `"postgres.stg:billing"` matches the `billing` database. The roles from every matching entry are collected; the highest wins (`owner` > `read_write` > `read_only`).
5. The connection is authenticated against a pool user named after the role: `owner`, `read_write`, or `read_only`. That user must exist in the pool with `server_username` and `server_password` configured.

The client identity (`clientId` from the token) is preserved in `application_name` and audit logs.

## Configuration

```yaml
general:
  host: "0.0.0.0"
  port: 6432
  talos:
    keys:
      - "/etc/pg_doorman/talos/keys/abc123.pem"
      - "/etc/pg_doorman/talos/keys/def456.pem"
    databases:
      - "billing"
      - "inventory"

pools:
  billing:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "owner"
        server_username: "billing_owner"
        server_password: "md5..."
        pool_size: 20
      - username: "read_write"
        server_username: "billing_app"
        server_password: "md5..."
        pool_size: 40
      - username: "read_only"
        server_username: "billing_ro"
        server_password: "md5..."
        pool_size: 60
```

The file stem of each key (`abc123`, `def456`) is the `kid` matched against the JWT header.

`databases` is a filter: only listed databases are eligible for Talos. A token without an entry for the requested database is rejected.

## Token shape

```json
{
  "kid": "abc123",
  "alg": "RS256"
}
.
{
  "exp": 1714500000,
  "nbf": 1714400000,
  "clientId": "billing-service",
  "resource_access": {
    "postgres.stg:billing": { "roles": ["read_write"] },
    "postgres.stg:inventory": { "roles": ["read_only", "read_write"] }
  }
}
```

`resource_access` keys must include a colon. PgDoorman ignores everything before it and matches the suffix against `general.talos.databases`. A token built without the colon prefix will produce no role and authentication will fail with "Token may not contain valid roles for the requested databases".

A client connecting to `inventory` with this token lands in the `read_write` user (max of the two listed roles).

## Dispatch order

Talos has highest priority. If a client connects with username `talos` and `general.talos.keys` is non-empty, no other authentication method is tried.

See [Overview](overview.md#dispatch-order).

## Caveats

- Talos requires the special `talos` username. Non-Talos clients use other authentication methods normally.
- The role-to-user mapping is fixed: `owner`, `read_write`, `read_only`. Custom role names need code changes.
- Multiple roles in the same `resource_access` entry collapse to the maximum. There is no "deny" semantics.
- Public keys are loaded once at startup and reloaded on `SIGHUP`.
