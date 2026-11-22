# Join deferred BEGIN with the next simple query

Status: accepted Odyssey-compatible optimization; implementation in this PR.
Review history: two Astra GPT-6.2 reviews, followed by an explicit product
decision to accept the error/ROLLBACK compatibility trade-off.

## Goal

Remove the separate server roundtrip for a deferred BEGIN by combining BEGIN
and the following simple query in one PostgreSQL Simple Query message.

Current client behavior is unchanged for a standalone BEGIN: pg_doorman
immediately returns synthetic `CommandComplete(BEGIN) + ReadyForQuery(T)` and
does not acquire a backend until the next client message.

## Selected design

When the next frontend message is simple Query (`Q`), send one joined Q:

```text
Q("BEGIN;\n;<original SQL>")
```

The stored BEGIN frame is reused: remove its NUL, append `b"\n;"`, append the
original SQL body including its NUL, and recompute the Q length with checked
arithmetic. Validate the stored BEGIN frame, including its final NUL.

The real `CommandComplete(BEGIN)` is hidden because the client already received
the synthetic completion. `Server::recv()` still parses all frames and updates
transaction state and statistics normally.

Non-Q follow-ups retain the existing separate BEGIN roundtrip. Extended
Parse/Bind/Execute/Sync behavior is not changed.

## Explicitly accepted compatibility differences

One joined Simple Query is not identical to two independent Simple Queries.
The following Odyssey-compatible differences are accepted:

- PostgreSQL parses the whole joined Q before executing BEGIN. A syntax or
  analysis error can therefore prevent BEGIN from executing and produce
  ReadyForQuery(I) instead of ReadyForQuery(E).
- The client may consequently see a PostgreSQL warning on ROLLBACK saying
  there is no transaction. ROLLBACK is the supported recovery and must leave
  the client able to issue subsequent queries.
- `current_query()` can expose the BEGIN prefix.
- ErrorResponse position fields refer to the joined SQL.
- Statement timestamps use the joined Q boundary.
- An empty follow-up loses its independent EmptyQueryResponse.

No erroneous SQL is reported as successful. The real ErrorResponse is forwarded
unchanged. If PostgreSQL did not execute BEGIN because parsing failed, it also
did not execute the following SQL, so this trade-off does not commit the
client's failed query outside a transaction.

Two independent Q frames in one socket write were rejected: if BEGIN fails,
PostgreSQL can still execute the already queued second Q as a standalone
transaction. Joined SQL does not have that particular data-safety problem.

## Response suppression

`Server::recv()` can aggregate multiple backend frames in one `BytesMut`.
Dropping the whole first response buffer is therefore incorrect.

The suppression helper parses complete frame boundaries and:

1. removes exactly one CommandComplete whose tag is `BEGIN`;
2. preserves all following frames in their original order;
3. preserves async frames;
4. if ErrorResponse appears before BEGIN completion, returns the response
   unchanged — this is the accepted joined-parse-error case;
5. rejects a ReadyForQuery or truncated stream with neither BEGIN completion
   nor ErrorResponse as a protocol synchronization error.

Suppression is client-visible only. It happens after server parsing, so BEGIN
completion and ReadyForQuery continue to update server state and accounting.

The flag is consumed on the first received response. It must not suppress a
frame in a later DataRow/COPY chunk.

## Transaction behavior

### Successful query

```text
client: BEGIN;
pooler -> client: synthetic C(BEGIN) + Z(T)

client: SELECT 1
pooler -> PostgreSQL: Q("BEGIN;\n;SELECT 1")
PostgreSQL -> pooler: C(BEGIN), query frames, Z(T)
pooler -> client: query frames, Z(T)
```

### Parse error before BEGIN

```text
PostgreSQL -> pooler: ErrorResponse, Z(I)
pooler -> client: ErrorResponse, Z(I)
client: ROLLBACK
client -> pooler: native PostgreSQL ROLLBACK response; warning is acceptable
client: SELECT 1 -- must work normally afterwards
```

### Runtime error after BEGIN

PostgreSQL has already executed BEGIN, so the joined Q produces the normal
ErrorResponse and ReadyForQuery(E). ROLLBACK must restore ReadyForQuery(I).

### COMMIT or ROLLBACK as the first following query

Only the real BEGIN completion is suppressed. The original COMMIT/ROLLBACK
completion and final ReadyForQuery(I) are forwarded, and normal backend release
applies.

### COPY and streamed rows

COPY and large DataRow responses may split output across recv calls. Only the
first response is inspected for BEGIN suppression. Empty returned buffers from
streaming handlers remain subject to the existing `is_data_available()` and
COPY state machine; they are not treated as transaction completion.

### Extended protocol, health checks and migration

Health-check/DEALLOCATE precedence and Terminate behavior are unchanged.
Non-Q extended protocol uses the old separate path. A client with pending BEGIN
continues to defer binary migration.

## Implementation scope

- `src/client/util.rs`: joined-Q construction, exact response suppression and
  unit tests.
- `src/client/transaction.rs`: simple-Q join branch and one-shot response
  suppression flag.
- Existing deferred-BEGIN and migration BDD suites.
- New BDD coverage for successful SELECT, parse error + ROLLBACK, runtime error
  + ROLLBACK, COMMIT/ROLLBACK and mixed simple-to-extended fallback.

No buffer resizing, TCP tuning, pool changes or new configuration are part of
this patch.

## Required validation

Unit tests:

- exact joined Q bytes and recomputed length;
- malformed stored BEGIN rejection;
- exact BEGIN-frame removal with following ReadyForQuery;
- unchanged ErrorResponse + ReadyForQuery(I) when BEGIN completion is absent.

BDD tests:

- all existing deferred-BEGIN simple and extended scenarios;
- BEGIN + SELECT returns the expected row and ReadyForQuery(T);
- BEGIN + syntax error, ROLLBACK, then successful SELECT;
- BEGIN + runtime error, ROLLBACK, then successful SELECT;
- BEGIN + COMMIT/ROLLBACK returns final ReadyForQuery(I);
- BEGIN followed by extended protocol retains its previous responses;
- client migration scenarios in both `thread_per_core=false` and `true` modes;
- cancellation and COPY suites before merge.

## A/B evaluation

Benchmark the current PR head versus this patch with the same 500-thread,
60-second simple-query sysbench command and fixed-worker configuration.

Measure TPS, QPS, average/p95 latency, pg_doorman CPU time per transaction and
server writes per eligible transaction. The joined path removes one server
write and the BEGIN-only response relay per eligible transaction. Actual CPU
savings must be measured rather than inferred from Odyssey.

## Review gates

1. Review joined-Q length/frame construction and one-shot suppression.
2. Confirm parse errors without BEGIN completion are forwarded, not rejected.
3. Confirm ROLLBACK recovery and no erroneous SQL execution.
4. Confirm non-Q extended protocol and migration behavior are unchanged.
5. Run existing regression suites and retain paired benchmark output.
