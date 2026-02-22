# Binary Upgrade Process

## Overview

PgDoorman supports seamless binary upgrades that allow you to update the software with minimal disruption to your database connections. This document explains how the upgrade process works and what to expect during an upgrade.

## How the Upgrade Process Works

When you send a `SIGINT` signal to the PgDoorman process, the binary upgrade process is initiated:

1. The current PgDoorman instance executes the exec command and starts a new, daemonized process
2. The new process uses the `SO_REUSE_PORT` socket option, allowing the operating system to distribute incoming traffic to the new instance
3. The old instance then closes its socket for incoming connections
4. Existing connections are handled gracefully during the transition

## Handling Existing Connections

During the upgrade process, PgDoorman handles existing connections as follows:

1. Current queries and transactions are allowed to complete within the specified `shutdown_timeout` (default: 10 seconds)
2. After each query or transaction completes successfully, PgDoorman returns error code `58006` to the client
3. This error code indicates to the client that they need to reconnect to the server
4. After reconnecting, clients can safely retry their queries with the new PgDoorman instance

## Important Considerations

```admonish warning title="Query Repetition"
Repeating a query without receiving error code `58006` may cause problems as described in [this issue](https://github.com/lib/pq/issues/939). Make sure your client application properly handles reconnection scenarios.
```

```admonish tip title="Client Library Compatibility"
Be careful when using client libraries like `github.com/lib/pq` or Go's standard `database/sql` package. Ensure they properly handle the reconnection process during binary upgrades.
```
