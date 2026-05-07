use crate::pool::{AUTH_QUERY_STATE, DYNAMIC_POOLS};
use crate::web::routes::dto::{AuthQueryDto, AuthQueryRowDto};

use super::now_unix_ms;

pub(crate) fn collect_auth_query() -> AuthQueryDto {
    let states = AUTH_QUERY_STATE.load();
    let dynamic = DYNAMIC_POOLS.load();

    let mut pools: Vec<AuthQueryRowDto> = states
        .iter()
        .map(|(pool_name, state)| {
            let cache_entries = state.cache_len() as u64;
            let dyn_current = dynamic.iter().filter(|id| id.db == *pool_name).count() as u64;
            let s = state.stats.snapshot();
            AuthQueryRowDto {
                database: pool_name.clone(),
                cache_entries,
                cache_hits: s.cache_hits,
                cache_misses: s.cache_misses,
                cache_refetches: s.cache_refetches,
                cache_rate_limited: s.cache_rate_limited,
                auth_success: s.auth_success,
                auth_failure: s.auth_failure,
                executor_queries: s.executor_queries,
                executor_errors: s.executor_errors,
                dynamic_pools_current: dyn_current,
                dynamic_pools_created: s.dynamic_pools_created,
                dynamic_pools_destroyed: s.dynamic_pools_destroyed,
            }
        })
        .collect();

    pools.sort_by(|a, b| a.database.cmp(&b.database));

    AuthQueryDto {
        ts: now_unix_ms(),
        pools,
    }
}
