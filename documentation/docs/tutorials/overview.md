---
title: Overview
---

# Overview

PgDoorman is a PostgreSQL connection pooler. Any application can consider connection to PgDoorman as if it were a 
connection to Postgresql server. PgDoorman will create a connection to the actual server or will reuse an existed connection.

In order not to compromise transaction semantics for connection  pooling, PgDoorman supports several types of pooling when rotating connections.

## Session pooling
Client gets an assigned server connection for the lifetime of the client connection. After the client disconnects, server connection will be released back into the pool.

## Transaction pooling
Client gets an assigned server connection only for the duration of transaction. After PgDoorman notices the end of the transaction, connection will be released back into the pool.

## Managing

You can manage PgDoorman via [Admin Console](./basic-usage.md#admin-console)