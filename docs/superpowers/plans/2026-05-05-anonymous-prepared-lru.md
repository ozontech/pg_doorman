# Anonymous Prepared LRU Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the per-client prepared statement cache into a Named map (unbounded, never evicted by the pooler) and an Anonymous LRU bounded by a new config knob `client_anonymous_prepared_cache_size` (default 256). Anonymous LRU eviction drops only the local `Arc<Parse>`; nothing is sent to the backend (lazy retirement via server-level LRU and `server_lifetime`). Add `seen_as_named` / `seen_as_anonymous` flags to pool `CacheEntry` to expose a `kind` column (`named` / `anonymous` / `mixed`) in `SHOW PREPARED_STATEMENTS`. Split `SHOW POOLS_MEMORY` and Prometheus to expose Named/Anonymous counts separately. Remove the obsolete `client_prepared_statements_cache_size` field.

**Architecture:** Replace the `PreparedStatementCache` enum in `src/client/core.rs` with a struct holding two independent maps (`named: AHashMap<String, CachedStatement>`, `anonymous: AnonymousCache`), routed by `PreparedStatementKey`. `AnonymousCache` is `Unlimited(AHashMap)` for size 0 or `Limited(LruCache)` for size > 0. Pool-level cache (`src/server/prepared_statement_cache.rs`) is unchanged structurally; we only widen its `register_parse_to_cache` signature with a `client_given_name: &str` parameter so the pool entry can flip its `seen_as_*` flags. Migration reconstruction passes the name from the blob through to that signature; blob format and reconstruction logic stay byte-identical.

**Tech Stack:** Rust 2021 edition, `tokio`, `ahash::AHashMap`, `lru::LruCache`, `dashmap::DashMap`, `bytes::BytesMut`, cucumber-rs for BDD. Tests run via `cargo test --lib` (unit) and `make -C tests test-bdd` (integration).

**Project conventions to follow at every commit:**
- `cargo fmt` (reformat) before commit.
- `cargo clippy -- --deny "warnings"` must pass.
- Commit messages in English, conventional-commit style (`feat:`, `refactor:`, `test:`, `docs:`, ...). No `Co-Authored-By` line.
- Don't push until the user asks; this plan ends with a single PR from `feat/client-cache-anonymous-lru` to `master`.

---

## Task 1: Split `PreparedStatementCache` into Named + Anonymous

**Files:**
- Modify: `src/client/core.rs` (lines 16–305 — types and impls of `PreparedStatementCache`, `PreparedStatementState`, `cache_memory_usage`, `cache_count`)
- Test: `src/client/core.rs` (under `#[cfg(test)] mod tests`, append at end)

This is a structural refactor. We keep the public API surface (`get`, `put`, `pop`, `clear`, `len`, `iter`, `is_empty`) and the existing `CachedStatement` and `PreparedStatementKey` types. Internally the storage splits in two.

- [ ] **Step 1.1: Read current state**

Open `src/client/core.rs`. Confirm:
- `PreparedStatementCache` is an `enum` with `Unlimited(AHashMap<...>)` / `Limited(LruCache<...>)` variants (lines 38–103).
- `PreparedStatementState::new(enabled: bool, max_cache_size: usize)` builds it via `PreparedStatementCache::Limited(...)` if `max_cache_size > 0`, else `Unlimited` (lines 236–256).
- `cache_memory_usage` iterates via `cache.iter()` (lines 273–299).

- [ ] **Step 1.2: Write the failing tests**

Append at the end of `src/client/core.rs`, inside (or after) the existing `#[cfg(test)] mod tests` module:

```rust
#[cfg(test)]
mod cache_split_tests {
    use super::*;
    use std::sync::Arc;

    fn make_cached(name: &str, query: &str) -> CachedStatement {
        let mut buf = bytes::BytesMut::new();
        use bytes::BufMut;
        buf.put_u8(b'P');
        let name_bytes = name.as_bytes();
        let query_bytes = query.as_bytes();
        let len = 4 + name_bytes.len() + 1 + query_bytes.len() + 1 + 2;
        buf.put_i32(len as i32);
        buf.put_slice(name_bytes);
        buf.put_u8(0);
        buf.put_slice(query_bytes);
        buf.put_u8(0);
        buf.put_i16(0);
        let parse: crate::messages::Parse = (&buf).try_into().unwrap();
        CachedStatement {
            parse: Arc::new(parse),
            hash: 0xdead_beef,
            async_name: None,
        }
    }

    #[test]
    fn named_entries_are_never_evicted_under_anon_pressure() {
        // Anonymous LRU size 1 — but Named must persist regardless.
        let mut cache = PreparedStatementCache::new(1);
        let named_key = PreparedStatementKey::Named("stmt_one".into());
        cache.put(named_key.clone(), make_cached("stmt_one", "SELECT 1"));

        for i in 0..5 {
            let h = i as u64;
            cache.put(
                PreparedStatementKey::Anonymous(h),
                make_cached("anon", &format!("SELECT {i}")),
            );
        }

        assert!(cache.get(&named_key).is_some(), "Named entry was evicted");
    }

    #[test]
    fn anonymous_lru_evicts_oldest_when_full() {
        let mut cache = PreparedStatementCache::new(2);
        cache.put(PreparedStatementKey::Anonymous(1), make_cached("a", "Q1"));
        cache.put(PreparedStatementKey::Anonymous(2), make_cached("a", "Q2"));
        let evicted = cache.put(PreparedStatementKey::Anonymous(3), make_cached("a", "Q3"));
        assert!(evicted.is_some(), "Should have evicted entry 1");
        assert!(cache.get(&PreparedStatementKey::Anonymous(1)).is_none());
        assert!(cache.get(&PreparedStatementKey::Anonymous(2)).is_some());
        assert!(cache.get(&PreparedStatementKey::Anonymous(3)).is_some());
    }

    #[test]
    fn anonymous_unlimited_when_size_zero() {
        let mut cache = PreparedStatementCache::new(0);
        for i in 0..1000_u64 {
            let evicted = cache.put(PreparedStatementKey::Anonymous(i), make_cached("a", "Q"));
            assert!(evicted.is_none(), "Unlimited cache must not evict");
        }
        assert_eq!(cache.anonymous_count(), 1000);
    }

    #[test]
    fn pop_routes_by_key_kind() {
        let mut cache = PreparedStatementCache::new(0);
        cache.put(PreparedStatementKey::Named("a".into()), make_cached("a", "Q"));
        cache.put(PreparedStatementKey::Anonymous(1), make_cached("b", "Q"));
        assert!(cache.pop(&PreparedStatementKey::Named("a".into())).is_some());
        assert!(cache.pop(&PreparedStatementKey::Named("a".into())).is_none());
        assert!(cache.pop(&PreparedStatementKey::Anonymous(1)).is_some());
    }

    #[test]
    fn clear_empties_both_maps() {
        let mut cache = PreparedStatementCache::new(0);
        cache.put(PreparedStatementKey::Named("a".into()), make_cached("a", "Q"));
        cache.put(PreparedStatementKey::Anonymous(1), make_cached("b", "Q"));
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.named_count(), 0);
        assert_eq!(cache.anonymous_count(), 0);
    }

    #[test]
    fn iter_yields_both_maps() {
        let mut cache = PreparedStatementCache::new(0);
        cache.put(PreparedStatementKey::Named("a".into()), make_cached("a", "Q"));
        cache.put(PreparedStatementKey::Anonymous(1), make_cached("b", "Q"));
        let kinds: Vec<&str> = cache
            .iter()
            .map(|(k, _)| match k {
                PreparedStatementKey::Named(_) => "named",
                PreparedStatementKey::Anonymous(_) => "anon",
            })
            .collect();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&"named"));
        assert!(kinds.contains(&"anon"));
    }
}
```

- [ ] **Step 1.3: Run tests to confirm they fail**

Run:

```
cargo test --lib client::core::cache_split_tests
```

Expected: compile errors (`new(1)` on enum doesn't accept usize like that, `anonymous_count`/`named_count` don't exist) or runtime failures.

- [ ] **Step 1.4: Replace the enum with a struct + AnonymousCache enum**

In `src/client/core.rs`, replace lines 38–103 (the `pub enum PreparedStatementCache { ... }` and its `impl` block) with:

```rust
/// Per-client prepared statement cache, split into two parts:
///   - `named`: AHashMap of client-provided statement names. Never evicted
///     by the pooler; lifecycle is owned by the client (Close, DEALLOCATE,
///     disconnect).
///   - `anonymous`: LRU keyed by query hash. Bounded by
///     `client_anonymous_prepared_cache_size`. On eviction the local
///     `Arc<Parse>` is dropped; nothing is sent to the backend.
pub struct PreparedStatementCache {
    named: AHashMap<String, CachedStatement>,
    anonymous: AnonymousCache,
}

enum AnonymousCache {
    Unlimited(AHashMap<u64, CachedStatement>),
    Limited(LruCache<u64, CachedStatement>),
}

impl PreparedStatementCache {
    /// `anon_size = 0` selects an unlimited Anonymous map (no LRU).
    pub fn new(anon_size: usize) -> Self {
        let anonymous = if anon_size > 0 {
            AnonymousCache::Limited(LruCache::new(NonZeroUsize::new(anon_size).unwrap()))
        } else {
            AnonymousCache::Unlimited(AHashMap::new())
        };
        Self {
            named: AHashMap::new(),
            anonymous,
        }
    }

    #[inline]
    pub fn get(&mut self, key: &PreparedStatementKey) -> Option<&CachedStatement> {
        match key {
            PreparedStatementKey::Named(s) => self.named.get(s),
            PreparedStatementKey::Anonymous(h) => match &mut self.anonymous {
                AnonymousCache::Unlimited(m) => m.get(h),
                AnonymousCache::Limited(l) => l.get(h),
            },
        }
    }

    /// Insert into the routed map. For Anonymous + Limited, may evict the
    /// oldest entry; the evicted entry is returned for the caller (caller
    /// uses this to bump an evictions counter, otherwise drops it).
    #[inline]
    pub fn put(
        &mut self,
        key: PreparedStatementKey,
        value: CachedStatement,
    ) -> Option<CachedStatement> {
        match key {
            PreparedStatementKey::Named(s) => {
                self.named.insert(s, value);
                None
            }
            PreparedStatementKey::Anonymous(h) => match &mut self.anonymous {
                AnonymousCache::Unlimited(m) => {
                    m.insert(h, value);
                    None
                }
                AnonymousCache::Limited(l) => l.put(h, value).map(|(_, evicted)| evicted),
            },
        }
    }

    #[inline]
    pub fn pop(&mut self, key: &PreparedStatementKey) -> Option<CachedStatement> {
        match key {
            PreparedStatementKey::Named(s) => self.named.remove(s),
            PreparedStatementKey::Anonymous(h) => match &mut self.anonymous {
                AnonymousCache::Unlimited(m) => m.remove(h),
                AnonymousCache::Limited(l) => l.pop(h),
            },
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.named_count() + self.anonymous_count()
    }

    #[inline]
    pub fn named_count(&self) -> usize {
        self.named.len()
    }

    #[inline]
    pub fn anonymous_count(&self) -> usize {
        match &self.anonymous {
            AnonymousCache::Unlimited(m) => m.len(),
            AnonymousCache::Limited(l) => l.len(),
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.named.clear();
        match &mut self.anonymous {
            AnonymousCache::Unlimited(m) => m.clear(),
            AnonymousCache::Limited(l) => l.clear(),
        }
    }

    /// Yields `(synthesised key, value)` for both maps. The Anonymous side
    /// produces `PreparedStatementKey::Anonymous(hash)` keys, the Named
    /// side `PreparedStatementKey::Named(name)`. Order is unspecified.
    pub fn iter(
        &self,
    ) -> Box<dyn Iterator<Item = (PreparedStatementKey, &CachedStatement)> + '_> {
        let named_iter = self
            .named
            .iter()
            .map(|(k, v)| (PreparedStatementKey::Named(k.clone()), v));
        let anon_iter: Box<dyn Iterator<Item = (PreparedStatementKey, &CachedStatement)>> =
            match &self.anonymous {
                AnonymousCache::Unlimited(m) => Box::new(
                    m.iter()
                        .map(|(h, v)| (PreparedStatementKey::Anonymous(*h), v)),
                ),
                AnonymousCache::Limited(l) => Box::new(
                    l.iter()
                        .map(|(h, v)| (PreparedStatementKey::Anonymous(*h), v)),
                ),
            };
        Box::new(named_iter.chain(anon_iter))
    }
}
```

- [ ] **Step 1.5: Update `PreparedStatementState::new` and helpers**

In `src/client/core.rs:236-265`, replace the body of `PreparedStatementState::new` so that `cache` is built via the new constructor:

```rust
impl PreparedStatementState {
    /// `anon_cache_size = 0` => unlimited Anonymous map (no LRU eviction).
    pub fn new(enabled: bool, anon_cache_size: usize) -> Self {
        Self {
            enabled,
            async_client: false,
            cache: PreparedStatementCache::new(anon_cache_size),
            last_anonymous_hash: None,
            skipped_parses: Vec::new(),
            batch_operations: Vec::new(),
            parses_sent_in_batch: 0,
            processed_response_counts: ResponseCounts::default(),
            pending_close_complete: 0,
        }
    }

    #[inline(always)]
    pub fn reset_batch(&mut self) {
        self.parses_sent_in_batch = 0;
        self.skipped_parses.clear();
        self.batch_operations.clear();
        self.processed_response_counts.clear();
    }

    #[inline(always)]
    pub fn cache_count(&self) -> usize {
        self.cache.len()
    }

    #[inline(always)]
    pub fn named_count(&self) -> usize {
        self.cache.named_count()
    }

    #[inline(always)]
    pub fn anonymous_count(&self) -> usize {
        self.cache.anonymous_count()
    }

    /// Calculates approximate memory usage of the client's prepared statement
    /// cache in bytes. Iterates both maps; counts shared `Parse` only when
    /// it is not also held by the pool (`Arc::strong_count == 1`).
    pub fn cache_memory_usage(&self) -> usize {
        let mut total = 0;
        for (key, cached) in self.cache.iter() {
            total += match key {
                PreparedStatementKey::Named(s) => {
                    std::mem::size_of::<PreparedStatementKey>() + s.capacity()
                }
                PreparedStatementKey::Anonymous(_) => std::mem::size_of::<PreparedStatementKey>(),
            };
            total += std::mem::size_of::<CachedStatement>();
            if let Some(name) = &cached.async_name {
                total += name.capacity();
            }
            if std::sync::Arc::strong_count(&cached.parse) == 1 {
                total += cached.parse.memory_usage();
            }
        }
        total
    }
}
```

The signature `new(enabled, anon_cache_size)` keeps the same shape as before (`new(enabled, max_cache_size)`), so call sites in `src/client/startup.rs:425` and `src/client/migration.rs:647` continue to compile — they will be updated in Task 2 to pass the renamed config field.

- [ ] **Step 1.6: Run tests, expect pass**

```
cargo test --lib client::core::cache_split_tests
```

Expected: all six tests pass.

- [ ] **Step 1.7: Run the existing test module too**

```
cargo test --lib client::core
```

Expected: all pre-existing tests still pass. If a pre-existing test referenced the old `PreparedStatementCache::Unlimited(...)` / `Limited(...)` constructors directly, port it to `PreparedStatementCache::new(...)`.

- [ ] **Step 1.8: Compile the whole crate**

```
cargo build
```

Expected: compiles. `iter()` callers may need a tiny fix because the old `iter()` returned `Box<dyn Iterator<Item = (&PreparedStatementKey, &CachedStatement)>>` and the new one returns owned `PreparedStatementKey`. The only consumers are `cache_memory_usage` (already adapted) and the SHOW PREPARED_STATEMENTS path (will be visited in Task 4).

If `cargo build` complains in `src/admin/show.rs` or `src/client/migration.rs` about a borrowed `&PreparedStatementKey` vs owned, fix the callsite to take `key` by value.

- [ ] **Step 1.9: Format and lint**

```
cargo fmt
cargo clippy --lib -- --deny "warnings"
```

Both must succeed.

- [ ] **Step 1.10: Commit**

Use the pre-commit code review per project rule (CLAUDE.md). Then:

```
git add src/client/core.rs
git commit -m "refactor(client/cache): split per-client prepared cache into Named + Anonymous

Replaces the single PreparedStatementCache enum (Unlimited or Limited
LRU on a unified key) with a struct of two independent maps: named (an
unbounded AHashMap on String) and anonymous (Unlimited AHashMap or
Limited LruCache on u64 hash). Routing is done by PreparedStatementKey.

Named entries are now structurally protected from LRU eviction, which
fixes the long-standing bug where a Limited cache could drop a Named
entry without sending Close to the backend, leaving the next Bind to
fail with 'prepared statement does not exist'.

Anonymous LRU eviction returns the evicted CachedStatement; the caller
will use the return value in a follow-up commit to bump a metrics
counter."
```

---

## Task 2: Drop `client_prepared_statements_cache_size`, add `client_anonymous_prepared_cache_size`

**Files:**
- Modify: `src/config/general.rs:208-217, 405-414, 522-525`
- Modify: `src/client/startup.rs:425-428`
- Modify: `src/client/migration.rs:647`

- [ ] **Step 2.1: Read current state**

Confirm current default:
- `default_client_prepared_statements_cache_size()` returns `0` (`src/config/general.rs:412-414`).
- The field is on `General` struct (`src/config/general.rs:216-217`).
- It's referenced in `Default for General` impl (`src/config/general.rs:522-525`).
- Two consumers: `src/client/startup.rs:425-428` and `src/client/migration.rs` (the `cache_size` arg into `reconstruct_prepared_state`).

- [ ] **Step 2.2: Write a failing test for the new default**

Append to `src/config/general.rs` inside `#[cfg(test)] mod tests` (create the module at the end of the file if it doesn't exist):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_prepared_cache_default_is_256() {
        let g = General::default();
        assert_eq!(g.client_anonymous_prepared_cache_size, 256);
    }

    #[test]
    fn old_field_is_silently_ignored_in_yaml() {
        // The field is removed; presence should not fail parsing because
        // General does not use #[serde(deny_unknown_fields)].
        let yaml = r#"
host: "0.0.0.0"
port: 6432
admin_username: "admin"
admin_password: "x"
client_prepared_statements_cache_size: 1024
"#;
        let parsed: serde_yaml::Result<General> = serde_yaml::from_str(yaml);
        assert!(parsed.is_ok(), "should parse with unknown field");
        // The new field gets the default since it wasn't specified.
        assert_eq!(parsed.unwrap().client_anonymous_prepared_cache_size, 256);
    }
}
```

- [ ] **Step 2.3: Run, confirm failure**

```
cargo test --lib config::general::tests
```

Expected: compile error (`client_anonymous_prepared_cache_size` not found).

- [ ] **Step 2.4: Replace the field and default**

In `src/config/general.rs`:

Remove (lines 208-217 — field + `serde(default)` attr):

```rust
    #[serde(default = "General::default_prepared_statements_cache_size")]
    pub prepared_statements_cache_size: usize,

    /// Maximum number of prepared statements cached per client connection.
    /// ...
    #[serde(default = "General::default_client_prepared_statements_cache_size")]
    pub client_prepared_statements_cache_size: usize,
```

Replace with:

```rust
    #[serde(default = "General::default_prepared_statements_cache_size")]
    pub prepared_statements_cache_size: usize,

    /// Per-client Anonymous prepared statement LRU size.
    /// `0` disables the LRU and uses an unlimited map. The Named part of
    /// the per-client cache is always unbounded; only Anonymous entries
    /// participate in LRU eviction.
    #[serde(default = "General::default_client_anonymous_prepared_cache_size")]
    pub client_anonymous_prepared_cache_size: usize,
```

Remove `default_client_prepared_statements_cache_size` (lines 411-414) and add:

```rust
    /// Default per-client Anonymous LRU size.
    pub fn default_client_anonymous_prepared_cache_size() -> usize {
        256
    }
```

In `Default for General` (lines 522-525), replace:

```rust
            client_prepared_statements_cache_size:
                Self::default_client_prepared_statements_cache_size(),
```

with:

```rust
            client_anonymous_prepared_cache_size:
                Self::default_client_anonymous_prepared_cache_size(),
```

- [ ] **Step 2.5: Update consumers**

In `src/client/startup.rs:425-428`, replace:

```rust
            prepared: PreparedStatementState::new(
                prepared_statements_enabled,
                config.general.client_prepared_statements_cache_size,
            ),
```

with:

```rust
            prepared: PreparedStatementState::new(
                prepared_statements_enabled,
                config.general.client_anonymous_prepared_cache_size,
            ),
```

In `src/client/migration.rs`: locate where `reconstruct_prepared_state` is invoked and where `cache_size` is sourced (around line 647 the function already takes `cache_size: usize`). Trace one level up to find the caller that reads from config (`grep -n reconstruct_prepared_state` is the easy way) and replace `client_prepared_statements_cache_size` with `client_anonymous_prepared_cache_size` there too.

Run command to find the call site:

```
grep -n "client_prepared_statements_cache_size" src/
```

Expected: only `src/client/migration.rs` after Step 2.4. Replace with `client_anonymous_prepared_cache_size` and re-grep to confirm zero matches in `src/`.

- [ ] **Step 2.6: Run tests, expect pass**

```
cargo test --lib config::general::tests
```

Expected: both new tests pass.

- [ ] **Step 2.7: Run the full crate test**

```
cargo test --lib
```

Expected: existing tests pass. If any test still references `client_prepared_statements_cache_size`, port it to the new name.

- [ ] **Step 2.8: Compile and lint**

```
cargo build
cargo fmt
cargo clippy --lib -- --deny "warnings"
```

- [ ] **Step 2.9: Update generated reference config**

Run:

```
make generate
```

This regenerates the reference config files (`pg_doorman.yaml`, `pg_doorman.toml`) and the generated reference docs under `documentation/en/src/reference/`. Inspect the diff: the old field disappears, the new one appears with default `256`. Stage the regenerated files in this commit.

- [ ] **Step 2.10: Commit**

Pre-commit code review, then:

```
git add src/config/general.rs src/client/startup.rs src/client/migration.rs pg_doorman.yaml pg_doorman.toml documentation/en/src/reference/
git commit -m "feat(config): replace client_prepared_statements_cache_size with client_anonymous_prepared_cache_size

Removes the obsolete per-client cache-size knob whose Limited mode
silently evicted Named entries and broke clients on next Bind.
Introduces client_anonymous_prepared_cache_size (default 256) which
applies only to the Anonymous part of the per-client cache; Named is
always unbounded.

Old configs that still mention the removed field continue to parse
because General does not use deny_unknown_fields; the value is
silently ignored. Operators tuning the old field need to migrate."
```

---

## Task 3: Add `seen_as_named` / `seen_as_anonymous` flags to pool `CacheEntry`

**Files:**
- Modify: `src/server/prepared_statement_cache.rs:31-35` (CacheEntry), `:80-110` (`get_or_insert`), `:133-139` (`get_entries`)
- Modify: `src/pool/mod.rs:783-794` (`register_parse_to_cache` signature)
- Modify: `src/client/protocol.rs:164` (call site, has client_given_name in scope)
- Modify: `src/client/migration.rs:656` (call site, derive name from blob entry's key kind)

- [ ] **Step 3.1: Add a Kind enum next to CacheEntry**

In `src/server/prepared_statement_cache.rs`, just below the `CacheEntry` struct (around line 35), add:

```rust
/// Classification of how clients have referenced a pool cache entry over
/// its lifetime. Flags only ever flip from false to true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheEntryKind {
    Named,
    Anonymous,
    Mixed,
}

impl CacheEntryKind {
    fn from_flags(named: bool, anonymous: bool) -> Self {
        match (named, anonymous) {
            (true, true) => CacheEntryKind::Mixed,
            (true, false) => CacheEntryKind::Named,
            (false, true) => CacheEntryKind::Anonymous,
            (false, false) => CacheEntryKind::Anonymous, // unreachable in practice
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CacheEntryKind::Named => "named",
            CacheEntryKind::Anonymous => "anonymous",
            CacheEntryKind::Mixed => "mixed",
        }
    }
}
```

- [ ] **Step 3.2: Extend CacheEntry with two AtomicBool flags**

In `src/server/prepared_statement_cache.rs:31-35`, replace:

```rust
struct CacheEntry {
    parse: Arc<Parse>,
    /// Counter for LRU ordering (higher = more recently used)
    count_used: u64,
}
```

with:

```rust
struct CacheEntry {
    parse: Arc<Parse>,
    /// Counter for LRU ordering (higher = more recently used)
    count_used: u64,
    /// Has at least one client ever Parse'd this hash with a non-empty name?
    seen_as_named: AtomicBool,
    /// Has at least one client ever Parse'd this hash with an empty name?
    seen_as_anonymous: AtomicBool,
}
```

Add `use std::sync::atomic::AtomicBool;` to the imports at the top of the file (it already imports `AtomicU64`; extend that line: `use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};`).

- [ ] **Step 3.3: Update `get_or_insert` to take `client_given_name` and flip flags**

Replace `get_or_insert` (lines 80-109) with:

```rust
pub fn get_or_insert(
    &self,
    parse: &Parse,
    hash: u64,
    client_given_name: &str,
) -> Arc<Parse> {
    let timestamp = self.counter.fetch_add(1, Ordering::Relaxed);
    let is_anonymous = client_given_name.is_empty();

    if let Some(entry) = self.cache.get(&hash) {
        // Bump LRU and update kind flags via atomic stores.
        // We need a mutable reference to update count_used, so use get_mut.
        drop(entry);
        if let Some(mut entry) = self.cache.get_mut(&hash) {
            entry.count_used = timestamp;
            if is_anonymous {
                entry.seen_as_anonymous.store(true, Ordering::Relaxed);
            } else {
                entry.seen_as_named.store(true, Ordering::Relaxed);
            }
            return entry.parse.clone();
        }
    }

    let new_parse = Arc::new(parse.clone().intern_query(hash).rewrite());

    self.cache.insert(
        hash,
        CacheEntry {
            parse: new_parse.clone(),
            count_used: timestamp,
            seen_as_named: AtomicBool::new(!is_anonymous),
            seen_as_anonymous: AtomicBool::new(is_anonymous),
        },
    );

    while self.cache.len() > self.max_size {
        self.evict_oldest();
    }

    new_parse
}
```

- [ ] **Step 3.4: Update `get_entries` to expose Kind**

Replace `get_entries` (lines 133-139) with:

```rust
/// Returns a list of all entries in the cache, including the LRU
/// timestamp and the classification derived from the seen_as_* flags.
pub fn get_entries(&self) -> Vec<(u64, Arc<Parse>, u64, CacheEntryKind)> {
    self.cache
        .iter()
        .map(|entry| {
            let kind = CacheEntryKind::from_flags(
                entry.seen_as_named.load(Ordering::Relaxed),
                entry.seen_as_anonymous.load(Ordering::Relaxed),
            );
            (*entry.key(), entry.parse.clone(), entry.count_used, kind)
        })
        .collect()
}
```

- [ ] **Step 3.5: Widen `register_parse_to_cache` signature**

In `src/pool/mod.rs:788`, replace:

```rust
pub fn register_parse_to_cache(&self, hash: u64, parse: &Parse) -> Option<Arc<Parse>> {
    self.prepared_statement_cache
        .as_ref()
        .map(|cache| cache.get_or_insert(parse, hash))
}
```

with:

```rust
pub fn register_parse_to_cache(
    &self,
    hash: u64,
    parse: &Parse,
    client_given_name: &str,
) -> Option<Arc<Parse>> {
    self.prepared_statement_cache
        .as_ref()
        .map(|cache| cache.get_or_insert(parse, hash, client_given_name))
}
```

- [ ] **Step 3.6: Update call site in `protocol.rs`**

In `src/client/protocol.rs:164`, replace:

```rust
let shared_parse = match pool.register_parse_to_cache(hash, &parse) {
```

with:

```rust
let shared_parse = match pool.register_parse_to_cache(hash, &parse, &client_given_name) {
```

`client_given_name` is already in scope (declared at line 157 in the same function).

- [ ] **Step 3.7: Update call site in `migration.rs`**

In `src/client/migration.rs:656`, the loop iterates `entries` where each entry has the original key kind. Locate the field carrying it (look near the deserialised entry struct — likely `entry.client_name: Option<String>` or distinguished via `entry.key`). Replace:

```rust
let Some(shared_parse) = pool.register_parse_to_cache(hash, &parse) else {
```

with:

```rust
let client_given_name: &str = entry
    .client_given_name
    .as_deref()
    .unwrap_or("");
let Some(shared_parse) = pool.register_parse_to_cache(hash, &parse, client_given_name) else {
```

If the deserialised struct uses a different field name, adapt accordingly (common alternatives: `entry.name`, `entry.client_name`). The blob already serialises the name for `Named` entries and an empty marker for `Anonymous`; this commit just threads it through.

- [ ] **Step 3.8: Add unit tests for the flags**

In `src/server/prepared_statement_cache.rs`, inside the existing `#[cfg(test)] mod tests` (the helper `make_parse` is already there), append:

```rust
#[test]
fn flags_named_only_on_named_register() {
    let cache = PreparedStatementCache::new(8, 1);
    let parse = make_parse("stmt_one", "SELECT 1");
    cache.get_or_insert(&parse, 1, "stmt_one");
    let entries = cache.get_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].3, CacheEntryKind::Named);
}

#[test]
fn flags_anonymous_only_on_anonymous_register() {
    let cache = PreparedStatementCache::new(8, 1);
    let parse = make_parse("", "SELECT 1");
    cache.get_or_insert(&parse, 1, "");
    let entries = cache.get_entries();
    assert_eq!(entries[0].3, CacheEntryKind::Anonymous);
}

#[test]
fn flags_mixed_when_both_seen() {
    let cache = PreparedStatementCache::new(8, 1);
    let p1 = make_parse("stmt_one", "SELECT 1");
    cache.get_or_insert(&p1, 1, "stmt_one");
    let p2 = make_parse("", "SELECT 1");
    cache.get_or_insert(&p2, 1, "");
    let entries = cache.get_entries();
    assert_eq!(entries[0].3, CacheEntryKind::Mixed);
}
```

If `PreparedStatementCache::new` constructor in this file takes a different signature (e.g. `(max_size: usize, worker_threads: usize)`), match it; the values above are illustrative.

- [ ] **Step 3.9: Run unit tests**

```
cargo test --lib server::prepared_statement_cache::tests
```

Expected: all three new tests pass plus existing.

- [ ] **Step 3.10: Compile, lint, format**

```
cargo build
cargo fmt
cargo clippy --lib -- --deny "warnings"
```

If clippy nags about `drop(entry)` followed by `get_mut`, simplify by using only `get_mut` in `get_or_insert` (DashMap allows it, with a lock upgrade). Alternative implementation:

```rust
if let Some(mut entry) = self.cache.get_mut(&hash) {
    entry.count_used = timestamp;
    if is_anonymous {
        entry.seen_as_anonymous.store(true, Ordering::Relaxed);
    } else {
        entry.seen_as_named.store(true, Ordering::Relaxed);
    }
    return entry.parse.clone();
}
```

(no preceding `cache.get` and no `drop`). Use this form if the early-return version above triggers a borrow conflict.

- [ ] **Step 3.11: Commit**

```
git add src/server/prepared_statement_cache.rs src/pool/mod.rs src/client/protocol.rs src/client/migration.rs
git commit -m "feat(pool/cache): track named/anonymous usage on each pool entry

Adds seen_as_named and seen_as_anonymous AtomicBool flags to
CacheEntry; flipped from false to true on each register_parse_to_cache
based on whether the client used a non-empty Parse name. Widens
register_parse_to_cache to take the client-given name through to the
pool cache.

Migration call site forwards the name from the deserialised blob, so
the seen_as_* flags are restored to a correct value across binary
upgrade.

get_entries now also returns the derived Kind (Named, Anonymous,
Mixed); this will surface as a column in SHOW PREPARED_STATEMENTS in
the next commit."
```

---

## Task 4: Add `kind` column to `SHOW PREPARED_STATEMENTS`

**Files:**
- Modify: `src/admin/show.rs:159-194` (`show_prepared_statements`)

- [ ] **Step 4.1: Update column list and row emission**

In `src/admin/show.rs:159-194`, replace the body of `show_prepared_statements` with:

```rust
pub async fn show_prepared_statements<T>(stream: &mut T) -> Result<(), Error>
where
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    let columns = vec![
        ("pool", DataType::Text),
        ("hash", DataType::Numeric),
        ("name", DataType::Text),
        ("kind", DataType::Text),
        ("query", DataType::Text),
        ("count_used", DataType::Numeric),
    ];
    let mut res = BytesMut::new();
    res.put(row_description(&columns));

    for (identifier, pool) in get_all_pools().iter() {
        if let Some(cache) = pool.prepared_statement_cache.as_ref() {
            let entries = cache.get_entries();
            for (hash, parse, last_used, kind) in entries {
                res.put(data_row(&[
                    identifier.to_string(),
                    hash.to_string(),
                    parse.name.clone(),
                    kind.as_str().to_string(),
                    parse.query().to_string(),
                    last_used.to_string(),
                ]));
            }
        }
    }

    res.put(command_complete("SHOW"));
    res.put_u8(b'Z');
    res.put_i32(5);
    res.put_u8(b'I');
    write_all_half(stream, &res).await
}
```

If `CacheEntryKind` is not yet re-exported, add to the imports at the top of `src/admin/show.rs`:

```rust
use crate::server::prepared_statement_cache::CacheEntryKind;
```

(or `use crate::server::CacheEntryKind` depending on the actual `mod` layout). Verify by trying to compile.

- [ ] **Step 4.2: Compile**

```
cargo build
```

Expected: clean build. If `CacheEntryKind` is not pub-exported from its module, add `pub use prepared_statement_cache::CacheEntryKind;` in `src/server/mod.rs` next to the `PreparedStatementCache` re-export.

- [ ] **Step 4.3: Lint and format**

```
cargo fmt
cargo clippy --lib -- --deny "warnings"
```

- [ ] **Step 4.4: Commit**

```
git add src/admin/show.rs src/server/mod.rs
git commit -m "feat(admin): SHOW PREPARED_STATEMENTS exposes named/anonymous/mixed kind

Adds a 'kind' column derived from the per-entry seen_as_named and
seen_as_anonymous flags introduced in the previous commit. Operators
can now see at a glance whether a hot DOORMAN_<N> entry is owned by
named clients, anonymous traffic, or both."
```

---

## Task 5: Add `client_named_count` / `client_anonymous_count` columns to `SHOW POOLS_MEMORY` and Prometheus

**Files:**
- Modify: `src/stats/pool.rs:149-152, 254-255, 392-411, 606-607` (PoolStats fields, default, header, row, fill loop)
- Modify: `src/client/core.rs` (already has `named_count`/`anonymous_count` from Task 1)

- [ ] **Step 5.1: Extend `PoolStats` fields**

In `src/stats/pool.rs`, after line 152 (where `client_prepared_bytes` ends), add:

```rust
    pub client_named_count: u64,
    pub client_anonymous_count: u64,
```

In the `Default` impl around line 254-255, append (after `client_prepared_bytes: 0,`):

```rust
            client_named_count: 0,
            client_anonymous_count: 0,
```

- [ ] **Step 5.2: Extend `generate_show_pools_memory_header`**

Around line 392, locate:

```rust
("client_prepared_count", DataType::Numeric),
("client_prepared_bytes", DataType::Numeric),
```

Replace with:

```rust
("client_prepared_count", DataType::Numeric),
("client_prepared_bytes", DataType::Numeric),
("client_named_count", DataType::Numeric),
("client_anonymous_count", DataType::Numeric),
```

- [ ] **Step 5.3: Extend `generate_show_pools_memory_row`**

Around line 410, locate:

```rust
Cow::Owned(self.client_prepared_count.to_string()),
Cow::Owned(self.client_prepared_bytes.to_string()),
```

Replace with:

```rust
Cow::Owned(self.client_prepared_count.to_string()),
Cow::Owned(self.client_prepared_bytes.to_string()),
Cow::Owned(self.client_named_count.to_string()),
Cow::Owned(self.client_anonymous_count.to_string()),
```

- [ ] **Step 5.4: Extend the fill loop**

Around line 606-607:

```rust
pool_stats.client_prepared_count += client.prepared_cache_count();
pool_stats.client_prepared_bytes += client.prepared_cache_bytes();
```

After these two lines, append:

```rust
pool_stats.client_named_count += client.prepared_named_count();
pool_stats.client_anonymous_count += client.prepared_anonymous_count();
```

- [ ] **Step 5.5: Add the helpers on the Client side**

`Client` exposes `prepared_cache_count()` and `prepared_cache_bytes()` already (forwarded to `PreparedStatementState`). Find their definitions (`grep -n "fn prepared_cache_count" src/client/`) and add right next to them:

```rust
#[inline(always)]
pub fn prepared_named_count(&self) -> u64 {
    self.prepared.named_count() as u64
}

#[inline(always)]
pub fn prepared_anonymous_count(&self) -> u64 {
    self.prepared.anonymous_count() as u64
}
```

- [ ] **Step 5.6: Compile**

```
cargo build
```

- [ ] **Step 5.7: Lint, format**

```
cargo fmt
cargo clippy --lib -- --deny "warnings"
```

- [ ] **Step 5.8: Commit**

```
git add src/stats/pool.rs src/client/
git commit -m "feat(admin): SHOW POOLS_MEMORY breaks down client cache by kind

Adds client_named_count and client_anonymous_count columns to the
existing memory view; the existing client_prepared_count stays as
the sum (backward compatible). Gives operators direct visibility
into which side of the per-client cache is consuming memory."
```

---

## Task 6: Prometheus metrics for the split

**Files:**
- Modify: `src/prometheus/metrics.rs`
- Modify: `src/stats/pool.rs` (add `client_anonymous_evictions` counter field)
- Modify: `src/client/protocol.rs` (bump the eviction counter)

- [ ] **Step 6.1: Add the eviction counter to `PoolStats`**

In `src/stats/pool.rs`, next to the new `client_anonymous_count`, add:

```rust
pub client_anonymous_evictions: u64,
```

And in `Default` impl:

```rust
client_anonymous_evictions: 0,
```

This is a monotonic counter; we increment it via the same fill loop, but the source has to be a per-client counter (the client owns the eviction event).

- [ ] **Step 6.2: Add a per-client eviction counter**

In `src/client/core.rs`, inside `PreparedStatementState`:

```rust
pub anonymous_evictions: u64,
```

In `PreparedStatementState::new`:

```rust
anonymous_evictions: 0,
```

Add accessor:

```rust
#[inline(always)]
pub fn anonymous_evictions(&self) -> u64 {
    self.anonymous_evictions
}
```

- [ ] **Step 6.3: Wire eviction counting at the call site**

In `src/client/protocol.rs:219`, the line is currently:

```rust
self.prepared.cache.put(cache_key, cached);
```

Replace with:

```rust
if let Some(_evicted) = self.prepared.cache.put(cache_key, cached) {
    self.prepared.anonymous_evictions += 1;
}
```

(`Named(_)` always returns `None` from `put`, so this only ever counts anonymous evictions.)

- [ ] **Step 6.4: Forward to PoolStats**

In `src/stats/pool.rs`, in the same fill loop where the count helpers are summed, append:

```rust
pool_stats.client_anonymous_evictions += client.prepared_anonymous_evictions();
```

And add the matching helper on `Client`:

```rust
#[inline(always)]
pub fn prepared_anonymous_evictions(&self) -> u64 {
    self.prepared.anonymous_evictions()
}
```

- [ ] **Step 6.5: Add the new gauges and counter to Prometheus exporter**

In `src/prometheus/metrics.rs`, around the existing `client_prepared_count` / `client_prepared_bytes` gauges (line 121-124), add:

```rust
PG_DOORMAN_CLIENTS_PREPARED_NAMED_ENTRIES
    .with_label_values(&[&stats.username, &stats.database])
    .set(stats.client_named_count as f64);
PG_DOORMAN_CLIENTS_PREPARED_ANONYMOUS_ENTRIES
    .with_label_values(&[&stats.username, &stats.database])
    .set(stats.client_anonymous_count as f64);
PG_DOORMAN_CLIENTS_PREPARED_ANONYMOUS_EVICTIONS_TOTAL
    .with_label_values(&[&stats.username, &stats.database])
    .reset();
PG_DOORMAN_CLIENTS_PREPARED_ANONYMOUS_EVICTIONS_TOTAL
    .with_label_values(&[&stats.username, &stats.database])
    .inc_by(stats.client_anonymous_evictions as f64);
```

(For a true monotonic counter, prefer the prometheus IntCounterVec and `.inc_by()` of the delta since last flush. The exact pattern depends on how other counters in this file are wired; mirror the closest-existing example and use the same pattern.)

At the top of `src/prometheus/metrics.rs`, register the three new metrics with `lazy_static!` blocks alongside the existing ones:

```rust
lazy_static! {
    pub static ref PG_DOORMAN_CLIENTS_PREPARED_NAMED_ENTRIES: GaugeVec = register_gauge_vec!(
        "pg_doorman_clients_prepared_named_entries",
        "Per-client Named prepared statement entries",
        &["user", "database"]
    )
    .unwrap();
    pub static ref PG_DOORMAN_CLIENTS_PREPARED_ANONYMOUS_ENTRIES: GaugeVec = register_gauge_vec!(
        "pg_doorman_clients_prepared_anonymous_entries",
        "Per-client Anonymous prepared statement entries",
        &["user", "database"]
    )
    .unwrap();
    pub static ref PG_DOORMAN_CLIENTS_PREPARED_ANONYMOUS_EVICTIONS_TOTAL: IntCounterVec =
        register_int_counter_vec!(
            "pg_doorman_clients_prepared_anonymous_evictions_total",
            "Cumulative count of anonymous LRU evictions on the per-client cache",
            &["user", "database"]
        )
        .unwrap();
}
```

If the file uses a different macro layer or uses `Opts::new()` style, mirror the closest-existing metric.

- [ ] **Step 6.6: Compile, format, lint**

```
cargo build
cargo fmt
cargo clippy --lib -- --deny "warnings"
```

- [ ] **Step 6.7: Manual probe (no full test runtime change)**

Run the binary briefly with a small config and `curl` `/metrics`:

```
cargo run --bin pg_doorman -- -c examples/local.yaml &
sleep 2
curl -s http://localhost:9090/metrics | grep -E "(named|anonymous|evictions)"
kill %1
```

Expected: the three new metric names appear. Values are 0 on a fresh start.

- [ ] **Step 6.8: Commit**

```
git add src/prometheus/metrics.rs src/stats/pool.rs src/client/core.rs src/client/protocol.rs
git commit -m "feat(metrics): expose per-client named/anonymous gauges and eviction counter

Adds three new Prometheus series:
  - pg_doorman_clients_prepared_named_entries
  - pg_doorman_clients_prepared_anonymous_entries
  - pg_doorman_clients_prepared_anonymous_evictions_total

Existing pg_doorman_clients_prepared_cache_entries / _bytes stay as
sums (backward compatible). The eviction counter is the primary
signal for tuning client_anonymous_prepared_cache_size: a sustained
non-zero rate means the limit is too small for the workload."
```

---

## Task 7: BDD scenarios

**Files:**
- Create: `tests/bdd/features/anonymous-prepared-lru.feature`

The existing `tests/bdd/features/anonymous-caching.feature` is the closest cousin and is the right template to copy patterns from.

- [ ] **Step 7.1: Read the closest existing feature**

```
sed -n '1,80p' tests/bdd/features/anonymous-caching.feature
```

Note: tag conventions (`@cache`, `@anonymous`, `@admin`), Gherkin steps for "Given pg_doorman is running with config", `And client A connects ...`, `When client A executes Parse(...)`. Reuse these phrasings.

- [ ] **Step 7.2: Create the feature file**

`tests/bdd/features/anonymous-prepared-lru.feature`:

```gherkin
@cache @anonymous @lru
Feature: Per-client Anonymous LRU keeps Named entries safe

  Background:
    Given pg_doorman is running with config:
      """
      general:
        client_anonymous_prepared_cache_size: 2
      """

  Scenario: Named statement survives anonymous LRU pressure
    Given a client connection
    When the client executes Parse with name "stmt_keep" for query "SELECT 1"
    And the client executes Parse with empty name for query "SELECT 11"
    And the client executes Parse with empty name for query "SELECT 22"
    And the client executes Parse with empty name for query "SELECT 33"
    And the client Binds and Executes statement "stmt_keep"
    Then the response is successful with one row

  Scenario: Anonymous LRU eviction does not break a sibling client sharing the same hash
    Given two client connections A and B
    When client A executes Parse with empty name for query "SELECT 'shared'"
    And client B executes Parse with empty name for query "SELECT 'shared'"
    And client A executes Parse with empty name for query "SELECT 'one'"
    And client A executes Parse with empty name for query "SELECT 'two'"
    Then client A's anonymous cache eviction counter is at least 1
    When client B Binds and Executes the previous shared anonymous statement
    Then client B's response is successful

  Scenario: SHOW PREPARED_STATEMENTS classifies entries by kind
    Given a client connection
    When the client executes Parse with name "stmt_named" for query "SELECT 'A'"
    And the client executes Parse with empty name for query "SELECT 'B'"
    And admin runs "SHOW PREPARED_STATEMENTS"
    Then the result has a row with name "DOORMAN_*" matching query "SELECT 'A'" with kind "named"
    And the result has a row matching query "SELECT 'B'" with kind "anonymous"

  Scenario: SHOW POOLS_MEMORY breaks down client cache by kind
    Given a client connection
    When the client executes Parse with name "stmt_one" for query "SELECT 1"
    And the client executes Parse with empty name for query "SELECT 2"
    And admin runs "SHOW POOLS_MEMORY"
    Then the row has client_named_count >= 1
    And the row has client_anonymous_count >= 1
```

- [ ] **Step 7.3: Find or implement step definitions**

Run:

```
grep -rn "executes Parse with name" tests/bdd/steps/
grep -rn "executes Parse with empty name" tests/bdd/steps/
grep -rn "anonymous cache eviction counter" tests/bdd/steps/
grep -rn "SHOW PREPARED_STATEMENTS" tests/bdd/steps/
```

For any step that has no match, write a step definition in the appropriate `tests/bdd/steps/*.rs` file. The patterns:

- `Parse with empty name` and `Parse with name "X"` likely map to existing `extended-protocol` steps. Reuse.
- `anonymous cache eviction counter` is new — implement by reading
  `pg_doorman_clients_prepared_anonymous_evictions_total{user=…,database=…}` from `/metrics`.
- `kind` matchers in SHOW PREPARED_STATEMENTS — parse the SHOW result table and look up the column.

If existing test step framework already exposes `world.admin_query("SHOW PREPARED_STATEMENTS")` and a row map, write a small assertion helper next to the cucumber `When/Then` macros.

- [ ] **Step 7.4: Run the BDD suite**

```
make -C tests test-bdd TAGS="@anonymous and @lru"
```

Expected: the four new scenarios pass. Iterate on step definitions if any fail.

- [ ] **Step 7.5: Commit**

```
git add tests/bdd/features/anonymous-prepared-lru.feature tests/bdd/steps/
git commit -m "test(bdd): cover anonymous LRU eviction and SHOW kind/breakdown

Adds four scenarios:
  - Named entry survives despite anon LRU evicting many entries.
  - Anonymous eviction in one client does not break a sibling client
    sharing the same query hash.
  - SHOW PREPARED_STATEMENTS classifies entries as named, anonymous,
    or mixed.
  - SHOW POOLS_MEMORY exposes client_named_count and
    client_anonymous_count columns."
```

---

## Task 8: Documentation and changelog

**Files:**
- Modify: `documentation/en/src/tutorials/prepared-statements.md`
- Modify: `documentation/ru/src/tutorials/prepared-statements.md`
- Modify: `documentation/en/src/reference/general.md` (RU is symlinked, no separate edit)
- Modify: `documentation/en/src/changelog.md` (RU is symlink)

- [ ] **Step 8.1: Update EN tutorial — Cache layers section**

In `documentation/en/src/tutorials/prepared-statements.md`, find the "Cache layers" section and replace the per-client paragraph to reflect the split. Keep the existing diagram intact except for the per-client entry, which becomes:

```text
  Client-level  Named:     AHashMap<String, CachedStatement>, unbounded.
                Anonymous: LruCache<u64, CachedStatement> bounded by
                           client_anonymous_prepared_cache_size (default 256),
                           or AHashMap if size = 0.
                Eviction of an Anonymous entry is local: the Arc<Parse> is
                dropped, the underlying DOORMAN_<N> on the backend stays.
```

Replace the configuration table to use the new field name; remove the row for the obsolete one. Add a note that Named is always unlimited.

- [ ] **Step 8.2: Mirror in RU tutorial**

Apply the same edits to `documentation/ru/src/tutorials/prepared-statements.md`:

```text
  Client-level  Named:     AHashMap<String, CachedStatement>, без лимита.
                Anonymous: LruCache<u64, CachedStatement> ограничен
                           client_anonymous_prepared_cache_size (default 256),
                           или AHashMap при размере 0.
                Выселение Anonymous локальное: Arc<Parse> дропается,
                DOORMAN_<N> на бекенде остаётся.
```

Update the Russian configuration table identically.

- [ ] **Step 8.3: Update general reference**

In `documentation/en/src/reference/general.md`, find the entry for `client_prepared_statements_cache_size` (it was generated by `make generate` in Task 2 — likely already the new field). If the regenerated content includes the new field with the correct default, just verify and add a hand-written paragraph explaining the Named/Anonymous split. Example:

```markdown
### `client_anonymous_prepared_cache_size`

Per-client Anonymous prepared-statement LRU size. Default `256`.
Set to `0` to disable LRU (the per-client Anonymous cache becomes
unbounded). The Named portion of the per-client cache is always
unbounded; this knob only constrains Anonymous entries.

When the limit is reached, the oldest Anonymous entry is dropped
locally. No `Close` is sent to the backend; the underlying
`DOORMAN_<N>` is retired by server-level LRU or `server_lifetime`
(whichever fires first).
```

The Russian reference is a symlink; no separate edit.

- [ ] **Step 8.4: Update changelog**

In `documentation/en/src/changelog.md`, add an entry:

```markdown
## Unreleased

### Added
- `client_anonymous_prepared_cache_size` (default 256): bounds the
  Anonymous part of the per-client prepared statement cache. Named
  statements are always unbounded.
- `kind` column in `SHOW PREPARED_STATEMENTS` (named / anonymous /
  mixed) reflects how clients have used each pool entry.
- `client_named_count` and `client_anonymous_count` columns in
  `SHOW POOLS_MEMORY`.
- New Prometheus metrics:
  - `pg_doorman_clients_prepared_named_entries`
  - `pg_doorman_clients_prepared_anonymous_entries`
  - `pg_doorman_clients_prepared_anonymous_evictions_total`

### Changed
- The per-client prepared statement cache is split into two maps:
  Named (unbounded) and Anonymous (LRU). This fixes a long-standing
  bug where the previous Limited cache could evict Named entries and
  cause subsequent Bind to fail with `prepared statement does not
  exist`.

### Removed
- `client_prepared_statements_cache_size` is removed. The setting is
  silently ignored if still present in user configs (the new field
  takes its place). Operators tuning that value should migrate to
  `client_anonymous_prepared_cache_size`.
```

- [ ] **Step 8.5: Build the docs**

```
cd documentation && bash build.sh
```

Expected: clean build, both EN and RU rendered. Open `book/tutorials/prepared-statements.html` in a browser briefly to sanity-check formatting.

- [ ] **Step 8.6: Commit**

```
git add documentation/
git commit -m "docs: split client cache documentation, document new config field

Updates the Anonymous Prepared Statement Caching tutorial to describe
the Named/Anonymous split on the per-client cache and the lazy
eviction policy. Adds an entry for client_anonymous_prepared_cache_size
to the general reference. Records the change in the changelog,
including the removal of client_prepared_statements_cache_size."
```

---

## Final cleanup

- [ ] **Step F.1: Verify the full test suite**

```
cargo test --lib
make -C tests test-bdd TAGS="@cache or @anonymous"
```

Both suites pass.

- [ ] **Step F.2: Format and lint over the entire crate**

```
cargo fmt
cargo clippy -- --deny "warnings"
```

- [ ] **Step F.3: Confirm changelog and docs reflect the final state**

Open the rendered HTML one more time, eyeball the tutorial section.

- [ ] **Step F.4: Push the branch and open a PR**

```
git push -u origin feat/client-cache-anonymous-lru
gh pr create --base master \
  --title "feat: split per-client prepared cache into Named + Anonymous LRU" \
  --body-file docs/superpowers/specs/2026-05-05-anonymous-prepared-lru-design.md
```

(Or paste a hand-written summary instead of the full spec; the spec lives in `docs/superpowers/specs/` for reviewers to read in detail.)

---

## Self-Review

The plan covers every section of the spec:

- **Architecture (struct + AnonymousCache enum):** Task 1.
- **Configuration (new field, removed old):** Task 2.
- **`seen_as_*` flags + signature widening:** Task 3.
- **`kind` column in SHOW PREPARED_STATEMENTS:** Task 4.
- **SHOW POOLS_MEMORY columns:** Task 5.
- **Prometheus metrics (named, anonymous, evictions counter):** Task 6.
- **BDD scenarios:** Task 7.
- **Tutorial / reference / changelog:** Task 8.
- **Migration (compile-only call site update):** integrated into Task 3, Step 3.7.

No placeholders. Type names match across tasks (`PreparedStatementCache`, `AnonymousCache`, `CachedStatement`, `PreparedStatementKey`, `CacheEntry`, `CacheEntryKind`). Method names consistent (`get`, `put`, `pop`, `clear`, `len`, `iter`, `named_count`, `anonymous_count`, `register_parse_to_cache`).

One open dependency between tasks: Task 6 needs `client.prepared_anonymous_evictions()`, which is added in Step 6.4 in the same task — no forward reference.
