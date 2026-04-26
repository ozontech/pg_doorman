# pg_hba.conf

Restrict who can connect to PgDoorman based on source address, database, user, and connection type. Uses the same rule format as PostgreSQL's `pg_hba.conf`.

This is a network-level access control layer that runs **before** credential authentication. A connection rejected by `pg_hba` never gets to the password check.

## Configuration

Three formats. Pick whichever fits your deployment.

### Inline string

```yaml
general:
  hba: |
    hostssl all all 0.0.0.0/0 scram-sha-256
    host    all all 127.0.0.1/32 trust
    local   all all              trust
    host    all all 0.0.0.0/0    reject
```

### From file

```yaml
general:
  hba:
    path: "/etc/pg_doorman/pg_hba.conf"
```

The file is read on startup and on `SIGHUP`.

### Inline content under structured key

```yaml
general:
  hba:
    content: |
      hostssl all all 0.0.0.0/0 scram-sha-256
      host    all all 127.0.0.1/32 trust
```

Same as the inline string, useful when you generate the config from templating tools.

## Rule format

Each line:

```
<connection_type> <database> <user> [<source_cidr>] <method>
```

**connection_type** — one of:

| Type | Matches |
| --- | --- |
| `host` | TCP, with or without TLS |
| `hostssl` | TCP only when TLS is active |
| `hostnossl` | TCP only when TLS is **not** active |
| `local` | Unix domain socket |

**database** — `all`, a specific database name, or a comma-separated list. `replication` is not handled (PgDoorman doesn't support replication passthrough).

**user** — `all`, a specific user, or a comma-separated list. `+groupname` (PostgreSQL role membership) is not supported.

**source_cidr** — IPv4 or IPv6 CIDR. Required for `host`, `hostssl`, `hostnossl`. Not applicable to `local`.

**method** — one of:

| Method | Behavior |
| --- | --- |
| `trust` | Skip credential check entirely. The client is admitted with the username it claimed. |
| `md5` | Force MD5 password authentication. |
| `scram-sha-256` | Force SCRAM-SHA-256 authentication. |
| `reject` | Refuse the connection before any credential check. |

Rules are evaluated top to bottom. The first match wins.

## Examples

### Require TLS from the network, allow plain local

```
hostssl   all all 10.0.0.0/8     scram-sha-256
hostnossl all all 10.0.0.0/8     reject
host      all all 127.0.0.1/32   trust
local     all all                trust
```

### Per-database ACL

```
host billing  app_billing  10.0.0.0/8 scram-sha-256
host billing  all          0.0.0.0/0  reject
host inventory app_inv     10.0.0.0/8 scram-sha-256
host all       admin       10.1.1.0/24 scram-sha-256
host all       all         0.0.0.0/0  reject
```

### Block legacy MD5 from the open internet

```
hostssl all all 0.0.0.0/0 scram-sha-256
host    all all 0.0.0.0/0 reject
```

If your database stores MD5 hashes only and a client requests SCRAM, authentication fails with a clear error. Switch the database to SCRAM-SHA-256 (`ALTER ROLE ... PASSWORD`) before tightening rules.

## Differences from PostgreSQL's `pg_hba.conf`

- No `replication` keyword (PgDoorman does not pass replication connections).
- No `peer`, `ident`, `cert`, `gss`, `sspi`, or `pam` methods. PAM is configured per-user with `auth_pam_service`, not via HBA.
- No `+groupname` user prefix.
- No regex (`/regex` syntax).
- IPv6 CIDR is supported. IPv4-mapped IPv6 (`::ffff:1.2.3.4`) is matched against IPv4 rules.

## Reload

```bash
kill -HUP $(pidof pg_doorman)
```

Existing connections are not re-evaluated. New connections use the new rules.

## Caveats

- Rules apply to clients connecting **to PgDoorman**, not to PostgreSQL. PostgreSQL's own `pg_hba.conf` still matters for the backend connection.
- `trust` admits the client without any credential check. The backend still has to authenticate as the pool user — but the client side is unverified. Use `trust` only on networks where the source address is trustworthy (loopback, restricted Unix socket).
- For LDAP, Kerberos, or `peer` authentication, see [Comparison](../comparison.md#authentication) — these are not supported.
