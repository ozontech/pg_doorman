# Fastpath and Large Objects

Use this page when pgjdbc or Hibernate works with PostgreSQL large objects
through a pg_doorman transaction pool.

pgjdbc `LargeObjectManager` uses PostgreSQL Fastpath `FunctionCall` (`F`) for
large object functions such as `lo_creat`, `lo_open`, `lo_read`, `lo_write`,
and `lo_close`. PostgreSQL replies with `FunctionCallResponse` (`V`) and then
`ReadyForQuery` (`Z`). The `V` message contains the function result; the
transaction status is in the following `ReadyForQuery`.

Before 3.10.7, pg_doorman did not forward `FunctionCall` in transaction
pooling. A client could send a large object call and then wait forever for a
response. Since 3.10.7, pg_doorman forwards the call, passes
`FunctionCallResponse` back to the client, and releases the backend only after
`ReadyForQuery` says the session is idle.

## Transaction Pooling

Large object descriptors live inside a PostgreSQL transaction. If
`ReadyForQuery` reports status `T` or `E` after a fastpath call, pg_doorman
keeps the same backend assigned to the client. The backend is released only
after PostgreSQL later reports idle status `I`, normally after `COMMIT` or
`ROLLBACK`.

Autocommit fastpath calls release the backend as soon as `ReadyForQuery`
reports idle.

This matches PgBouncer transaction-pooling behavior for `FunctionCall` traffic.

## Pool Sizing

Each active large object call holds one backend until PostgreSQL sends
`ReadyForQuery`. Size the pool for concurrent large object reads and writes,
not only for ordinary SQL statement rate.

Watch these signals after enabling this traffic:

- `SHOW POOLS`: active clients, active servers, and waiting clients.
- `query_wait_timeout` errors.
- Latency percentiles for pools used by large object traffic.

If large object bursts push clients close to `query_wait_timeout`, increase
pool capacity for that user/database or reduce application-side large object
concurrency.

## Large Reads

pg_doorman streams large `DataRow`, `CopyData`, and `FunctionCallResponse`
messages when they exceed `general.message_size_to_be_stream`. A large
fastpath `lo_read` response is forwarded without buffering the full response in
pg_doorman memory first.

Streaming limits pg_doorman heap use; it does not make large single reads free.
A large read still holds a backend and socket buffers while PostgreSQL sends
the response, and PostgreSQL protocol message limits still apply. Keep
application-side large object reads chunked.

## Timeouts

`server_lifetime` applies to idle pooled backends. It does not interrupt a
backend that is serving a large object read or write.

Large object descriptors also depend on PostgreSQL transaction state. If an
application leaves a large object transaction idle between fastpath calls,
PostgreSQL `idle_in_transaction_session_timeout` can terminate the backend.
pg_doorman then returns a connection error to the client. Keep large object
transactions short, or tune PostgreSQL timeouts for sessions that perform large
object work.
