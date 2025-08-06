---
title: General Settings
---

# Settings

## General Settings

### host

Listen host (TCP v4 only).

Default: `"0.0.0.0"`.

### port

Listen port for incoming connections.

Default: `6432`.

### backlog

TCP backlog for incoming connections. A value of zero sets the `max_connections` as value for the TCP backlog.

Default: `0`.

### max_connections

The maximum number of clients that can connect to the pooler simultaneously. When this limit is reached:
* A client connecting without SSL will receive the expected error (code: `53300`, message: `sorry, too many clients already`).
* A client connecting via SSL will see a message indicating that the server does not support the SSL protocol.

Default: `8192`.

### tls_mode

The TLS mode for incoming connections. It can be one of the following:

* `allow` - TLS connections are allowed but not required. The pg_doorman will attempt to establish a TLS connection if the client requests it.
* `disable` - TLS connections are not allowed. All connections will be established without TLS encryption.
* `require` - TLS connections are required. The pg_doorman will only accept connections that use TLS encryption.
* `verify-full` - TLS connections are required and the pg_doorman will verify the client certificate. This mode provides the highest level of security.

Default: `"allow"`.

### tls_ca_file

The file containing the CA certificate to verify the client certificate. This is required when `tls_mode` is set to `verify-full`.

Default: `None`.

### tls_private_key

The path to the private key file for TLS connections. This is required to enable TLS for incoming client connections. Must be used together with `tls_certificate`.

Default: `None`.

### tls_certificate

The path to the certificate file for TLS connections. This is required to enable TLS for incoming client connections. Must be used together with `tls_private_key`.

Default: `None`.

### tls_rate_limit_per_second

Limit the number of simultaneous attempts to create a TLS session.
Any value other than zero implies that there is a queue through which clients must pass in order to establish a TLS connection.
In some cases, this is necessary in order to launch an application that opens many connections at startup (the so-called "hot start").

Default: `0`.

### daemon_pid_file

Enabling this setting enables daemon mode. Comment this out if you want to run pg_doorman in the foreground with `-d`.

Default: `None`.

### syslog_prog_name

When specified, pg_doorman starts sending messages to syslog (using /dev/log or /var/run/syslog).
Comment this out if you want to log to stdout.

Default: `None`.

### log_client_connections 

Log client connections for monitoring.

Default: `true`.

### log_client_disconnections 

Log client disconnections for monitoring.

Default: `true`.

### worker_threads

The number of worker processes (posix threads) that async serve clients, which affects the performance of pg_doorman.
The more workers there are, the faster the system works, but only up to a certain limit (cpu count).
If you already have a lot of workers, you should consider increasing the number of virtual pools.

Default: `4`.

### worker_cpu_affinity_pinning

Automatically assign workers to different CPUs (man 3 cpu_set).

Default: `false`.

### virtual_pool_count

Increasing the number of virtual pools can help deal with internal latches that occur when processing very large numbers of fast queries.
It is strongly recommended not to change this parameter if you do not understand what you are doing.

Default: `1`.

### tokio_global_queue_interval 

[Tokio runtime settings](https://docs.rs/tokio/latest/tokio/).
It is strongly recommended not to change this parameter if you do not understand what you are doing.

Default: `5`.

### tokio_event_interval

[Tokio runtime settings](https://docs.rs/tokio/latest/tokio/).
It is strongly recommended not to change this parameter if you do not understand what you are doing.

Default: `1`.

### worker_stack_size

[Tokio runtime settings](https://docs.rs/tokio/latest/tokio/).
It is strongly recommended not to change this parameter if you do not understand what you are doing.

Default: `8388608`.


### connect_timeout

Connection timeout to server in milliseconds.

Default: `3000` (3 sec).

### query_wait_timeout

Maximum time to wait for a query to complete, in milliseconds.

Default: `5000` (5 sec).

### idle_timeout

Server idle timeout in milliseconds.

Default: `300000000` (5000 min).

### server_lifetime

Server lifetime in milliseconds.

Default: `300000` (5 min).

### server_round_robin

In transactional pool mode, we can choose whether the last free server backend will be used or the next one will be selected.
By default, the LRU (Least Recently Used) method is used, which has a positive impact on performance.

Default: `false`.

### sync_server_parameters

If enabled, we strive to restore the parameters (via query `SET`) that were set by the client (and application_name)
in transaction mode in other server backends. By default, this is disabled (false) due to performance.
If you need to know `application_name`, but don't want to experience performance issues due to constant server queries `SET`,
you can consider creating a separate pool for each application and using the `application_name` parameter in the `pool` settings.

Default: `false`.

### tcp_so_linger

By default, pg_doorman send `RST` instead of keeping the connection open for a long time.

Default: `0`.

### tcp_no_delay

TCP_NODELAY to disable Nagle's algorithm for lower latency.

Default: `true`.

### tcp_keepalives_count

Keepalive enabled by default and overwrite OS defaults.

Default: `5`.

### tcp_keepalives_idle

Default: `5`.

### tcp_keepalives_interval

Default: `1`.

### unix_socket_buffer_size

Buffer size for read and write operations when connecting to PostgreSQL via a unix socket.

Default: `1048576`.

### admin_username

Access to the virtual admin database is carried out through the administrator's username and password.

Default: `"admin"`.

### admin_password

Access to the virtual admin database is carried out through the administrator's username and password.
It should be replaced with your secret.

Default: `"admin"`.

### prepared_statements

Switcher to enable/disable caching of prepared statements.

Default: `true`.

### prepared_statements_cache_size

Cache size of prepared requests on the server side.

Default: `8192`.

### message_size_to_be_stream

Data responses from the server (message type 'D') greater than this value will be
transmitted through the proxy in small chunks (1 MB).

Default: `1048576`.

### max_memory_usage

We calculate the total amount of memory used by the internal buffers for all current queries.
If the limit is reached, the client will receive an error (256 MB).

Default: `268435456`.

### shutdown_timeout

With a graceful shutdown, we wait for transactions to be completed within this time limit (10 seconds).

Default: `10000`.

### proxy_copy_data_timeout

Maximum time to wait for data copy operations during proxying, in milliseconds.

Default: `15000` (15 sec).


### server_tls

Enable TLS for connections to the PostgreSQL server. When enabled, pg_doorman will attempt to establish TLS connections to the backend PostgreSQL servers.

Default: `false`.

### verify_server_certificate

Verify the PostgreSQL server's TLS certificate when connecting with TLS. This setting is only relevant when `server_tls` is enabled.

Default: `false`.

### hba

The list of IP addresses from which it is permitted to connect to the pg-doorman.

### pooler_check_query

This query will not be sent to the server if it is run as a SimpleQuery.
It can be used to check the connection at the application level.

Default: `;`.
