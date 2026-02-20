# Troubleshooting

This guide helps you resolve common issues when using PgDoorman.

## Authentication Errors When Connecting to PostgreSQL

**Symptom:** PgDoorman starts successfully but clients get authentication errors like `password authentication failed` when trying to execute queries.

**Cause:** By default, PgDoorman uses the same `username` and `password` for both client authentication and connecting to the PostgreSQL server. If the `password` field contains an MD5 or SCRAM hash (which is the typical and recommended setup), PostgreSQL will reject it because it expects a plaintext password.

**Solution:** Set `server_username` and `server_password` in your user configuration to the actual PostgreSQL credentials:

=== "YAML"

    ```yaml
    pools:
      mydb:
        server_host: "127.0.0.1"
        server_port: 5432
        users:
          - username: "app_user"
            password: "md5..."                # MD5/SCRAM hash for client auth
            server_username: "app_user"       # real PostgreSQL username
            server_password: "plaintext_pwd"  # real PostgreSQL password
            pool_size: 40
    ```

=== "TOML"

    ```toml
    [pools.mydb.users.0]
    username = "app_user"
    password = "md5..."                # MD5/SCRAM hash for client auth
    server_username = "app_user"       # real PostgreSQL username
    server_password = "plaintext_pwd"  # real PostgreSQL password
    pool_size = 40
    ```

!!! tip "How to get the password hash"
    You can get user password hashes from PostgreSQL using: `SELECT usename, passwd FROM pg_shadow;`

    Or use the `pg_doorman generate` command which automatically retrieves them.

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

!!! tip "Still having issues?"
    If you encounter a problem not listed here, please [open an issue on GitHub](https://github.com/ozontech/pg_doorman/issues).
