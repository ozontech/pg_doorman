//! Pure cascade resolver for operator-supplied PostgreSQL startup parameters.
//!
//! Three levels merge by union; per-key, the more specific level wins
//! (auth_query > pool > general). The result is what pg_doorman sends in
//! `StartupMessage` for one backend connection.

use std::collections::{BTreeMap, HashMap};

/// Merge cascade and return the map pg_doorman will put on the wire.
///
/// `auth_query_params` is `None` for connections that don't go through
/// `auth_query` (static user), and also for dedicated-mode auth_query pools
/// where one shared backend serves multiple dynamic users so per-user
/// parameters cannot be honoured.
///
/// The production hot path goes through
/// [`ServerPool::resolved_startup_parameters`] using a cached
/// `Arc<BTreeMap>` for the general+pool base; this function is the
/// pure-cascade variant kept as the canonical reference of the merge
/// rule and exercised by unit tests.
#[allow(dead_code)]
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

/// Layer in the cascade that contributed the winning value for a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterSource {
    General,
    Pool,
    AuthQuery,
}

impl ParameterSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ParameterSource::General => "general",
            ParameterSource::Pool => "pool",
            ParameterSource::AuthQuery => "auth_query",
        }
    }
}

/// Wire-application state for an entry returned by
/// `ServerPool::effective_startup_parameters_with_sources`. Lets the
/// admin/Web UI flag entries that the operator configured but the
/// runtime will not actually ship in `StartupMessage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationState {
    /// Configured value matches the wire-ready map; the next backend
    /// spawn will ship this key/value.
    Applied,
    /// Runtime dropped the key (operator cascade exceeded the budget
    /// or the packet cap on the most recent backend spawn — the
    /// `*_dropped_total` counter ticked on the same spawn).
    DroppedDueToBudget,
    /// Wire map ships a different value than the live config has. The
    /// pool's frozen baseline / overlay snapshot is stale — RELOAD or
    /// auth_query cache refetch has not yet recycled this pool.
    Stale,
}

impl ApplicationState {
    pub fn as_str(self) -> &'static str {
        match self {
            ApplicationState::Applied => "applied",
            ApplicationState::DroppedDueToBudget => "dropped_due_to_budget",
            ApplicationState::Stale => "stale",
        }
    }
}

/// Same cascade as [`resolve`], but carries the layer that contributed each
/// winning value. Used by `SHOW STARTUP_PARAMETERS` and `/api/pools` so an
/// operator can see "this `work_mem` came from the pool, that `lock_timeout`
/// from auth_query" without re-reading config plus the auth_query cache.
pub fn resolve_with_sources(
    general: &BTreeMap<String, String>,
    pool: &BTreeMap<String, String>,
    auth_query_params: Option<&HashMap<String, String>>,
) -> BTreeMap<String, (String, ParameterSource)> {
    // Canonicalise keys at insert so the read model matches the
    // wire-ready map produced by `cascade_canonical_keys` in the
    // runtime. Without this, a `timezone` baseline and a `TimeZone`
    // override surface as two entries in `SHOW STARTUP_PARAMETERS` /
    // `/api/pools`, and the wire compare in
    // `effective_startup_parameters_with_sources` flags one variant as
    // `dropped_due_to_budget`/`stale`. Later layers win because their
    // canonical key replaces earlier inserts at the same key.
    let canon = crate::server::parameters::canonicalize_param_name;
    let mut out: BTreeMap<String, (String, ParameterSource)> = BTreeMap::new();
    for (k, v) in general {
        out.insert(canon(k.clone()), (v.clone(), ParameterSource::General));
    }
    for (k, v) in pool {
        out.insert(canon(k.clone()), (v.clone(), ParameterSource::Pool));
    }
    if let Some(extra) = auth_query_params {
        for (k, v) in extra {
            out.insert(canon(k.clone()), (v.clone(), ParameterSource::AuthQuery));
        }
    }
    out
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
    fn resolve_with_sources_attributes_each_layer() {
        let g = b(&[("statement_timeout", "10s"), ("plan_cache_mode", "auto")]);
        let p = b(&[
            ("plan_cache_mode", "force_custom_plan"),
            ("work_mem", "64MB"),
        ]);
        let a = h(&[("work_mem", "256MB"), ("lock_timeout", "5s")]);
        let r = resolve_with_sources(&g, &p, Some(&a));
        assert_eq!(
            r.get("statement_timeout"),
            Some(&("10s".to_string(), ParameterSource::General))
        );
        assert_eq!(
            r.get("plan_cache_mode"),
            Some(&("force_custom_plan".to_string(), ParameterSource::Pool))
        );
        assert_eq!(
            r.get("work_mem"),
            Some(&("256MB".to_string(), ParameterSource::AuthQuery))
        );
        assert_eq!(
            r.get("lock_timeout"),
            Some(&("5s".to_string(), ParameterSource::AuthQuery))
        );
    }

    #[test]
    fn resolve_with_sources_canonicalises_tracked_keys() {
        // PG GUC lookup is case-insensitive; the runtime cascade
        // canonicalises names before they reach the wire (e.g. pool
        // `TimeZone` collapses with general `timezone`). The read model
        // backing `SHOW STARTUP_PARAMETERS` and `/api/pools` must do the
        // same — otherwise the same logical GUC appears twice in the
        // SHOW output and the wire-map compare flags one variant as
        // `dropped_due_to_budget`/`stale`.
        let g = b(&[("timezone", "UTC")]);
        let p = b(&[("TimeZone", "Europe/Moscow")]);
        let r = resolve_with_sources(&g, &p, None);
        assert!(
            !r.contains_key("timezone"),
            "raw lowercase key must not survive canonicalisation"
        );
        assert_eq!(
            r.get("TimeZone"),
            Some(&("Europe/Moscow".to_string(), ParameterSource::Pool))
        );
    }

    #[test]
    fn resolve_with_sources_canonicalises_non_tracked_keys() {
        // For non-tracked GUCs canonicalisation is `to_ascii_lowercase`,
        // mirroring `canonicalize_param_name`. Without it `Work_Mem`
        // (pool) and `work_mem` (general) would both appear in the SHOW
        // output and only one would be present on the wire.
        let g = b(&[("work_mem", "64MB")]);
        let p = b(&[("Work_Mem", "256MB")]);
        let r = resolve_with_sources(&g, &p, None);
        assert!(!r.contains_key("Work_Mem"));
        assert_eq!(
            r.get("work_mem"),
            Some(&("256MB".to_string(), ParameterSource::Pool))
        );
    }

    #[test]
    fn resolve_with_sources_canonicalises_auth_query_keys() {
        // auth_query JSON rows can return any casing for tracked GUCs.
        // The read model must canonicalise them too so the auth_query
        // overlay overrides the same canonical key the runtime ships.
        let p = b(&[("TimeZone", "UTC")]);
        let a = h(&[("timezone", "Europe/Moscow")]);
        let r = resolve_with_sources(&BTreeMap::new(), &p, Some(&a));
        assert_eq!(
            r.get("TimeZone"),
            Some(&("Europe/Moscow".to_string(), ParameterSource::AuthQuery))
        );
        assert!(!r.contains_key("timezone"));
    }

    #[test]
    fn application_name_can_cascade_too() {
        // operator-wins on application_name extends through the cascade:
        // pool overrides general's baseline; auth_query overrides pool.
        let g = b(&[("application_name", "tier-default")]);
        let p = b(&[("application_name", "checkout-pool")]);
        let a = h(&[("application_name", "vip-user-app")]);
        let r = resolve(&g, &p, Some(&a));
        assert_eq!(r.get("application_name").unwrap(), "vip-user-app");
    }
}
