//! Validation of operator-supplied PostgreSQL startup parameters.
//!
//! Used by [`crate::config::General`] and [`crate::config::Pool`] to refuse
//! configs that try to inject reserved protocol keys (user, database,
//! replication, options, `_pq_.*`), or that would exceed PG's
//! `MAX_STARTUP_PACKET_LENGTH` (10 000 bytes) StartupMessage body cap once
//! concatenated with `user`+`database`+`application_name`.

use std::collections::BTreeMap;

use crate::errors::Error;

/// PostgreSQL caps the StartupMessage body at 10 000 bytes
/// (`MAX_STARTUP_PACKET_LENGTH` in `src/include/libpq/pqcomm.h`) to prevent
/// memory-exhaustion attacks via oversize packets. pg_doorman reserves a
/// modest slice for its own `user`/`database`/`application_name` triple
/// and the protocol's per-pair NUL terminators; the rest is the budget
/// available to operator-supplied parameters.
pub const MAX_STARTUP_PACKET_SIZE: usize = 10_000;
pub const RESERVED_HEADROOM: usize = 512;
pub const MAX_OPERATOR_BUDGET: usize = MAX_STARTUP_PACKET_SIZE - RESERVED_HEADROOM;

/// Keys pg_doorman manages itself or that PG treats specially in the startup
/// packet. Operator must not put them in `startup_parameters`.
pub const RESERVED_KEYS: &[&str] = &["user", "database", "replication", "options"];
pub const RESERVED_PREFIX: &str = "_pq_.";

/// Allowed GUC name shape: ASCII letter / underscore, then letters /
/// digits / underscores / dots (for namespaced GUC like
/// `auto_explain.log_min_duration`). Equivalent to the regex
/// `^[A-Za-z_][A-Za-z0-9_.]*$`; hand-rolled to keep `regex` out of the
/// runtime dependency set.
fn is_valid_guc_name(key: &str) -> bool {
    let mut bytes = key.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

/// Validate one map (general or per-pool).
///
/// * `scope` — human-friendly label used in error messages, e.g.
///   `"general.startup_parameters"` or `"pool.startup_parameters"`.
pub fn validate(map: &BTreeMap<String, String>, scope: &str) -> Result<(), Error> {
    for (k, v) in map {
        validate_key(k, scope)?;
        validate_value(k, v, scope)?;
    }
    validate_total_size(map, scope)
}

fn validate_key(key: &str, scope: &str) -> Result<(), Error> {
    if key.is_empty() {
        return Err(Error::BadConfig(format!("{scope}: empty key")));
    }
    if RESERVED_KEYS.iter().any(|r| r.eq_ignore_ascii_case(key)) {
        return Err(Error::BadConfig(format!(
            "{scope}: '{key}' is reserved and managed by pg_doorman"
        )));
    }
    if key.starts_with(RESERVED_PREFIX) {
        return Err(Error::BadConfig(format!(
            "{scope}: '{key}' uses the reserved '_pq_.' prefix"
        )));
    }
    if !is_valid_guc_name(key) {
        return Err(Error::BadConfig(format!(
            "{scope}: '{key}' is not a valid GUC name (expected [A-Za-z_][A-Za-z0-9_.]*)"
        )));
    }
    Ok(())
}

fn validate_value(key: &str, value: &str, scope: &str) -> Result<(), Error> {
    if value.as_bytes().contains(&b'\0') {
        return Err(Error::BadConfig(format!(
            "{scope}: value for '{key}' contains a null byte"
        )));
    }
    Ok(())
}

fn validate_total_size(map: &BTreeMap<String, String>, scope: &str) -> Result<(), Error> {
    let total = serialized_bytes(map);
    if total > MAX_OPERATOR_BUDGET {
        return Err(Error::BadConfig(format!(
            "{scope}: serialized size {total} bytes exceeds operator budget {MAX_OPERATOR_BUDGET} \
             (PG StartupMessage cap is {MAX_STARTUP_PACKET_SIZE} bytes per \
             MAX_STARTUP_PACKET_LENGTH; {RESERVED_HEADROOM} reserved for \
             pg_doorman-managed keys)"
        )));
    }
    Ok(())
}

/// Bytes the operator-supplied map will occupy on the StartupMessage wire,
/// per the PG layout where each pair contributes `key\0value\0`.
pub fn serialized_bytes(map: &BTreeMap<String, String>) -> usize {
    map.iter().map(|(k, v)| k.len() + 1 + v.len() + 1).sum()
}

/// Exact byte length of the full StartupMessage pg_doorman will put on the
/// wire for one backend spawn, *including* the 4-byte length prefix. The
/// layout mirrors `crate::messages::protocol::startup`:
///
/// * 4 bytes - length prefix (the wire field itself)
/// * 4 bytes - protocol version
/// * `"user\0<user>\0"`, `"application_name\0<app>\0"`, `"database\0<database>\0"`
///   (`application_name` from `extras` wins over the pg_doorman default)
/// * each remaining `(key, value)` pair as `key\0value\0`
/// * 1 byte - parameter-list terminator (`\0`)
///
/// The per-level config validation only sees `extras`; this helper is what
/// the runtime path uses to ensure the *full* packet still fits under PG's
/// `MAX_STARTUP_PACKET_LENGTH` cap once user / database / application_name
/// are included.
pub fn full_packet_bytes(
    user: &str,
    database: &str,
    application_name: &str,
    extras: &BTreeMap<String, String>,
) -> usize {
    let mut total = 4usize + 4; // length prefix + protocol version
    total += b"user\0".len() + user.len() + 1;
    total += b"database\0".len() + database.len() + 1;
    let effective_app_name = extras
        .get("application_name")
        .map(String::as_str)
        .unwrap_or(application_name);
    total += b"application_name\0".len() + effective_app_name.len() + 1;
    for (key, value) in extras {
        if key == "application_name" {
            continue;
        }
        total += key.len() + 1 + value.len() + 1;
    }
    total += 1; // parameter-list terminator
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn empty_map_is_valid() {
        assert!(validate(&BTreeMap::new(), "general.startup_parameters").is_ok());
    }

    #[test]
    fn plain_guc_is_valid() {
        let map = m(&[
            ("plan_cache_mode", "force_custom_plan"),
            ("work_mem", "64MB"),
        ]);
        assert!(validate(&map, "general.startup_parameters").is_ok());
    }

    #[test]
    fn namespaced_guc_is_valid() {
        let map = m(&[("auto_explain.log_min_duration", "100ms")]);
        assert!(validate(&map, "pools.foo.startup_parameters").is_ok());
    }

    #[test]
    fn reserved_user_rejected() {
        let err = validate(&m(&[("user", "x")]), "scope").unwrap_err();
        assert!(matches!(err, Error::BadConfig(ref msg) if msg.contains("reserved")));
    }

    #[test]
    fn reserved_database_rejected_case_insensitive() {
        let err = validate(&m(&[("DATABASE", "x")]), "scope").unwrap_err();
        assert!(matches!(err, Error::BadConfig(ref msg) if msg.contains("reserved")));
    }

    #[test]
    fn pq_prefix_rejected() {
        let err = validate(&m(&[("_pq_.fancy_ext", "x")]), "scope").unwrap_err();
        assert!(matches!(err, Error::BadConfig(_)));
    }

    #[test]
    fn empty_key_rejected() {
        let err = validate(&m(&[("", "x")]), "scope").unwrap_err();
        assert!(matches!(err, Error::BadConfig(ref m) if m.contains("empty key")));
    }

    #[test]
    fn weird_chars_rejected() {
        let err = validate(&m(&[("bad name", "x")]), "scope").unwrap_err();
        assert!(matches!(err, Error::BadConfig(_)));
    }

    #[test]
    fn null_byte_in_value_rejected() {
        let err = validate(&m(&[("work_mem", "64\0MB")]), "scope").unwrap_err();
        assert!(matches!(err, Error::BadConfig(ref m) if m.contains("null byte")));
    }

    #[test]
    fn oversize_rejected() {
        // 16 keys × 1 KiB value still overruns the 9 488-byte operator budget.
        let big: BTreeMap<String, String> = (0..16)
            .map(|i| (format!("key{i}"), "a".repeat(1024)))
            .collect();
        let err = validate(&big, "scope").unwrap_err();
        assert!(matches!(err, Error::BadConfig(ref m) if m.contains("exceeds operator budget")));
    }

    #[test]
    fn application_name_is_not_reserved() {
        // application_name is explicitly allowed in startup_parameters; the
        // operator-wins merge against pg_doorman's default happens at the
        // wire layer, not here.
        let map = m(&[("application_name", "my_app")]);
        assert!(validate(&map, "scope").is_ok());
    }

    #[test]
    fn budget_matches_pg_startup_packet_cap() {
        // Locks the constants in place — PG's MAX_STARTUP_PACKET_LENGTH
        // (src/include/libpq/pqcomm.h) is 10 000; pg_doorman reserves
        // 512 bytes for its own keys, leaving 9 488 for the operator.
        // A future careless edit that drifts back to a 16 KiB ceiling
        // would re-introduce silently-rejected configs on every backend
        // startup; this assertion is the trip-wire.
        assert_eq!(MAX_STARTUP_PACKET_SIZE, 10_000);
        assert_eq!(RESERVED_HEADROOM, 512);
        assert_eq!(
            MAX_OPERATOR_BUDGET,
            MAX_STARTUP_PACKET_SIZE - RESERVED_HEADROOM
        );
    }

    #[test]
    fn serialized_bytes_counts_per_pair_nuls() {
        let map = m(&[("k1", "v1"), ("plan_cache_mode", "force_custom_plan")]);
        // "k1\0v1\0" = 2 + 1 + 2 + 1 = 6 bytes
        // "plan_cache_mode\0force_custom_plan\0" = 15 + 1 + 17 + 1 = 34 bytes
        assert_eq!(serialized_bytes(&map), 6 + 34);
    }

    #[test]
    fn serialized_bytes_empty_map_is_zero() {
        assert_eq!(serialized_bytes(&BTreeMap::new()), 0);
    }

    #[test]
    fn full_packet_bytes_matches_pg_layout() {
        let extras = m(&[]);
        // 4 + 4 + ("user\0"=5 + 4 + 1) + ("database\0"=9 + 4 + 1) +
        // ("application_name\0"=17 + 10 + 1) + 1 = 61
        let n = full_packet_bytes("usr1", "db01", "pg_doorman", &extras);
        assert_eq!(n, 4 + 4 + (5 + 4 + 1) + (9 + 4 + 1) + (17 + 10 + 1) + 1);
    }

    #[test]
    fn full_packet_bytes_overrides_application_name_from_extras() {
        let extras = m(&[("application_name", "checkout_pool")]);
        let n = full_packet_bytes("usr1", "db01", "pg_doorman", &extras);
        // Same as above but with "checkout_pool" (13 bytes) instead of
        // "pg_doorman" (10 bytes): 61 + 3 = 64.
        assert_eq!(n, 4 + 4 + (5 + 4 + 1) + (9 + 4 + 1) + (17 + 13 + 1) + 1);
    }

    #[test]
    fn full_packet_bytes_counts_each_extra_pair() {
        let extras = m(&[("plan_cache_mode", "force_custom_plan")]);
        // Base 61 + key("plan_cache_mode"=15 + 1) + value("force_custom_plan"=17 + 1) = 95.
        let n = full_packet_bytes("usr1", "db01", "pg_doorman", &extras);
        assert_eq!(n, 61 + (15 + 1) + (17 + 1));
    }

    #[test]
    fn cascade_overflow_detectable_after_merge() {
        // Each level fits the per-level budget on its own (every map below is
        // ~3 KiB), but the union of all three pushes past 9 488 bytes and
        // would trip the post-resolve guard in `server_pool.rs`.
        let general: BTreeMap<String, String> = (0..32)
            .map(|i| (format!("g_key_{i}"), "a".repeat(100)))
            .collect();
        let pool: BTreeMap<String, String> = (0..32)
            .map(|i| (format!("p_key_{i}"), "b".repeat(100)))
            .collect();
        let auth: BTreeMap<String, String> = (0..32)
            .map(|i| (format!("a_key_{i}"), "c".repeat(100)))
            .collect();
        // Each map ~ 32 * (8 + 1 + 100 + 1) = 32 * 110 = 3 520 bytes < 9 488.
        assert!(serialized_bytes(&general) < MAX_OPERATOR_BUDGET);
        assert!(serialized_bytes(&pool) < MAX_OPERATOR_BUDGET);
        assert!(serialized_bytes(&auth) < MAX_OPERATOR_BUDGET);

        let mut merged: BTreeMap<String, String> = BTreeMap::new();
        merged.extend(general.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged.extend(pool.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged.extend(auth.iter().map(|(k, v)| (k.clone(), v.clone())));
        assert!(serialized_bytes(&merged) > MAX_OPERATOR_BUDGET);
    }
}
