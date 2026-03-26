# Pipeline disconnect: server connection corruption research

## Overview

Client sends a large query, kills TCP socket (RST) mid-transfer. Next client on the same
server connection gets protocol violation. Four separate code paths contribute to the problem.

## BUG-1: `handle_large_data_row` bypasses response reordering

**Status: reproduced 100%**

### Code trace

When `message_size_to_be_stream` is low (e.g. 2048), DataRow messages larger than the threshold
go through `handle_large_data_row` (protocol_io.rs:102-146). This function writes `server.buffer`
directly to the client socket (line 116), bypassing `reorder_parse_complete_responses`
(transaction.rs:976).

Normal path:
```
recv() → buffer messages → return buffer
  → execute_server_roundtrip → reorder_parse_complete_responses → write to client
```

Streaming path:
```
recv() → buffer messages → encounter large DataRow
  → handle_large_data_row → write_all_flush(client, server.buffer) ← BYPASSES REORDER
  → stream DataRow payload directly
```

If Parse was skipped (prepared statement cache hit), the buffer contains BindComplete without
a synthetic ParseComplete. Client receives BindComplete when it expects ParseComplete.

### Affected code

- `src/server/protocol_io.rs:381-385` — recv calls handle_large_data_row
- `src/server/protocol_io.rs:112-116` — handle_large_data_row writes buffer directly
- `src/client/transaction.rs:970-977` — reorder_parse_complete_responses (never reached)

### Reproduction

Config: `message_size_to_be_stream = 2048`, `pool_size = 1`, `prepared_statements = true`
Client: Npgsql with ~4MB text parameter, kill socket after reading first row.

---

## BUG-2: Write error swallowed in non-async/non-copy mode

**Status: needs test**

### Code trace

In `execute_server_roundtrip` (transaction.rs:1010-1024):

```rust
if let Err(err_write) = write_all_flush(&mut self.write, &response).await {
    server.wait_available().await;
    if server.is_async() || server.in_copy_mode() {
        server.mark_bad(...);
        return Err(err_write);
    }
    // error swallowed, loop continues
}
```

For non-async, non-copy connections: write error is logged, `wait_available()` drains some data,
but the error is **not returned** and the server is **not marked bad**. The loop continues to
the `is_data_available()` check (line 1029) and may break or continue.

### Risk

If `wait_available()` doesn't drain all pending data (server still processing, network latency),
the connection returns to pool with partial response data in server's BufStream.

### Affected code

- `src/client/transaction.rs:1010-1024` — write error handling
- `src/server/server_backend.rs:265-285` — wait_available relies on data_available flag

---

## BUG-3: Async mode connections skip `checkin_cleanup`

**Status: needs test**

### Code trace

At transaction end (transaction.rs:879-881):

```rust
} else if !server.is_async() {
    server.checkin_cleanup().await?;
}
```

When `server.is_async() == true`, `checkin_cleanup()` is skipped. The server returns to pool
without checking:
- `is_data_available()` — may have pending data
- `buffer.is_empty()` — may have buffered messages
- `in_transaction()` — may need ROLLBACK
- `cleanup_state` — may need RESET ALL

Additionally, `async_mode` and `expected_responses` fields are not reset, potentially corrupting
the next client's response counting.

### Affected code

- `src/client/transaction.rs:879-881` — conditional cleanup
- `src/server/server_backend.rs:332-404` — checkin_cleanup checks

---

## BUG-4: Fast-release returns server before client write

**Status: needs test**

### Code trace

Fast-release path (transaction.rs:996-1006):

```rust
if can_fast_release && !server.is_data_available() && ... {
    self.client_last_messages_in_tx.put(&response[..]);
    break;  // server released to pool here (Drop)
}
```

Server is released (line 889 via Drop of `Object<Server>`). Then:

```rust
// line 891-895: server already in pool
if !self.client_last_messages_in_tx.is_empty() {
    write_all_flush(&mut self.write, &self.client_last_messages_in_tx).await?;
}
```

If client is dead (RST), write fails, error propagates via `?`. Server is already in pool.
From PostgreSQL's perspective the connection is clean (ReadyForQuery received). But pg_doorman's
prepared statement cache state may be stale — the server has prepared statements that pg_doorman
will try to skip Parse for on the next client.

### Risk

Theoretical: if the next client sends the same query, pg_doorman skips Parse (cache hit),
sends Bind/Execute. Server responds with BindComplete. If pg_doorman's reorder logic has
any issue with the new client's state vs the cached server state, protocol violation.

### Affected code

- `src/client/transaction.rs:996-1006` — fast release
- `src/client/transaction.rs:891-895` — deferred write

---

## Common factors

All four bugs share a common theme: **the code assumes that if data was successfully read from
the server, it will be successfully written to the client.** When the client dies mid-transfer,
this assumption breaks and various invariants are violated.

The streaming path (BUG-1) is the most severe because it bypasses the response reordering
that inserts synthetic protocol messages for cached prepared statements.
