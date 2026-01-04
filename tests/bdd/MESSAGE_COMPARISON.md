# PostgreSQL Protocol Message Comparison

## Overview

BDD tests for pg_doorman include detailed comparison of PostgreSQL protocol messages between the real PostgreSQL server and pg_doorman. When differences are detected, a maximally informative error message is displayed.

## PostgreSQL Protocol Message Format

Each message consists of:
- **Type** (1 byte): a character identifying the message type (e.g., 'R' for Authentication, 'Z' for ReadyForQuery)
- **Length** (4 bytes): message length in big-endian format (including the 4 bytes of length itself)
- **Data**: message content, specific to each type

## Supported Message Types

The `format_message_details` function recognizes and formats the following message types:

### Backend Messages (from server to client)

- **'R'** - AuthenticationRequest
  - Parses authentication type (0=Ok, 3=CleartextPassword, 5=MD5Password)
  - Example: `type='R' len=8 [AuthenticationRequest type=0]`

- **'S'** - ParameterStatus
  - Extracts parameter name and value
  - Example: `type='S' len=25 [ParameterStatus server_version=14.5]`

- **'K'** - BackendKeyData
  - Shows process ID and secret key
  - Example: `type='K' len=12 [BackendKeyData pid=12345 key=67890]`

- **'Z'** - ReadyForQuery
  - Shows transaction status (Idle/InTransaction/FailedTransaction)
  - Example: `type='Z' len=5 [ReadyForQuery status=Idle]`

- **'T'** - RowDescription
  - Shows number of fields
  - Example: `type='T' len=42 [RowDescription fields=3]`

- **'D'** - DataRow
  - Shows number of fields
  - Example: `type='D' len=15 [DataRow fields=1]`

- **'C'** - CommandComplete
  - Extracts command tag
  - Example: `type='C' len=13 [CommandComplete tag='SELECT 1']`

- **'E'** - ErrorResponse
  - Parses severity, code and message
  - Example: `type='E' len=87 [ErrorResponse severity=ERROR code=42601 message=syntax error at or near "bad"]`

- **'N'** - NoticeResponse
  - Parses severity, code and message
  - Example: `type='N' len=50 [NoticeResponse severity=NOTICE code=00000 message=some notice]`

- **'1'** - ParseComplete
  - Example: `type='1' len=4 [ParseComplete]`

- **'2'** - BindComplete
  - Example: `type='2' len=4 [BindComplete]`

- **'t'** - ParameterDescription
  - Shows number of parameters
  - Example: `type='t' len=10 [ParameterDescription params=2]`

- **'n'** - NoData
  - Example: `type='n' len=4 [NoData]`

- **'s'** - PortalSuspended
  - Example: `type='s' len=4 [PortalSuspended]`

### Unknown Types

For unknown message types, a hex dump of the first 32 bytes is displayed:
- Example: `type='X' len=100 [data: 00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f...]`

## Comparison Error Types

### 1. MESSAGE COUNT MISMATCH

When the number of messages differs:

```
=== MESSAGE COUNT MISMATCH ===
PostgreSQL: 5 messages
pg_doorman: 6 messages

=== PostgreSQL messages ===
  [0] type='1' len=4 [ParseComplete]
  [1] type='t' len=10 [ParameterDescription params=1]
  [2] type='n' len=4 [NoData]
  [3] type='2' len=4 [BindComplete]
  [4] type='Z' len=5 [ReadyForQuery status=Idle]

=== pg_doorman messages ===
  [0] type='1' len=4 [ParseComplete]
  [1] type='t' len=10 [ParameterDescription params=1]
  [2] type='n' len=4 [NoData]
  [3] type='2' len=4 [BindComplete]
  [4] type='E' len=87 [ErrorResponse severity=ERROR code=42P05 message=prepared statement already exists]
  [5] type='Z' len=5 [ReadyForQuery status=Idle]
```

### 2. MESSAGE TYPE MISMATCH

When message types at the same position differ:

```
=== MESSAGE TYPE MISMATCH at position 3 ===
PostgreSQL: type='D' len=15 [DataRow fields=1]
pg_doorman: type='E' len=87 [ErrorResponse severity=ERROR code=42601 message=syntax error]
```

### 3. MESSAGE LENGTH MISMATCH

When message lengths of the same type differ:

```
=== MESSAGE LENGTH MISMATCH at position 2 ===
PostgreSQL: type='C' len=13 [CommandComplete tag='SELECT 1']
pg_doorman: type='C' len=14 [CommandComplete tag='SELECT 10']

--- Hex comparison (first 13 bytes) ---
PostgreSQL: 53 45 4c 45 43 54 20 31 00 00 00 00 00
pg_doorman: 53 45 4c 45 43 54 20 31 30 00 00 00 00 00
```

### 4. MESSAGE DATA MISMATCH

When message contents differ:

```
=== MESSAGE DATA MISMATCH at position 1 ===
PostgreSQL: type='S' len=25 [ParameterStatus server_version=14.5]
pg_doorman: type='S' len=25 [ParameterStatus server_version=14.6]

First difference at byte 23: PostgreSQL=0x35 pg_doorman=0x36
Context (bytes 15-25):
  PostgreSQL: 31 34 2e 35 00 00 00 00 00 00
  pg_doorman: 31 34 2e 36 00 00 00 00 00 00
```

## Usage

Tests automatically use improved formatting for any differences. To run tests:

```bash
# Normal run
cargo test --test bdd

# With DEBUG mode for additional output
DEBUG=1 cargo test --test bdd
```

## Benefits

1. **Immediate problem identification**: You can see not only what differs, but also what exactly the messages contain
2. **Context**: Hex dump and context around differences help quickly understand the cause
3. **Readability**: Instead of raw bytes, parsed values are shown (e.g., "severity=ERROR code=42601")
4. **Completeness**: When the number of messages differs, all messages from both sides are shown

## Real-World Scenario Examples

### Successful Comparison

```
Message 0 is identical: type='1' len=4 [ParseComplete]
Message 1 is identical: type='t' len=10 [ParameterDescription params=1]
Message 2 is identical: type='n' len=4 [NoData]
Message 3 is identical: type='2' len=4 [BindComplete]
Message 4 is identical: type='D' len=15 [DataRow fields=1]
Message 5 is identical: type='C' len=13 [CommandComplete tag='SELECT 1']
Message 6 is identical: type='Z' len=5 [ReadyForQuery status=Idle]
```

### Error Detected in pg_doorman

If pg_doorman incorrectly handles prepared statements, you will see:

```
=== MESSAGE COUNT MISMATCH ===
PostgreSQL: 7 messages
pg_doorman: 8 messages

=== PostgreSQL messages ===
  [0] type='1' len=4 [ParseComplete]
  [1] type='t' len=10 [ParameterDescription params=1]
  [2] type='n' len=4 [NoData]
  [3] type='2' len=4 [BindComplete]
  [4] type='D' len=15 [DataRow fields=1]
  [5] type='C' len=13 [CommandComplete tag='SELECT 1']
  [6] type='Z' len=5 [ReadyForQuery status=Idle]

=== pg_doorman messages ===
  [0] type='1' len=4 [ParseComplete]
  [1] type='t' len=10 [ParameterDescription params=1]
  [2] type='n' len=4 [NoData]
  [3] type='E' len=109 [ErrorResponse severity=ERROR code=42P05 message=prepared statement "stmt1" already exists]
  [4] type='2' len=4 [BindComplete]
  [5] type='D' len=15 [DataRow fields=1]
  [6] type='C' len=13 [CommandComplete tag='SELECT 1']
  [7] type='Z' len=5 [ReadyForQuery status=FailedTransaction]
```

This immediately shows that pg_doorman generates an additional ErrorResponse message and changes the transaction status.
