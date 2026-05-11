//! Pure cascade resolver for operator-supplied PostgreSQL startup parameters.
//!
//! Three levels merge by union; per-key, the more specific level wins
//! (auth_query > pool > general). The result is what pg_doorman sends in
//! `StartupMessage` for one backend connection.

use std::collections::{BTreeMap, HashMap};

/// Merge cascade and return the map pg_doorman will put on the wire.
///
/// `auth_query_params` is `None` for connections that don't go through
/// `auth_query` (static user) or for dedicated-mode auth_query pools where
/// per-user parameters are intentionally ignored (D7).
pub fn resolve(
    general: &BTreeMap<String, String>,
    pool: &BTreeMap<String, String>,
    auth_query_params: Option<&HashMap<String, String>>,
) -> BTreeMap<String, String> {
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    merged.extend(general.iter().map(|(k, v)| (k.clone(), v.clone())));
    merged.extend(pool.iter().map(|(k, v)| (k.clone(), v.clone())));
    if let Some(extra) = auth_query_params {
        merged.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
    fn h(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_cascade_yields_empty() {
        let r = resolve(&BTreeMap::new(), &BTreeMap::new(), None);
        assert!(r.is_empty());
    }

    #[test]
    fn general_baseline_passes_through() {
        let g = b(&[("statement_timeout", "10s")]);
        let r = resolve(&g, &BTreeMap::new(), None);
        assert_eq!(r.get("statement_timeout").map(String::as_str), Some("10s"));
    }

    #[test]
    fn pool_overrides_general_per_key() {
        let g = b(&[("plan_cache_mode", "auto"), ("statement_timeout", "10s")]);
        let p = b(&[("plan_cache_mode", "force_custom_plan")]);
        let r = resolve(&g, &p, None);
        assert_eq!(r.get("plan_cache_mode").unwrap(), "force_custom_plan");
        assert_eq!(r.get("statement_timeout").unwrap(), "10s");
    }

    #[test]
    fn auth_query_overrides_pool() {
        let p = b(&[("work_mem", "64MB")]);
        let a = h(&[("work_mem", "256MB"), ("lock_timeout", "5s")]);
        let r = resolve(&BTreeMap::new(), &p, Some(&a));
        assert_eq!(r.get("work_mem").unwrap(), "256MB");
        assert_eq!(r.get("lock_timeout").unwrap(), "5s");
    }

    #[test]
    fn dedicated_mode_signaled_by_none_auth_query() {
        let p = b(&[("work_mem", "64MB")]);
        let r = resolve(&BTreeMap::new(), &p, None);
        assert_eq!(r.get("work_mem").unwrap(), "64MB");
        assert!(!r.contains_key("lock_timeout"));
    }

    #[test]
    fn application_name_can_cascade_too() {
        // operator-wins on application_name (D5/B2): pool can override general's
        // baseline; auth_query in turn overrides pool.
        let g = b(&[("application_name", "tier-default")]);
        let p = b(&[("application_name", "checkout-pool")]);
        let a = h(&[("application_name", "vip-user-app")]);
        let r = resolve(&g, &p, Some(&a));
        assert_eq!(r.get("application_name").unwrap(), "vip-user-app");
    }
}
