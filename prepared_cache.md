# Prepared Statement Cache Architecture

This document describes how pg_doorman stores and manages prepared statements to optimize memory usage and improve performance.

## Overview

pg_doorman uses a two-level caching system for prepared statements:

1. **Pool-level cache** — A shared cache for all clients in a pool (LRU eviction)
2. **Client-level cache** — A per-client mapping from client statement names to shared cache entries

Additionally, a **Query String Interner** ensures that identical SQL query texts share the same memory allocation across all clients.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Pool Level                                      │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │              PreparedStatementCache (LRU, max_size entries)          │    │
│  │  ┌──────────┬──────────────────────────────────────────────────┐    │    │
│  │  │   hash   │              Arc<Parse>                          │    │    │
│  │  ├──────────┼──────────────────────────────────────────────────┤    │    │
│  │  │ 0x1a2b.. │ { name: "DOORMAN_1", query: Arc<str>, params }   │    │    │
│  │  │ 0x3c4d.. │ { name: "DOORMAN_2", query: Arc<str>, params }   │    │    │
│  │  │ 0x5e6f.. │ { name: "DOORMAN_3", query: Arc<str>, params }   │    │    │
│  │  └──────────┴──────────────────────────────────────────────────┘    │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                    QUERY_INTERNER (global)                           │    │
│  │  ┌──────────┬────────────────────────────────────────────────┐      │    │
│  │  │   hash   │                   Arc<str>                      │      │    │
│  │  ├──────────┼────────────────────────────────────────────────┤      │    │
│  │  │ 0x1a2b.. │ "SELECT * FROM users WHERE id = $1"            │      │    │
│  │  │ 0x3c4d.. │ "INSERT INTO orders (user_id) VALUES ($1)"     │      │    │
│  │  └──────────┴────────────────────────────────────────────────┘      │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                             Client Level                                     │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                    Client 1 (PreparedStatementState)                 │    │
│  │  ┌─────────────────┬─────────────────────────────────────────┐      │    │
│  │  │  client_name    │           CachedStatement                │      │    │
│  │  ├─────────────────┼─────────────────────────────────────────┤      │    │
│  │  │ "my_query"      │ { parse: Arc<Parse>↑, hash, async_name } │      │    │
│  │  │ "" (anonymous)  │ { parse: Arc<Parse>↑, hash, async_name } │      │    │
│  │  └─────────────────┴─────────────────────────────────────────┘      │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                    Client 2 (PreparedStatementState)                 │    │
│  │  ┌─────────────────┬─────────────────────────────────────────┐      │    │
│  │  │  client_name    │           CachedStatement                │      │    │
│  │  ├─────────────────┼─────────────────────────────────────────┤      │    │
│  │  │ "stmt1"         │ { parse: Arc<Parse>↑, hash, async_name } │      │    │
│  │  └─────────────────┴─────────────────────────────────────────┘      │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────────┘

                    ↑ = shared reference (Arc pointer)
```

## Components

### 1. Pool-Level Cache (`PreparedStatementCache`)

**Location:** `src/server/prepared_statement_cache.rs`

The pool-level cache is shared among all clients connected to the same database/user pool. It stores `Arc<Parse>` objects indexed by a hash of the query text and parameter types.

**Key characteristics:**
- **LRU eviction** — When the cache reaches `max_size`, the least recently used entry is evicted
- **Lock-free reads** — Uses `DashMap` for concurrent access with minimal contention
- **Configurable size** — Set via `prepared_statements_cache_size` (default: 8192)

**Structure:**
```rust
struct PreparedStatementCache {
    cache: DashMap<u64, CacheEntry>,  // hash → (Arc<Parse>, count_used)
    max_size: usize,
    counter: AtomicU64,               // for LRU ordering
}
```

### 2. Query String Interner (`QUERY_INTERNER`)

**Location:** `src/server/prepared_statement_cache.rs`

The query interner ensures that identical SQL query texts share the same `Arc<str>` allocation, even after the corresponding `Arc<Parse>` is evicted from the pool cache.

**Key characteristics:**
- **Global singleton** — One interner for the entire process
- **Never evicts** — Entries are kept as long as any client holds a reference
- **Memory efficient** — Identical queries share one allocation

**Structure:**
```rust
static QUERY_INTERNER: DashMap<u64, Arc<str>>
```

### 3. Client-Level Cache (`PreparedStatementState`)

**Location:** `src/client/core.rs`

Each client maintains its own mapping from client-provided statement names to cached statement info.

**Key characteristics:**
- **Name mapping** — Maps client's statement name (e.g., "my_query") to shared `Arc<Parse>`
- **Supports anonymous statements** — Uses query hash as key for unnamed statements
- **Optional size limit** — Set via `client_prepared_statements_cache_size` (default: 0 = unlimited)

**Structure:**
```rust
struct PreparedStatementState {
    cache: AHashMap<PreparedStatementKey, CachedStatement>,
    // ... other fields
}

struct CachedStatement {
    parse: Arc<Parse>,           // shared from pool cache
    hash: u64,                   // query hash
    async_name: Option<String>,  // unique name for async clients
}
```

### 4. Server-Level Cache (per connection)

**Location:** `src/server/server_backend.rs`

Each PostgreSQL server connection tracks which prepared statements have been registered on that specific backend.

**Structure:**
```rust
struct Server {
    prepared_statement_cache: Option<LruCache<String, ()>>,  // statement names only
    // ... other fields
}
```

## Data Flow

### When a Client Sends a Parse Message

```
Client                    pg_doorman                         PostgreSQL
  │                           │                                  │
  │── Parse "my_stmt" ───────>│                                  │
  │   query: "SELECT..."      │                                  │
  │                           │                                  │
  │                     1. Compute hash of query + params        │
  │                           │                                  │
  │                     2. Check pool cache                      │
  │                        ├─ HIT: get Arc<Parse>                │
  │                        └─ MISS: create new Arc<Parse>        │
  │                              └─ intern query string          │
  │                              └─ rewrite name to "DOORMAN_N"  │
  │                              └─ insert into pool cache       │
  │                           │                                  │
  │                     3. Store in client cache:                │
  │                        "my_stmt" → CachedStatement           │
  │                           │                                  │
  │                     4. Check if server has statement         │
  │                        ├─ YES: skip sending Parse            │
  │                        └─ NO: send Parse to server ─────────>│
  │                           │                                  │
  │<── ParseComplete ─────────│<── ParseComplete ────────────────│
```

### Memory Sharing Example

When multiple clients use the same query:

```
Client A: Parse "stmt1" with "SELECT * FROM users WHERE id = $1"
Client B: Parse "my_query" with "SELECT * FROM users WHERE id = $1"
Client C: Parse "" (anonymous) with "SELECT * FROM users WHERE id = $1"

Memory layout:
┌─────────────────────────────────────────────────────────────────┐
│ QUERY_INTERNER                                                   │
│   hash_123 → Arc<str> "SELECT * FROM users WHERE id = $1"       │
│              (reference count: 1)                                │
└─────────────────────────────────────────────────────────────────┘
                              ↑
┌─────────────────────────────────────────────────────────────────┐
│ Pool Cache                                                       │
│   hash_123 → Arc<Parse> { name: "DOORMAN_1", query: ↑ }         │
│              (reference count: 3 - one per client)               │
└─────────────────────────────────────────────────────────────────┘
         ↑              ↑              ↑
┌────────┴───┐  ┌───────┴────┐  ┌──────┴─────┐
│ Client A   │  │ Client B   │  │ Client C   │
│ "stmt1" →  │  │ "my_query" │  │ hash_123 → │
│ Arc<Parse> │  │ → Arc<Parse│  │ Arc<Parse> │
└────────────┘  └────────────┘  └────────────┘
```

**Result:** The query text "SELECT * FROM users WHERE id = $1" exists only once in memory, shared by all clients.

## What Happens During Eviction

When the pool cache is full and a new statement needs to be added:

### Step 1: LRU Eviction from Pool Cache

```
Pool Cache (max_size = 3, full):
  hash_A → Arc<Parse_A>  count_used: 100  ← oldest, will be evicted
  hash_B → Arc<Parse_B>  count_used: 200
  hash_C → Arc<Parse_C>  count_used: 300

New statement arrives (hash_D):
  → Evict hash_A (oldest)
  → Insert hash_D
```

### Step 2: Client References Remain Valid

```
Before eviction:
  Pool Cache: hash_A → Arc<Parse_A> (strong_count = 3)
  Client 1: "stmt1" → Arc<Parse_A>
  Client 2: "query" → Arc<Parse_A>

After eviction:
  Pool Cache: hash_A removed
  Client 1: "stmt1" → Arc<Parse_A> (strong_count = 2) ← still valid!
  Client 2: "query" → Arc<Parse_A>
```

**Important:** Clients continue to work normally because they hold their own `Arc<Parse>` reference.

### Step 3: Query Text Remains Shared

Even after `Arc<Parse>` is evicted from the pool cache, the query text remains shared through the interner:

```
QUERY_INTERNER:
  hash_A → Arc<str> "SELECT..."  (still referenced by Client 1 and 2's Parse)

When Client 3 sends the same query:
  1. Pool cache MISS (hash_A was evicted)
  2. Create new Arc<Parse_A'>
  3. Query interner HIT → reuse existing Arc<str>
  4. Insert into pool cache
```

**Result:** Query text is never duplicated, even when `Arc<Parse>` objects are recreated.

## What Happens During DEALLOCATE

When a client sends `DEALLOCATE` commands, pg_doorman handles them to maintain cache consistency.

### DEALLOCATE \<name\>

Removes a specific prepared statement from the **client-level cache only**.

```
Before DEALLOCATE my_stmt:
  Client cache: "my_stmt" → Arc<Parse_A>
  Pool cache: hash_A → Arc<Parse_A>
  Server cache: "DOORMAN_1" → ()

After DEALLOCATE my_stmt:
  Client cache: (entry removed)
  Pool cache: hash_A → Arc<Parse_A>  ← unchanged
  Server cache: "DOORMAN_1" → ()     ← unchanged
```

**Behavior:**
1. pg_doorman intercepts the `DEALLOCATE <name>` query
2. Removes the entry from the client's `prepared.cache`
3. Sends a synthetic success response to the client
4. The query is **NOT forwarded** to the PostgreSQL server

**Note:** The pool-level cache and server-level cache are not affected. If the same client (or another client) sends the same query again, it will be found in the pool cache and reused.

### DEALLOCATE ALL

Clears the **entire client-level cache** for that client.

```
Before DEALLOCATE ALL:
  Client cache: "stmt1" → Arc<Parse_A>
                "stmt2" → Arc<Parse_B>
                "stmt3" → Arc<Parse_C>
  Pool cache: hash_A, hash_B, hash_C → Arc<Parse>...
  Server cache: "DOORMAN_1", "DOORMAN_2", "DOORMAN_3" → ()

After DEALLOCATE ALL:
  Client cache: (empty)
  Pool cache: hash_A, hash_B, hash_C → Arc<Parse>...  ← unchanged
  Server cache: "DOORMAN_1", "DOORMAN_2", "DOORMAN_3" → ()  ← unchanged
```

**Behavior:**
1. pg_doorman intercepts the `DEALLOCATE ALL` query
2. Clears all entries from the client's `prepared.cache`
3. Sends a synthetic success response to the client
4. The query is **NOT forwarded** to the PostgreSQL server

### Server-Side DEALLOCATE ALL

If `DEALLOCATE ALL` is sent as part of a transaction or through other means that bypasses pg_doorman's interception (e.g., inside a function), the server will execute it. When pg_doorman sees the `CommandComplete` response with "DEALLOCATE ALL":

1. The **server-level cache** (`prepared_statement_cache` on the `Server` struct) is cleared
2. This ensures pg_doorman knows the server no longer has those prepared statements registered

```
After server executes DEALLOCATE ALL:
  Client cache: unchanged (client still thinks statements exist)
  Pool cache: unchanged
  Server cache: (cleared) ← pg_doorman detects this and clears
```

**Important:** In this case, the client cache is NOT automatically cleared. The next time the client tries to use a cached statement, pg_doorman will re-register it on the server.

### Summary Table

| Command | Client Cache | Pool Cache | Server Cache |
|---------|--------------|------------|--------------|
| `DEALLOCATE <name>` (intercepted) | Entry removed | Unchanged | Unchanged |
| `DEALLOCATE ALL` (intercepted) | Cleared | Unchanged | Unchanged |
| `DEALLOCATE ALL` (server-executed) | Unchanged | Unchanged | Cleared |
| `DISCARD ALL` (server-executed) | Unchanged | Unchanged | Cleared |

## Configuration Options

| Option | Default | Description |
|--------|---------|-------------|
| `prepared_statements` | `true` | Enable/disable prepared statement caching |
| `prepared_statements_cache_size` | `8192` | Maximum entries in pool-level cache |
| `client_prepared_statements_cache_size` | `0` | Maximum entries per client (0 = unlimited) |

## Monitoring

### Admin Commands

```sql
-- Show pool-level and client-level cache statistics
SHOW POOLS_MEMORY;

-- Output columns:
-- database, user, pool_prepared_count, pool_prepared_bytes,
-- client_prepared_count, client_prepared_bytes, async_clients

-- Show all cached prepared statements
SHOW PREPARED_STATEMENTS;

-- Output columns:
-- pool, hash, name, query, count_used (monotonic counter for LRU ordering, higher = more recent)
```

### Prometheus Metrics

```
# Pool-level cache
pg_doorman_pool_prepared_cache_entries{user, database}
pg_doorman_pool_prepared_cache_bytes{user, database}

# Client-level cache (aggregated)
pg_doorman_clients_prepared_cache_entries{user, database}
pg_doorman_clients_prepared_cache_bytes{user, database}
pg_doorman_async_clients_count{user, database}
```

## Memory Optimization Tips

1. **Set appropriate `prepared_statements_cache_size`** — Should be larger than the number of unique queries in your application to avoid frequent evictions.

2. **Consider setting `client_prepared_statements_cache_size`** — Protects against clients that create many unique prepared statements without deallocating them.

3. **Monitor `client_prepared_bytes`** — If this grows significantly larger than `pool_prepared_bytes`, clients may be creating too many unique statements.

4. **Check `async_clients` count** — Async clients (using Flush instead of Sync) require unique statement names per client, which increases memory overhead.
