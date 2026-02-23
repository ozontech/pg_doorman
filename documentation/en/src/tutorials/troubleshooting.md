# Troubleshooting

This guide helps you resolve common issues when using PgDoorman.

## Authentication Errors When Connecting to PostgreSQL

**Symptom:** PgDoorman starts successfully but clients get authentication errors like `password authentication failed` when trying to execute queries.

### If the pool username matches the backend PostgreSQL user

PgDoorman uses **passthrough authentication** by default — the client's cryptographic proof (MD5 hash or SCRAM ClientKey) is reused to authenticate to PostgreSQL. Make sure the `password` field in your config contains the exact hash from `pg_authid` / `pg_shadow`:

```bash
SELECT usename, passwd FROM pg_shadow WHERE usename = 'your_user';
```

Copy the hash (e.g., `md5...` or `SCRAM-SHA-256$...`) into your config's `password` field. The hash **must match** the one stored in PostgreSQL (same salt and iterations for SCRAM).

### If the pool username differs from the backend user

When the client-facing `username` in PgDoorman is different from the actual PostgreSQL role, passthrough cannot work — you need explicit credentials:

```yaml
users:
  - username: "app_user"              # client-facing name
    password: "md5..."                # hash for client authentication
    server_username: "pg_app_user"    # actual PostgreSQL role
    server_password: "plaintext_pwd"  # plaintext password for that role
    pool_size: 40
```

This also applies to JWT authentication where there is no password to pass through.

```admonish tip title="How to get the password hash"
You can get user password hashes from PostgreSQL using: `SELECT usename, passwd FROM pg_shadow;`

Or use the `pg_doorman generate` command which automatically retrieves them.
```

## Configuration File Not Found

**Symptom:** PgDoorman exits with "configuration file not found" error.

**Solution:** Specify the configuration file path explicitly:

```bash
pg_doorman /path/to/pg_doorman.yaml
```

By default, PgDoorman looks for `pg_doorman.toml` in the current directory.

## Pool Size Too Small

**Symptom:** Clients experience high wait times or receive errors about too many connections.

**Solution:** Increase `pool_size` for the affected user, or check the `SHOW POOLS` admin command to see `cl_waiting` and `maxwait` values. If `maxwait` is consistently high, your pool is undersized for your workload.

---

```admonish tip title="Still having issues?"
If you encounter a problem not listed here, please [open an issue on GitHub](https://github.com/ozontech/pg_doorman/issues).
```
