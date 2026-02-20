---
title: Pool Settings
---

## Pool Settings

Each record in the pool is the name of the virtual database that the pg-doorman client can connect to.

```toml
[pools.exampledb] # Declaring the 'exampledb' database
```

### server_host 

The directory with unix sockets or the IPv4 address of the PostgreSQL server that serves this pool.

Example: `"/var/run/postgresql"` or `"127.0.0.1"`.

### server_port

The port through which PostgreSQL server accepts incoming connections.

Default: `5432`.

### server_database 

Optional parameter that determines which database should be connected to on the PostgreSQL server.

Example: `"exampledb-2"`

### application_name

Parameter application_name, is sent to the server when opening a connection with PostgreSQL. It may be useful with the sync_server_parameters = false setting.

Example: `"exampledb-pool"`

### connect_timeout

Maximum time to allow for establishing a new server connection for this pool, in milliseconds. If not specified, the global connect_timeout setting is used.

Default: `None` (uses global setting).

### idle_timeout

Close idle connections in this pool that have been opened for longer than this value, in milliseconds. If not specified, the global idle_timeout setting is used.

Default: `None` (uses global setting).

### server_lifetime

Close server connections in this pool that have been opened for longer than this value, in milliseconds. Only applied to idle connections. If not specified, the global server_lifetime setting is used.

Default: `None` (uses global setting).

### pool_mode

* `session`
:   Server is released back to pool after client disconnects.

* `transaction`
:   Server is released back to pool after transaction finishes.

Example: `"session"` or `"transaction"`.

### log_client_parameter_status_changes

Log information about any SET command in the log.

Default: `false`.

### cleanup_server_connections

When enabled, the pool will automatically clean up server connections that are no longer needed. This helps manage resources efficiently by closing idle connections.

Default: `true`.

## Pool Users Settings

```toml
[pools.exampledb.users.0]
username = "exampledb-user-0" # A virtual user who can connect to this virtual database.
```
### username

A virtual username who can connect to this virtual database (pool).

Example: `"exampledb-user-0"`.

### password

The password for the virtual pool user.
Password can be specified in `MD5`, `SCRAM-SHA-256`, or `JWT` format.
Also, you can create a mirror list of users using secrets from the PostgreSQL instance: `select usename, passwd from pg_shadow`.

Example: `md5dd9a0f2...76a09bbfad` or `SCRAM-SHA-256$4096:E+QNCSW3r58yM+Twj1P5Uw==$LQrKl...Ro1iBKM=` or in jwt format: `jwt-pkey-fpath:/etc/pg_doorman/jwt/public-exampledb-user.pem`

### auth_pam_service

The pam-service that is responsible for client authorization. In this case, pg_doorman will ignore the `password` value.

### server_username

The real PostgreSQL username used to connect to the database server.

By default, PgDoorman uses the same `username` for both client authentication and server connections. However, if the client `password` is an MD5 or SCRAM hash (which is the typical setup), PostgreSQL will **reject the connection** because it expects a plaintext password, not a hash.

To fix this, set `server_username` and `server_password` to the actual PostgreSQL credentials. Both must be specified together.

Example: `"exampledb_server_user"`.

### server_password

The plaintext password for the PostgreSQL server user specified in `server_username`.

This is needed because PgDoorman stores client passwords as MD5/SCRAM hashes for client authentication, but PostgreSQL requires a plaintext password during server-side authentication.

Example: `"real_password_here"`.

!!! warning "Common Setup Issue"
    If you see authentication errors when PgDoorman tries to connect to PostgreSQL, the most likely cause is that `server_username` and `server_password` are not set. Without these, PgDoorman tries to authenticate to PostgreSQL using the MD5/SCRAM hash from the `password` field, which PostgreSQL rejects.

    **Solution:** Set both `server_username` and `server_password` to the actual PostgreSQL credentials:

    ```yaml
    users:
      - username: "app_user"
        password: "md5..."                # hash for client authentication
        server_username: "app_user"       # real PostgreSQL username
        server_password: "plaintext_pwd"  # real PostgreSQL password
    ```

### pool_size

The maximum number of simultaneous connections to the PostgreSQL server available for this pool and user.

Default: `40`.

### min_pool_size

The minimum number of connections to maintain in the pool for this user. This helps with performance by keeping connections ready. If specified, it must be less than or equal to pool_size.

Default: `None`.

### server_lifetime

Close server connections for this user that have been opened for longer than this value, in milliseconds. Only applied to idle connections. If not specified, the pool's server_lifetime setting is used.

Default: `None` (uses pool setting).
