# Fastpath and Large Objects

PostgreSQL has a Fastpath FunctionCall message in the frontend/backend
protocol. A client sends `F`, PostgreSQL returns a `FunctionCallResponse`
message `V`, and the request is complete only after the following
`ReadyForQuery` message `Z`.

The pgjdbc `LargeObjectManager` uses this protocol path for operations such
as `lo_creat`, `lo_open`, `lo_write`, `lo_read`, and `lo_close`. Applications
often reach it indirectly through ORM mappings that store large values in OID
columns, including Hibernate configurations that unwrap pgjdbc's large object
API.

Before 3.10.7, pg_doorman did not forward frontend `FunctionCall` messages in
transaction pooling. pgjdbc clients could wait forever for the missing
`FunctionCallResponse`. From 3.10.7, pg_doorman forwards the `F` request,
passes the `V` response through, and uses the trailing `ReadyForQuery`
transaction status to decide when a transaction-pool backend can be released.

## Transaction Pooling

Large object descriptors live inside a PostgreSQL transaction. If a fastpath
call finishes with `ReadyForQuery` status `T` or `E`, pg_doorman keeps the same
backend assigned to the client. The backend is released only when PostgreSQL
later reports idle status `I`, typically after `COMMIT` or `ROLLBACK`.

Autocommit fastpath calls that finish with `I` release the backend immediately.

This matches PgBouncer's transaction-pooling behavior for PostgreSQL
FunctionCall traffic.

## Pool Sizing

Each in-flight large object call holds one backend until PostgreSQL returns
`ReadyForQuery`. For write-heavy or read-heavy large object workloads, size the
pool for the number of concurrent large object calls, not only for ordinary SQL
statement rate.

Watch these signals during rollout:

- `SHOW POOLS`: `cl_active`, `sv_active`, and waiting clients.
- Query wait timeout errors.
- Query latency percentiles for pools used by large object traffic.

If bursts of large object calls push clients close to `query_wait_timeout`,
increase pool capacity for that user/database or reduce application-side large
object concurrency.

## Read Size And Memory

pg_doorman streams large backend `DataRow` and `CopyData` messages when they
exceed `general.message_size_to_be_stream`. From 3.10.7 this also applies to
large `FunctionCallResponse` messages, so a large fastpath `lo_read` result is
forwarded without keeping the whole response in pg_doorman memory.

Keep application-side large object reads chunked anyway. Large single-message
reads still hold a backend for longer, keep socket buffers busy, and are bounded
by PostgreSQL protocol message limits. pgjdbc's common
`LargeObject.read(byte[])` usage already reads in small chunks.

Changing `general.message_size_to_be_stream` controls when pg_doorman switches
to byte-stream forwarding for `DataRow`, `CopyData`, and
`FunctionCallResponse` messages.

## Timeouts And Lifetime

`server_lifetime` is applied to idle pooled backends. It does not interrupt a
backend that is actively serving a large object read or write. The backend can
be closed after the call finishes and returns to the idle pool.

Large object descriptors also depend on PostgreSQL transaction state. If an
application leaves a large object transaction idle between fastpath calls,
PostgreSQL's `idle_in_transaction_session_timeout` can terminate the backend.
pg_doorman then returns a connection error to the client. Keep large object
transactions short, or tune PostgreSQL timeouts for sessions that perform large
object work.

## Patroni-Assisted Fallback

During a primary outage, Patroni-assisted fallback may temporarily connect to a
replica or a candidate that is still read-only. Large object writes sent there
fail with PostgreSQL read-only errors, for example SQLSTATE `25006`. This is a
loud PostgreSQL error, not silent data loss.

When large object traffic is critical during failover, monitor read-only errors
and pool wait time alongside the fallback logs.
