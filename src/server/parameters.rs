use bytes::{BufMut, BytesMut};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};

use crate::config::VERSION;

static TRACKED_PARAMETERS: Lazy<HashSet<String>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("client_encoding".to_string());
    set.insert("DateStyle".to_string());
    set.insert("IntervalStyle".to_string());
    set.insert("TimeZone".to_string());
    set.insert("standard_conforming_strings".to_string());
    set.insert("application_name".to_string());
    set
});

/// Read-only / server-injected GUCs that PostgreSQL refuses to `SET` at
/// runtime. A client that puts one of these in its StartupMessage must
/// not cause pg_doorman to issue `SET <name>` against the backend on
/// checkout, because PG will respond with SQLSTATE 55P02 and pg_doorman
/// will then mark the backend broken, eventually burning through the
/// pool.
static SET_FORBIDDEN_PARAMETERS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut s = HashSet::new();
    // Read-only / server-injected GUCs PG refuses to SET at runtime.
    s.insert("is_superuser");
    s.insert("session_authorization");
    s.insert("server_version");
    s.insert("server_version_num");
    s.insert("server_encoding");
    s.insert("integer_datetimes");
    s.insert("in_hot_standby");
    s.insert("session_user");
    s.insert("current_user");
    s.insert("block_size");
    s.insert("wal_block_size");
    s.insert("wal_segment_size");
    s.insert("max_index_keys");
    s.insert("max_identifier_length");
    s.insert("max_function_args");
    s.insert("data_checksums");
    s.insert("data_directory_mode");
    // Database-level GUCs frozen at CREATE DATABASE — PG returns 55P02 on
    // `SET lc_collate TO '...'`. They cannot vary per session, so even
    // though they affect planning they live with the read-only set
    // and stay out of PLANNER_KEYS.
    s.insert("lc_collate");
    s.insert("lc_ctype");
    // Per-transaction state — `SET transaction_isolation` is illegal
    // outside an active transaction (25P02); pg_doorman emits
    // `sync_parameters` on checkout, before BEGIN, so attempting
    // these would always fail.
    s.insert("transaction_isolation");
    s.insert("transaction_read_only");
    s.insert("transaction_deferrable");
    // StartupMessage-reserved names. `user`, `database`, `replication`,
    // `options` are handled by the wire protocol itself, not by SET;
    // attempting `SET user TO '...'` returns SQLSTATE 42704 because
    // there is no GUC by those names, and the failure poisons the
    // backend for the rest of the transaction.
    s.insert("user");
    s.insert("database");
    s.insert("replication");
    s.insert("options");
    s
});

/// GUCs that change planner output (and therefore the contents of a
/// cached prepared-statement plan). When any of these moves between
/// two checkouts of the same backend, pg_doorman must hand out a
/// different `DOORMAN_N` name so PostgreSQL prepares a fresh plan.
/// Names are stored in their canonical form (see `canonicalize_param_name`).
///
/// Extend this list when adding support for any new planner-visible
/// GUC. The wire-visible `TRACKED_PARAMETERS` set is unrelated — it
/// catalogues what PG reports back via `ParameterStatus`, not what
/// affects planning.
static PLANNER_KEYS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut s = HashSet::new();
    // Names PG accepts at session level (`SET search_path = ...`) and
    // that change the plan PostgreSQL caches at Parse time. `lc_collate`
    // and `lc_ctype` are deliberately *not* here — they affect plans
    // but are database-level and PG refuses to SET them, so they live
    // in `SET_FORBIDDEN_PARAMETERS` instead.
    s.insert("search_path");
    s.insert("default_transaction_isolation");
    s.insert("default_transaction_read_only");
    s.insert("default_text_search_config");
    s.insert("role");
    s
});

/// True when `name` is a planner-visible GUC whose change must
/// invalidate any cached prepared-statement-hash mix-in.
pub fn is_planner_key(name: &str) -> bool {
    PLANNER_KEYS.contains(name)
}

/// True when PostgreSQL would reject `SET <name> TO '<value>'` because
/// the GUC is read-only or server-supplied. pg_doorman uses this to
/// filter `sync_parameters` so a malicious or careless client can't
/// poison the pool by putting `is_superuser=on` in its StartupMessage.
pub fn is_set_forbidden(name: &str) -> bool {
    SET_FORBIDDEN_PARAMETERS.contains(name)
}

/// Canonicalise a PostgreSQL session parameter name. PG GUC lookups are
/// case-insensitive, so pg_doorman needs one normalised form per name
/// for every internal compare-by-key path (operator_managed key set,
/// cascade merge, dynamic-pool overlay hash, admin/Web read model). The
/// rule:
///
/// * Tracked parameters (`TRACKED_PARAMETERS`) return their fixed
///   spelling — the same casing PG reports back in `ParameterStatus`.
///   This keeps `sync_parameters` aligned with what the client expects
///   to see at the wire.
/// * Every other GUC is folded to ASCII lower case. PG itself accepts
///   any casing, but the cascade and admin views need a stable form so
///   `general.work_mem` plus `pool.Work_Mem` collapse to one entry
///   instead of shipping both rows in `StartupMessage`.
pub fn canonicalize_param_name(key: String) -> String {
    for tracked in TRACKED_PARAMETERS.iter() {
        if key.eq_ignore_ascii_case(tracked) {
            return tracked.clone();
        }
    }
    if key.chars().any(|c| c.is_ascii_uppercase()) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

/// One row of the `compare_params` diff. `sync_parameters` consumes
/// these to assemble the simple-query sent to the backend on checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamAction {
    /// `SET key TO 'value'` — value differs or backend has no record.
    SetTo(String),
    /// `RESET key` — backend has a value but the new client does not.
    Reset,
}

#[derive(Debug)]
pub struct ServerParameters {
    // Kept `pub(crate)` to preserve current internal usage patterns during refactor.
    pub(crate) parameters: HashMap<String, String>,

    /// Cached digest of `planner_params` computed on demand. Invalidated
    /// by `set_param` whenever a `PLANNER_KEYS` entry actually changes.
    /// Stored as `AtomicU64` with the `u64::MAX` sentinel meaning "not
    /// yet computed" so this struct stays `Send + Sync` even though
    /// the cache is mutated. Logically owned by a single tokio task
    /// (the Client), so contention is structural-not-real, but using
    /// `Cell` here would propagate `!Sync` to every Server / Client
    /// future that holds a `&ServerParameters` across await points.
    planner_hash_cache: std::sync::atomic::AtomicU64,
}

impl Clone for ServerParameters {
    fn clone(&self) -> Self {
        // A clone is a fresh owner that re-derives its own cache lazily;
        // cloning the atomic value would tie the new owner to digests
        // computed under the previous one's lifetime.
        ServerParameters {
            parameters: self.parameters.clone(),
            planner_hash_cache: std::sync::atomic::AtomicU64::new(PLANNER_HASH_UNSET),
        }
    }
}

/// Sentinel for `ServerParameters::planner_hash_cache` meaning "no
/// digest stored yet". Hash collisions on `u64::MAX` are theoretically
/// possible; the compute routine maps that one value to `u64::MAX - 1`
/// so the sentinel stays unambiguous.
const PLANNER_HASH_UNSET: u64 = u64::MAX;

impl Default for ServerParameters {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerParameters {
    pub fn new() -> Self {
        ServerParameters {
            parameters: HashMap::new(),
            planner_hash_cache: std::sync::atomic::AtomicU64::new(PLANNER_HASH_UNSET),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
    }

    pub fn admin() -> Self {
        let mut server_parameters = ServerParameters {
            parameters: HashMap::new(),
            planner_hash_cache: std::sync::atomic::AtomicU64::new(PLANNER_HASH_UNSET),
        };

        server_parameters.set_param("client_encoding", "UTF8", false);
        server_parameters.set_param("DateStyle", "ISO, MDY", false);
        server_parameters.set_param("TimeZone", "Etc/UTC", false);
        server_parameters.set_param("server_version", VERSION, true);
        server_parameters.set_param("server_encoding", "UTF-8", true);
        server_parameters.set_param("standard_conforming_strings", "on", false);
        // (64 bit = on) as of PostgreSQL 10, this is always on.
        server_parameters.set_param("integer_datetimes", "on", false);
        server_parameters.set_param("application_name", "pg_doorman", false);

        server_parameters
    }

    /// If `startup` is false, then only tracked parameters will be set.
    /// Returns `true` when the call actually changed a planner-visible
    /// GUC, so the caller can invalidate any cached prepared-statement
    /// hash that depends on the parameter snapshot.
    pub fn set_param(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
        startup: bool,
    ) -> bool {
        let key = canonicalize_param_name(key.into());
        let value = value.into();

        if TRACKED_PARAMETERS.contains(&key) || startup {
            let planner_relevant = is_planner_key(&key);
            let changed = match self.parameters.get(&key) {
                Some(existing) => existing != &value,
                None => true,
            };
            self.parameters.insert(key, value);
            if planner_relevant && changed {
                self.planner_hash_cache
                    .store(PLANNER_HASH_UNSET, std::sync::atomic::Ordering::Relaxed);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Bulk variant. Returns `true` if any of the inserts touched a
    /// planner-visible GUC — the caller treats that as a single
    /// invalidation event for the cached hash.
    pub fn set_from_hashmap(
        &mut self,
        parameters: &HashMap<String, String>,
        startup: bool,
    ) -> bool {
        let mut planner_changed = false;
        for (key, value) in parameters {
            if self.set_param(key, value, startup) {
                planner_changed = true;
            }
        }
        planner_changed
    }

    /// Diff between the backend's last known parameter state (`self`) and
    /// the client's desired state (`incoming_parameters`). For each key
    /// returns the action `sync_parameters` should run on the backend:
    ///
    ///   * `SetTo(value)` — `SET key TO 'value'`
    ///   * `Reset` — `RESET key` (backend has a value the client lacks)
    ///
    /// The iteration walks the union of both key sets, skipping read-only
    /// names listed in `SET_FORBIDDEN_PARAMETERS`. Anything the client
    /// puts in `is_superuser` or `server_version` is therefore ignored,
    /// not turned into a `SET` that would crash the backend with 55P02.
    #[inline(always)]
    pub(crate) fn compare_params(
        &self,
        incoming_parameters: &ServerParameters,
    ) -> HashMap<String, ParamAction> {
        let mut diff = HashMap::new();

        let mut keys: HashSet<&String> = HashSet::new();
        keys.extend(self.parameters.keys());
        keys.extend(incoming_parameters.parameters.keys());

        for key in keys {
            if is_set_forbidden(key) {
                continue;
            }
            match (
                self.parameters.get(key),
                incoming_parameters.parameters.get(key),
            ) {
                (Some(server_value), Some(client_value)) if server_value != client_value => {
                    diff.insert(key.clone(), ParamAction::SetTo(client_value.clone()));
                }
                (Some(_), None) => {
                    diff.insert(key.clone(), ParamAction::Reset);
                }
                (None, Some(client_value)) => {
                    diff.insert(key.clone(), ParamAction::SetTo(client_value.clone()));
                }
                _ => {}
            }
        }

        diff
    }

    pub fn get_application_name(&self) -> &String {
        // Can unwrap because we set it in the constructor.
        self.parameters.get("application_name").unwrap()
    }

    pub fn as_hashmap(&self) -> HashMap<String, String> {
        self.parameters.clone()
    }

    /// Single-`u64` digest of the planner-visible parameter set,
    /// suitable for folding into
    /// `Parse::get_hash_with_planner_params`. Returns `0` when no
    /// planner-visible GUC is set on the client, so the cache key
    /// stays byte-identical with the legacy `Parse::get_hash` path —
    /// existing prepared statements survive a rolling upgrade.
    ///
    /// Iteration order is the `BTreeMap` key order, so two clients
    /// with the same parameter set produce the same digest regardless
    /// of how their maps were assembled. Each entry contributes
    /// `key NUL value NUL` so `{a:"b","ab":""}` and `{ab:"b",a:""}`
    /// hash differently even though their byte concatenation matches —
    /// PostgreSQL forbids NUL inside GUC names and values, so this
    /// separator is safe.
    pub fn planner_param_hash(&self) -> u64 {
        let cached = self
            .planner_hash_cache
            .load(std::sync::atomic::Ordering::Relaxed);
        if cached != PLANNER_HASH_UNSET {
            return cached;
        }
        use std::hash::Hasher;
        let mut entries: Vec<(&String, &String)> = self
            .parameters
            .iter()
            .filter(|(k, _)| is_planner_key(k.as_str()))
            .collect();
        if entries.is_empty() {
            self.planner_hash_cache
                .store(0, std::sync::atomic::Ordering::Relaxed);
            return 0;
        }
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut hasher = xxhash_rust::xxh3::Xxh3::default();
        let mut count = 0u32;
        for (k, v) in &entries {
            hasher.write(k.as_bytes());
            hasher.write_u8(0);
            hasher.write(v.as_bytes());
            hasher.write_u8(0);
            count += 1;
        }
        hasher.write_u32(count);
        let h = hasher.finish();
        // 0 is the "no planner params" path in
        // `Parse::get_hash_with_planner_params`. PLANNER_HASH_UNSET is
        // the cache sentinel. Map both collisions away from real
        // hashes — astronomically rare but cheap to guard.
        let h = if h == 0 {
            1
        } else if h == PLANNER_HASH_UNSET {
            PLANNER_HASH_UNSET - 1
        } else {
            h
        };
        self.planner_hash_cache
            .store(h, std::sync::atomic::Ordering::Relaxed);
        h
    }

    fn add_parameter_message(key: &str, value: &str, buffer: &mut BytesMut) {
        buffer.put_u8(b'S');

        // 4 is len of i32, plus null terminators.
        let len = 4 + key.len() + 1 + value.len() + 1;

        buffer.put_i32(len as i32);
        buffer.put_slice(key.as_bytes());
        buffer.put_u8(0);
        buffer.put_slice(value.as_bytes());
        buffer.put_u8(0);
    }
}

impl From<&ServerParameters> for BytesMut {
    fn from(server_parameters: &ServerParameters) -> Self {
        let mut bytes = BytesMut::new();

        for (key, value) in &server_parameters.parameters {
            ServerParameters::add_parameter_message(key, value, &mut bytes);
        }

        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_timezone_matches_any_case() {
        assert_eq!(canonicalize_param_name("timezone".to_string()), "TimeZone");
        assert_eq!(canonicalize_param_name("TIMEZONE".to_string()), "TimeZone");
        assert_eq!(canonicalize_param_name("TimeZone".to_string()), "TimeZone");
        assert_eq!(canonicalize_param_name("TimezONE".to_string()), "TimeZone");
    }

    #[test]
    fn canonicalize_datestyle_matches_any_case() {
        assert_eq!(
            canonicalize_param_name("datestyle".to_string()),
            "DateStyle"
        );
        assert_eq!(
            canonicalize_param_name("DATESTYLE".to_string()),
            "DateStyle"
        );
        assert_eq!(
            canonicalize_param_name("DateStyle".to_string()),
            "DateStyle"
        );
    }

    #[test]
    fn canonicalize_intervalstyle_matches_any_case() {
        assert_eq!(
            canonicalize_param_name("intervalstyle".to_string()),
            "IntervalStyle"
        );
        assert_eq!(
            canonicalize_param_name("INTERVALSTYLE".to_string()),
            "IntervalStyle"
        );
    }

    #[test]
    fn canonicalize_lowercases_non_tracked_keys() {
        // PG GUC lookup is case-insensitive, so untracked names must
        // collapse to one canonical form too, otherwise a `work_mem`
        // baseline and a `Work_Mem` pool override become two rows on
        // the wire instead of one cascaded value.
        assert_eq!(canonicalize_param_name("work_mem".to_string()), "work_mem");
        assert_eq!(canonicalize_param_name("Work_Mem".to_string()), "work_mem");
        assert_eq!(canonicalize_param_name("WORK_MEM".to_string()), "work_mem");
        assert_eq!(
            canonicalize_param_name("statement_timeout".to_string()),
            "statement_timeout"
        );
        assert_eq!(
            canonicalize_param_name("Statement_Timeout".to_string()),
            "statement_timeout"
        );
    }

    #[test]
    fn is_set_forbidden_covers_read_only_and_reserved() {
        // Read-only GUCs PG returns 55P02 on.
        assert!(is_set_forbidden("is_superuser"));
        assert!(is_set_forbidden("server_version"));
        assert!(is_set_forbidden("lc_collate"));
        assert!(is_set_forbidden("lc_ctype"));
        // StartupMessage-reserved names that aren't GUCs (PG returns 42704).
        assert!(is_set_forbidden("user"));
        assert!(is_set_forbidden("database"));
        // Per-transaction state that pg_doorman has no business pushing
        // pre-BEGIN; PG returns 25P02 inside an open transaction.
        assert!(is_set_forbidden("transaction_isolation"));
        // search_path is the canonical mutable GUC the fix needs to push.
        assert!(!is_set_forbidden("search_path"));
        // Tracked wire-presentation GUCs stay settable.
        assert!(!is_set_forbidden("application_name"));
    }

    #[test]
    fn is_planner_key_targets_only_session_mutable_plan_inputs() {
        assert!(is_planner_key("search_path"));
        assert!(is_planner_key("default_transaction_isolation"));
        assert!(is_planner_key("role"));
        // lc_collate is plan-affecting but database-level — must live
        // in SET_FORBIDDEN_PARAMETERS, not PLANNER_KEYS, otherwise
        // pg_doorman would attempt SET and 55P02 would burn backends.
        assert!(!is_planner_key("lc_collate"));
        assert!(!is_planner_key("application_name"));
    }

    #[test]
    fn planner_param_hash_empty_returns_zero() {
        // The zero sentinel means "no planner state to fold"; callers
        // collapse it to the legacy `get_hash` for byte-compatibility.
        let sp = ServerParameters::new();
        assert_eq!(sp.planner_param_hash(), 0);
    }

    #[test]
    fn planner_param_hash_distinguishes_different_values() {
        let mut a = ServerParameters::new();
        a.set_param("search_path", "schema_a", true);
        let mut b = ServerParameters::new();
        b.set_param("search_path", "schema_b", true);
        assert_ne!(a.planner_param_hash(), b.planner_param_hash());
    }

    #[test]
    fn planner_param_hash_stable_for_identical_set() {
        // Two parameter maps populated in different order must hash
        // identically — that's the property that lets the digest
        // identify a planner state regardless of insertion history.
        let mut a = ServerParameters::new();
        a.set_param("search_path", "schema_a", true);
        a.set_param("role", "reader", true);
        let mut b = ServerParameters::new();
        b.set_param("role", "reader", true);
        b.set_param("search_path", "schema_a", true);
        assert_eq!(a.planner_param_hash(), b.planner_param_hash());
    }

    #[test]
    fn planner_param_hash_ignores_non_planner_keys() {
        // Two clients with different application_name / DateStyle but
        // the same planner state must collide on the hash — those are
        // wire-presentation knobs that don't change the plan.
        let mut a = ServerParameters::new();
        a.set_param("application_name", "client-A", true);
        let mut b = ServerParameters::new();
        b.set_param("application_name", "client-B", true);
        assert_eq!(a.planner_param_hash(), b.planner_param_hash());
        // Both are still the "no planner GUCs set" path.
        assert_eq!(a.planner_param_hash(), 0);
    }

    #[test]
    fn planner_param_hash_cache_invalidated_on_set() {
        // After the first read the cache stores a value; setting a
        // planner-relevant key must move the digest, not echo the
        // stale cached value.
        let mut sp = ServerParameters::new();
        sp.set_param("search_path", "schema_a", true);
        let h1 = sp.planner_param_hash();
        sp.set_param("search_path", "schema_b", true);
        let h2 = sp.planner_param_hash();
        assert_ne!(h1, h2);
    }

    #[test]
    fn planner_param_hash_cache_survives_non_planner_set() {
        // set_param on `application_name` (not in PLANNER_KEYS) must
        // not invalidate the cache. We verify by checking the second
        // call returns identical and is cheap enough that subsequent
        // reads still produce the same value.
        let mut sp = ServerParameters::new();
        sp.set_param("search_path", "schema_a", true);
        let h1 = sp.planner_param_hash();
        sp.set_param("application_name", "client-A", true);
        let h2 = sp.planner_param_hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn compare_params_emits_set_when_client_only() {
        let backend = ServerParameters::new();
        let mut client = ServerParameters::new();
        client.set_param("search_path", "schema_a", true);
        let diff = backend.compare_params(&client);
        match diff.get("search_path") {
            Some(ParamAction::SetTo(v)) => assert_eq!(v, "schema_a"),
            other => panic!("expected SetTo(schema_a), got {other:?}"),
        }
    }

    #[test]
    fn compare_params_emits_reset_when_backend_only() {
        // Sticky-state defence: backend retained a value from an earlier
        // checkout, new client doesn't pin it → must RESET so the next
        // query runs under the role default, not the previous client's
        // override.
        let mut backend = ServerParameters::new();
        backend.set_param("search_path", "schema_a", true);
        let client = ServerParameters::new();
        let diff = backend.compare_params(&client);
        assert!(matches!(diff.get("search_path"), Some(ParamAction::Reset)));
    }

    #[test]
    fn compare_params_skips_forbidden_names() {
        // Even if the client puts `is_superuser=on` in StartupMessage,
        // pg_doorman must not push it as a SET on the backend — PG
        // returns 55P02 and the backend is poisoned.
        let backend = ServerParameters::new();
        let mut client = ServerParameters::new();
        client.set_param("is_superuser", "on", true);
        client.set_param("user", "rogue", true);
        let diff = backend.compare_params(&client);
        assert!(!diff.contains_key("is_superuser"));
        assert!(!diff.contains_key("user"));
    }

    #[test]
    fn clone_resets_planner_hash_cache() {
        // Cloning a ServerParameters that already cached its digest
        // must not hand the new owner a digest computed under the
        // previous one. The clone is logically a fresh client; its
        // cache must start empty so the very first read recomputes.
        let mut sp = ServerParameters::new();
        sp.set_param("search_path", "schema_a", true);
        let _ = sp.planner_param_hash(); // populate the cache
        let cloned = sp.clone();
        // The clone's cache must report UNSET on read-back of the raw
        // atomic, even though it would compute to the same digest.
        let raw = cloned
            .planner_hash_cache
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(raw, PLANNER_HASH_UNSET);
    }
}
