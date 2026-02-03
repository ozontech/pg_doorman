# Pool Implementation

## Overview

Connection pool manages PostgreSQL connections with:
- Bounded concurrency via semaphore
- Connection reuse through queue (LIFO/FIFO)
- Gradual scaling with cooldown to avoid connection spikes

## Connection Acquisition Flow

```
pool.get()
  ↓
1. Acquire semaphore permit (limit concurrent operations)
  ↓
2. Try pop_front() - HOT PATH
   └─ If available → recycle → return (fast!)
  ↓
3. If queue empty → check cooldown zone
   └─ size < warm_threshold (20%) → create immediately
   └─ size >= warm_threshold → try wait
  ↓
4. Cooldown retries (if in zone):
   - Phase 1: 10 fast retries with yield_now (~100-500μs)
   - Phase 2: sleep 10ms, try again
  ↓
5. Create new connection
```

## Gradual Scaling

```rust
ScalingConfig {
    warm_pool_ratio: 0.2,      // 0-20% of max_size: instant creation
    fast_retries: 10,          // Retry count with yield
    cooldown_sleep_ms: 10,     // Sleep before final retry
}
```

**Example (max_size=40):**
- Connections 0-8: create immediately (warm)
- Connections 9-40: try wait before creating (cooldown)

## Performance

Production benchmark (pool_size=40, 1000 concurrent):
- **Throughput:** 2.14M ops/sec (+0.3% vs without scaling)
- **Latency p50:** 833ns (+25%)
- **Hot path:** near-zero overhead

## Components

- **Pool** - Cloneable handle (Arc internally)
- **Object** - RAII wrapper, returns connection on drop
- **Slots** - Mutex-protected VecDeque of connections
- **ScalingConfig** - Cooldown behavior configuration

## Queue Modes

- **LIFO** (default): reuse hot connections, better cache locality
- **FIFO**: fair distribution, even wear

## Recycling

Every `pool.get()` recycles the connection:
- Validates connection alive
- Cleans state (rollback transactions, etc.)
- Updates metrics

If recycle fails → connection removed from pool, size decremented.
