# Deferred BEGIN batching: rejected for generic queries

Status: design analysis complete; implementation rejected.
Target: thread-per-core PR, simple-query transaction path.
Reviewers: two focused Astra GPT-6.2 design reviews.

## Decision

Do not join or batch a deferred BEGIN with an arbitrary following query under
an unchanged PostgreSQL semantics contract.

The existing response barrier must remain:

```text
send Q(BEGIN)
receive CommandComplete(BEGIN) + ReadyForQuery(T)
only then send the next client query
```

No code change is proposed by this document.

## Existing optimization

`src/client/transaction.rs` already intercepts an exact standalone BEGIN and
returns synthetic `CommandComplete(BEGIN) + ReadyForQuery(T)` to the client
without acquiring a backend. When the next message arrives, the current path:
acquires a backend, sends BEGIN, drains its response through `io::sink`, then
sends the next query.

The investigated goal was to remove the additional server write/roundtrip.
Odyssey `d83337a` joins deferred BEGIN and the next SQL query in
`sources/relay.c:350-391`. Directly copying that behavior is not semantics-
preserving for pg_doorman.

## Alternative 1: one Q containing joined SQL

Rejected construction:

```text
Q("BEGIN;\n;SELECT 1")
```

### Blockers

PostgreSQL parses the complete simple Query before executing its first
statement. Consequently:

- a syntax error in the second statement can prevent BEGIN from executing;
- after synthetic BEGIN success, the client can receive ReadyForQuery(I)
  instead of the expected failed-transaction state;
- `current_query()` exposes the prefixed SQL;
- ErrorResponse position fields refer to the joined SQL rather than the
  client's original SQL;
- statement timestamps share the BEGIN message boundary;
- an empty follow-up loses its independent EmptyQueryResponse because the
  combined parse tree is nonempty;
- COPY can start before the separate BEGIN ReadyForQuery(T) boundary.

Filtering `CommandComplete(BEGIN)` fixes none of these differences.

### Aggregated response hazard

`Server::recv()` can return BEGIN completion and following query frames in one
buffer. Dropping the first receive buffer would also drop SELECT, ErrorResponse,
ReadyForQuery or COPY frames. Even if SQL joining were otherwise acceptable,
suppression would have to remove exactly one protocol frame, never an arbitrary
buffer.

## Alternative 2: two independent Q frames in one write

Rejected construction:

```text
Q("BEGIN;") Q("INSERT ...")
```

This preserves SQL text, parse boundaries, error positions, statement
timestamps, empty-query behavior and the first ReadyForQuery(T). It is safer
than joined SQL, but still has a transaction-safety blocker.

### P0: BEGIN failure does not remove Q2 from PostgreSQL's input queue

Both queries have already reached the backend. If BEGIN is cancelled or fails
before opening a transaction, PostgreSQL can produce:

```text
Q1 response: ErrorResponse + ReadyForQuery(I)
Q2 response: normal result + ReadyForQuery(I)
```

Simple Query errors do not skip the next independently framed Query. The
extended-protocol skip-until-Sync rule does not apply here.

An already queued INSERT/UPDATE/DELETE or other side-effecting query can
therefore execute and commit as a standalone transaction. Closing or marking
the backend bad cannot undo that commit. A CancelRequest targets the backend,
not specifically Q2, and does not make this safe.

### Insufficient restrictions

- Restricting Q2 to text starting with SELECT is not sufficient: SELECT can
  call volatile or side-effecting functions.
- Matching the sysbench SELECT shape would be workload-specific behavior, not
  a generally correct pooler optimization.
- Batching only COMMIT/ROLLBACK avoids most data-write risk but does not help
  the target sysbench transactions and does not justify a generic fast path.
- Converting the queries to extended protocol changes semantics and is outside
  this optimization.

## Required safe behavior

For arbitrary Q2, retain the response barrier. Q2 must not be sent until the
pooler has positively established:

```text
CommandComplete(BEGIN) + ReadyForQuery(T)
```

A successful client synthetic response alone is not proof of backend
transaction state.

The current sink loop at `src/client/transaction.rs:1083-1100` primarily checks
`is_data_available()`. If that code is revisited independently, it should
positively verify successful BEGIN completion and ReadyForQuery(T), not merely
drain currently available bytes. That is correctness hardening, not a license
to batch Q2.

## Corner cases recorded by the reviews

Any future proposal must preserve all of these:

- second-query syntax error: ReadyForQuery(E), then working ROLLBACK;
- second-query runtime error or cancellation: failed transaction, not I;
- BEGIN error/cancellation: Q2 must never have been sent;
- empty, whitespace-only and comment-only Q2: native EmptyQueryResponse;
- `current_query()`, ErrorResponse.P and statement_timestamp(): original SQL
  and query boundaries;
- multi-statement Q2: native PostgreSQL behavior;
- COPY IN/OUT: BEGIN Z(T) before CopyIn/CopyOut, correct CopyDone/CopyFail and
  final ReadyForQuery;
- DataRow/COPY streaming can produce an empty post-recv buffer, which is not
  end-of-response;
- no backend release between Q1 and Q2;
- async Notice/Notification/ParameterStatus do not terminate response tracking;
- health-check and DEALLOCATE handlers retain their precedence over pending
  BEGIN; Terminate retains its no-checkout behavior;
- simple BEGIN followed by extended Parse/Bind/Execute/Sync uses the existing
  fallback;
- clients with pending BEGIN continue to defer binary migration;
- default shared-runtime behavior remains identical to fixed-worker behavior.

The existing BEGIN matcher should also validate the stored frame's final NUL
before any future rewriting; checking only the SQL bytes can silently repair
malformed input.

## Tests required before reconsidering any implementation

1. A protocol-capable fixture returns ErrorResponse + ReadyForQuery(I) for
   BEGIN and asserts that the follow-up write is never sent.
2. Raw-client tests assert complete client response sequences, not only row
   values.
3. Syntax/runtime error, empty query, COPY, cancellation and ROLLBACK recovery
   tests cover the cases above.
4. A recording proxy decodes Q frames; TCP reads are not Q frames.
5. To prove removal of the response barrier, a fixture would withhold BEGIN's
   ReadyForQuery until Q2 arrives with a bounded timeout. For the safe current
   design, Q2 must instead time out and remain unsent until Z(T).
6. Application send-call counting must instrument `send_and_flush`; `write_all`
   does not guarantee one socket write.
7. Existing deferred-BEGIN, cancellation, COPY, prepared-statement and migration
   suites run in both `thread_per_core=false` and `true` modes.

## Performance conclusion

The potential saving was one server write and possibly one response wakeup per
eligible transaction. Neither generic construction preserves transaction
semantics, so an A/B benchmark is not a substitute for correctness and no
implementation should proceed.

Future CPU work should target overhead that does not remove the BEGIN success
barrier: response parsing, per-query accounting, allocations, or genuinely
independent socket/TCP costs. Do not resurrect SQL joining or arbitrary two-Q
batching as a hot-path fast path.
