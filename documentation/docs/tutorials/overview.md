---
title: Overview
---

# PgDoorman Overview

## What is PgDoorman?

PgDoorman is a high-performance PostgreSQL connection pooler based on PgCat. It acts as a middleware between your applications and PostgreSQL servers, efficiently managing database connections to improve performance and resource utilization.

When an application connects to PgDoorman, it behaves exactly like a PostgreSQL server. Behind the scenes, PgDoorman either creates a new connection to the actual PostgreSQL server or reuses an existing connection from its pool, significantly reducing connection overhead.

## Key Benefits

- **Reduced Connection Overhead**: Minimizes the performance impact of establishing new database connections
- **Resource Optimization**: Limits the number of connections to your PostgreSQL server
- **Improved Scalability**: Allows more client applications to connect to your database
- **Connection Management**: Provides tools to monitor and manage database connections

## Pooling Modes

To maintain proper transaction semantics while providing efficient connection pooling, PgDoorman supports multiple pooling modes:

### Session Pooling

In session pooling mode:

- Each client is assigned a dedicated server connection for the entire duration of the client connection
- The server connection remains exclusively allocated to that client until disconnection
- After the client disconnects, the server connection is released back into the pool for reuse
- This mode is ideal for applications that rely on session-level features like temporary tables or session variables

### Transaction Pooling

In transaction pooling mode:

- A client is assigned a server connection only for the duration of a transaction
- Once PgDoorman detects the end of a transaction, the server connection is immediately released back into the pool
- This mode allows for higher connection efficiency as connections are shared between clients
- Ideal for applications with many short-lived connections or those that don't rely on session state

## Administration

PgDoorman provides comprehensive tools for monitoring and management:

- **Admin Console**: A PostgreSQL-compatible interface for viewing statistics and managing the pooler
- **Configuration Options**: Extensive settings to customize behavior for your specific needs
- **Monitoring**: Detailed metrics about connection usage and performance

For detailed information on managing PgDoorman, see the [Admin Console documentation](./basic-usage.md#admin-console).