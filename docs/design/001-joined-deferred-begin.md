# Join deferred BEGIN with the next simple query

Status: design only; no implementation in this commit.
Target: current thread-per-core PR, simple-query sysbench transaction path.

## Goal

Complete the existing deferred-BEGIN optimization by sending the deferred
`BEGIN` and the next simple query to PostgreSQL as one frontend `Q` message.
This removes a separate server write and BEGIN-only response relay from every
eligible transaction.

This follows Odyssey `d83337a`:
`sources/relay.c:350-391` joins the query and consumes the first
`CommandComplete(BEGIN)` before relaying the remaining response.

## Current behavior

`src/client/transaction.rs` already intercepts an exact standalone `BEGIN` and
returns a synthetic `CommandComplete(BEGIN) + ReadyForQuery(T)` to the client.
No backend is acquired at that point.

When the next client message arrives, pg_doorman currently:

```text
client: BEGIN
pooler -> client: synthetic BEGIN response

client: SELECT 1
pooler -> PostgreSQL: Q("BEGIN;")
PostgreSQL -> pooler: CommandComplete(BEGIN) + ReadyForQuery(T)
pooler: discard that response through io::sink

pooler -> PostgreSQL: Q("SELECT 1")
PostgreSQL -> pooler -> client: query response
```

The client-side optimization is useful, but the backend still performs two
separate roundtrips and the pooler runs a separate timeout, send, receive and
suppression path for BEGIN.

## Proposed behavior

Only when both conditions are true:

- `client_pending_begin` contains the exact standalone BEGIN accepted by the
  existing `is_standalone_begin()` check;
- the next frontend message is simple Query (`Q`);

send one joined message:

```text
client: BEGIN
pooler -> client: synthetic BEGIN response       (unchanged)

client: SELECT 1
pooler -> PostgreSQL: Q("BEGIN;\n;SELECT 1")
PostgreSQL -> pooler:
    CommandComplete(BEGIN)                        suppress exactly this frame
    RowDescription/DataRow/CommandComplete(SELECT)
    ReadyForQuery(T)
pooler -> client: everything except real CommandComplete(BEGIN)
```

All other protocol messages keep the existing separate-BEGIN path. In
particular, this change does not join BEGIN with Parse/Bind/Execute/Sync,
FunctionCall, COPY data, Flush or Terminate.

No new configuration option is needed. The optimization applies only to the
existing narrow BEGIN fast path and can be removed without changing the wire
contract seen by clients.

## Wire construction

Reuse the stored BEGIN `BytesMut` where practical:

1. Remove its trailing NUL.
2. Append `b"\n;"` to terminate the BEGIN statement unambiguously.
3. Append the next Q message's SQL body, including its trailing NUL.
4. Recompute the Q length using checked arithmetic and the protocol `i32`
   limit.

The result must contain exactly one frontend Q frame. Do not concatenate two
complete Q frames: PostgreSQL would interpret the second frame's type byte as
SQL text.

The separator is required even though the current BEGIN matcher accepts only a
small exact spelling. It also keeps future BEGIN variants safe around trailing
comments. This optimization does not broaden the BEGIN parser.

An oversized or otherwise invalid joined message must return an error through
the existing connection-error path; it must never wrap the length or truncate
SQL.

## Response suppression

This is the correctness-critical part.

`Server::recv()` can return several complete backend frames in one `BytesMut`.
Therefore "drop the first response buffer" is incorrect: it can also drop the
SELECT RowDescription, DataRow, ErrorResponse or ReadyForQuery.

After `recv()` and before normal client response processing:

1. Parse complete backend frame boundaries in the returned buffer.
2. Locate the first non-async command result for the joined BEGIN.
3. Suppress exactly one CommandComplete whose command tag is `BEGIN`.
4. Preserve every other frame and their original order.
5. Preserve async NoticeResponse, NotificationResponse and ParameterStatus.
6. Leave server-side transaction, COPY, statistics and error tracking to
   `Server::recv()` exactly as today.

Suppression happens only in the client-visible buffer, after the server has
parsed the frame. Do not skip parsing the frame at the socket layer.

If the BEGIN CommandComplete and subsequent frames are split across reads,
suppression applies only to the matching BEGIN frame in the first relevant
read. If they share a read, removal must not discard the remainder.

## Corner cases

### Empty transaction

Next Q is COMMIT or ROLLBACK:

```text
Q("BEGIN;\n;COMMIT;")
```

Suppress only CommandComplete(BEGIN). Forward CommandComplete(COMMIT) and
ReadyForQuery(I). The fast-release path must still release the backend.

The same applies to ROLLBACK. Existing empty-transaction simple and extended
protocol BDD scenarios must remain unchanged and pass.

### Empty query

Next Q is an empty query. Preserve EmptyQueryResponse and ReadyForQuery(T)
after suppressing the BEGIN CommandComplete.

### Query error

For `BEGIN + failing SELECT`, suppress the successful BEGIN CommandComplete
but forward the SELECT ErrorResponse and ReadyForQuery(T). A following
ROLLBACK must work normally.

Do not synthesize a second error or hide a query error as a side effect of
BEGIN suppression.

### BEGIN failure or connection loss

Exact standalone BEGIN has few ordinary SQL failure cases, but connection,
timeout and server failures remain possible. If no matching BEGIN
CommandComplete arrives:

- stop the joined roundtrip through the existing error path;
- mark the backend bad when framing or connection state is uncertain;
- do not forward a partial or ambiguous response as a successful query;
- do not return the backend to the idle pool unless normal cleanup has
  established a known protocol boundary.

The client already received synthetic BEGIN success; that pre-existing
trade-off is unchanged.

### Multiple statements in the next Q

Joining is valid. Suppress only the first BEGIN CommandComplete and forward all
responses belonging to the original client Q, including multiple result sets.

### COPY

`BEGIN + COPY ... FROM STDIN` may produce CopyInResponse after the BEGIN
CommandComplete. Preserve it and enter the existing COPY state machine.
`BEGIN + COPY ... TO STDOUT` must relay CopyOutResponse and CopyData normally.

Any uncertain COPY/protocol state must use the existing bad-backend handling;
do not release early.

### Extended protocol and pipeline mode

Do not join non-Q messages. Existing deferred-BEGIN behavior for Parse/Bind/
Execute/Sync and pipeline ROLLBACK must remain byte-for-byte compatible.

### Async messages and parameter changes

Notice, notification and ParameterStatus frames are not BEGIN completion.
Preserve them and continue looking for the exact BEGIN CommandComplete.

### Client disconnect or cancellation

Before the next query, no backend is acquired, as today. After the joined Q is
sent, use the existing query cancellation and disconnect behavior. Suppression
must not make a partially consumed backend look reusable.

### Binary upgrade

A client with `client_pending_begin` already defers migration. That behavior
must not change. Once the joined query is sent, normal migration restrictions
for active transactions apply.

### Statistics and timing

There is one server send and one combined response relay. Server-side parsing
still observes both command completions, so command statistics must not be
silently dropped with the client-visible BEGIN frame.

Query timing should use the existing outer roundtrip timestamp; do not add a
second BEGIN-only timing interval or reset the clock between joined commands.

## Implementation boundaries

Expected changes are limited to:

- a small tested Q-frame join helper;
- the pending-BEGIN Q branch in `src/client/transaction.rs`;
- exact client-visible BEGIN frame removal in the existing response path;
- focused unit and BDD tests.

Do not combine this patch with buffer resizing, frame relay, TCP tuning or new
pool configuration. Those changes were already tried or are orthogonal and
would make attribution impossible.

## Required tests

Unit tests:

- joined Q has exact length, separator, one trailing NUL and original SQL;
- long input length overflow is rejected;
- BEGIN CommandComplete is removed when alone in a response buffer;
- BEGIN CommandComplete is removed when followed by SELECT frames in the same
  buffer;
- frame-split reads and preserved async messages are handled correctly;
- only the exact BEGIN tag is suppressed, not SELECT/COMMIT or an arbitrary
  CommandComplete.

Existing BDD suites that must pass:

- `deferred-begin-optimization.feature`;
- `rust-deferred-begin-bug.feature`;
- client migration and migration edge cases;
- transaction pooling, COPY and prepared-statement suites.

New BDD coverage:

- BEGIN + SELECT returns exactly one result set and no duplicate BEGIN;
- BEGIN + COMMIT and BEGIN + ROLLBACK return ReadyForQuery(I) and reuse the
  backend correctly;
- BEGIN + failing SELECT returns the query error and allows ROLLBACK;
- BEGIN + empty query;
- BEGIN + COPY;
- a blackhole/proxy wire capture proves one Q containing both statements and
  proves the client never receives the real BEGIN CommandComplete.

Run the same scenarios with `thread_per_core=false` and `true` where the
existing harness supports parameterization; joined BEGIN is orthogonal to the
scheduler and must not introduce a fixed-worker-only behavior.

## A/B evaluation

Use the existing 500-thread, 60-second simple-query sysbench benchmark and the
same fixed-worker configuration. Compare against the current PR head, not an
older stock or pre-local/global build.

Measure:

- TPS, QPS, average and p95 latency;
- pg_doorman CPU time per transaction;
- server writes per transaction (one fewer for eligible BEGIN transactions);
- receive/readiness wakeups and context switches;
- errors and transaction-state desynchronization.

A previous Odyssey A/B attributed roughly 3-4% throughput to its broader
virtual-transaction behavior. Treat that as context, not as a promised result
for this narrower pg_doorman patch. The primary goal is lower CPU per
transaction with unchanged protocol behavior.

## Review gates

Before merging:

1. Review wire joining and suppression independently.
2. Run all existing deferred-BEGIN and migration BDD scenarios.
3. Exercise both shared and fixed-worker runtimes.
4. Verify failure paths do not recycle an uncertain backend.
5. Benchmark against the current PR head and retain the paired output.
