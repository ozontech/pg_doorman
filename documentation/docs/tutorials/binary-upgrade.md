---
title: How to upgrade
---

## Binary upgrade

When you send a `SIGINT` signal to the pg_doorman process, the binary upgrade begins.
The old pg_doorman instance executes the exec command and starts a new, daemonized process.
This new process uses the `SO_REUSE_PORT` parameter, and the operating system sends traffic to the new instance.
After that, the old instance closes its socket for incoming connections.

We then give the option to complete any current queries and transactions within the specified `shutdown_timeout` (10 seconds).
After successful completion of each query or transaction, we return an error code `58006` to the client, indicating that they need to reconnect.
After reconnecting, the client can safely retry their queries.

!!! warning

    Repeating query (without code `58006`) may cause problems as described in [issue](https://github.com/lib/pq/issues/939)

!!! tip

    Be careful when using `github.com/lib/pq` or `database/sql`.