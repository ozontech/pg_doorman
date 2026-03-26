# Pipeline disconnect: server connection corruption research

## Proven bug

Client sends `SELECT $1` with ~4MB text parameter, kills TCP socket (RST) after reading
first row. Next client on the same server connection gets:
```
Received backend message BindComplete while expecting ParseCompleteMessage. Please file a bug.
```

Reproduces 100% at `message_size_to_be_stream` <= 64KB. Does NOT reproduce at default 1MB.

## Root cause (verified with DEBUG logs)

Two things happen together:

### 1. Fast-release returns server to pool before pg_doorman detects client death

With low `message_size_to_be_stream` (2-64KB), `handle_large_data_row` streams the 4MB
DataRow directly to client. On localhost TCP buffers absorb the data — streaming succeeds.
`recv()` returns Ok. Roundtrip loop reads remaining small messages (CommandComplete,
ReadyForQuery). Fast-release triggers: server returned to pool.

**Only after that**, pg_doorman tries to read the next message from client (transaction.rs:543).
Client is dead → `UnexpectedEof`. Error goes through `process_error` → server already in pool.

With default 1MB threshold, streaming takes longer (same 4MB DataRow, but different buffering).
Client's RST is detected **during** streaming → `BrokenPipe` during `write_all_flush` →
`mark_bad` → server NOT returned to pool.

**Debug log proof:**
- 2KB: `Error reading message code from socket - UnexpectedEof` (read from client, not write)
- 1MB: `Error writing to socket: BrokenPipe` (write to client during streaming)

### 2. `handle_large_data_row` writes `server.buffer` directly to client, bypassing response reordering

`handle_large_data_row` (protocol_io.rs:112-116) writes `server.buffer` to client socket.
This buffer may contain accumulated messages (BindComplete, RowDescription) that should go
through `reorder_parse_complete_responses` (transaction.rs:976) to insert synthetic
ParseComplete for cached/skipped Parse operations.

When Client B connects to the same server connection (pool_size=1), pg_doorman's prepared
statement cache has DOORMAN_0 from Client A. Client B's Parse is skipped → synthetic
ParseComplete should be inserted. But `handle_large_data_row` sends BindComplete to client
before reordering happens → protocol violation.

**Debug log proof:**
```
Parse skipped for `DOORMAN_0` (already on server), will insert ParseComplete later
PROTOCOL WARNING: Server has pending operations from previous client: 1xParse,1xBind,1xDescribe
Reordering responses: operations=4, skipped_parses=1  ← reordering runs, but TOO LATE
```

## Affected code

| File | Lines | What |
|------|-------|------|
| `src/server/protocol_io.rs` | 112-116 | `handle_large_data_row` writes buffer directly to client |
| `src/server/protocol_io.rs` | 381-385 | `recv()` calls `handle_large_data_row` with accumulated buffer |
| `src/client/transaction.rs` | 996-1006 | fast-release: server to pool before write to client |
| `src/client/transaction.rs` | 891-895 | deferred write after server release |
| `src/client/transaction.rs` | 543-545 | client read error after server already released |
| `src/client/transaction.rs` | 970-977 | `reorder_parse_complete_responses` — runs too late |

## Sequence of events

### Client A (first query, Parse NOT skipped)
```
1. Parse("", "SELECT $1") → DOORMAN_0, server cache miss → Parse sent to server
2. Server responds: ParseComplete + BindComplete + RowDescription + DataRow(4MB) + ...
3. recv() buffers ParseComplete, BindComplete, RowDescription
4. DataRow > threshold → handle_large_data_row
5. Streams 4MB to client → TCP buffer absorbs → Ok
6. recv() reads CommandComplete, ReadyForQuery
7. Fast-release → server to pool (CLEAN from PostgreSQL perspective)
8. write_all_flush to client → Ok (still in TCP buffer)
9. Client reads row, kills socket (RST)
10. Next read from client → UnexpectedEof → process_error
11. Server already in pool with DOORMAN_0 cached
```

### Client B (second query, Parse SKIPPED)
```
1. Parse("", "SELECT $1") → DOORMAN_0, server cache HIT → Parse SKIPPED
2. pg_doorman sends: Bind(DOORMAN_0) + Describe + Execute + Sync (no Parse!)
3. Server responds: BindComplete + RowDescription + DataRow(4MB) + ...
4. recv() buffers BindComplete, RowDescription
5. DataRow > threshold → handle_large_data_row
6. Line 116: write_all_flush(client, [BindComplete + RowDescription + DataRow_header])
7. Client receives BindComplete BEFORE synthetic ParseComplete
8. Protocol violation: "BindComplete while expecting ParseCompleteMessage"
```

## Fix direction

The core fix: `handle_large_data_row` must not write accumulated buffer directly to client.
Instead, when buffer is non-empty, return it from `recv()` for processing through
`reorder_parse_complete_responses`. Stream the large DataRow on the next `recv()` call.

Secondary: when fast-release returns server to pool, and the subsequent client write fails,
the server connection must be invalidated (mark_bad) retroactively.
