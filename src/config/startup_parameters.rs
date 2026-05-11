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
    // Per the PG wire layout each pair contributes key, NUL, value, NUL.
    let total: usize = map.iter().map(|(k, v)| k.len() + 1 + v.len() + 1).sum();
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
        // Operator-wins (B2): explicitly allowed; pg_doorman default merges
        // happen in the wire layer, not validation.
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
}
