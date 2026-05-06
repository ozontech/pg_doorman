use crate::pool::get_all_pools;
use crate::web::routes::dto::{PoolScalingDto, PoolScalingRowDto};

use super::now_unix_ms;

pub(crate) fn collect_pool_scaling() -> PoolScalingDto {
    let mut entries: Vec<_> = get_all_pools()
        .iter()
        .map(|(id, pool)| (id.clone(), pool.database.scaling_stats()))
        .collect();
    entries.sort_by(|a, b| (&a.0.db, &a.0.user).cmp(&(&b.0.db, &b.0.user)));

    let pools = entries
        .into_iter()
        .map(|(id, snapshot)| PoolScalingRowDto {
            user: id.user.clone(),
            database: id.db.clone(),
            inflight: snapshot.inflight_creates as u64,
            creates: snapshot.creates_started,
            gate_waits: snapshot.burst_gate_waits,
            gate_budget_ex: snapshot.burst_gate_budget_exhausted,
            antic_notify: snapshot.anticipation_wakes_notify,
            antic_timeout: snapshot.anticipation_wakes_timeout,
            create_fallback: snapshot.create_fallback,
            replenish_def: snapshot.replenish_deferred,
        })
        .collect();

    PoolScalingDto {
        ts: now_unix_ms(),
        pools,
    }
}
