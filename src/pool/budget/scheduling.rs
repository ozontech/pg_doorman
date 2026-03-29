use std::time::{Duration, Instant};

use super::types::BudgetState;

/// Grant one slot to `pool`: increment held, record connection age.
pub(crate) fn grant(state: &mut BudgetState, pool: &str, now: Instant) {
    let ps = state
        .pools
        .get_mut(pool)
        .expect("BUG: grant called for unregistered pool");
    ps.held += 1;
    ps.connection_ages.push_back(now);
    state.total_held += 1;
}

/// Evict one above-guarantee connection from `victim_pool`.
pub(crate) fn evict_one(
    state: &mut BudgetState,
    victim_pool: &str,
    now: Instant,
    min_lifetime: Duration,
) {
    let vs = state
        .pools
        .get_mut(victim_pool)
        .expect("BUG: evict_one called for unregistered pool");
    vs.held -= 1;
    if let Some(idx) = vs
        .connection_ages
        .iter()
        .position(|&t| now.duration_since(t) >= min_lifetime)
    {
        vs.connection_ages.remove(idx);
    } else {
        vs.connection_ages.pop_front();
    }
    state.total_held -= 1;
}

/// Enqueue a waiter for `pool`. Deduplicated: each pool appears at most once in waiters vec.
pub(crate) fn enqueue_waiter(state: &mut BudgetState, pool: &str) {
    let ps = state
        .pools
        .get_mut(pool)
        .expect("BUG: enqueue_waiter called for unregistered pool");
    if ps.waiting == 0 {
        state.waiters.push(pool.to_string());
    }
    ps.waiting += 1;
}

/// Remove one waiter request for `pool`. Remove from waiters vec if count reaches 0.
pub(crate) fn dequeue_waiter(state: &mut BudgetState, waiter_idx: usize, pool: &str) {
    let ps = state
        .pools
        .get_mut(pool)
        .expect("BUG: dequeue_waiter called for unregistered pool");
    ps.waiting -= 1;
    if ps.waiting == 0 {
        state.waiters.remove(waiter_idx);
    }
}

/// SCHEDULE: pick the best waiter and grant them a slot.
/// Returns the pool name that was auto-granted.
pub(crate) fn schedule(
    state: &mut BudgetState,
    max_connections: u32,
    min_lifetime: Duration,
    now: Instant,
) -> Option<String> {
    if state.waiters.is_empty() {
        return None;
    }

    let best_idx = select_best_waiter_idx(state)?;
    let best_pool = state.waiters[best_idx].clone();

    let best_state = state
        .pools
        .get(&best_pool)
        .expect("BUG: waiter references unregistered pool");
    let is_guaranteed = best_state.held < best_state.config.guaranteed;
    let weight = best_state.config.weight;

    if state.total_held < max_connections {
        dequeue_waiter(state, best_idx, &best_pool);
        grant(state, &best_pool, now);
        return Some(best_pool);
    }

    let requester_weight = if is_guaranteed { u32::MAX } else { weight };
    if let Some(victim) = find_evictable(state, &best_pool, requester_weight, now, min_lifetime) {
        let victim_name = victim.clone();
        evict_one(state, &victim_name, now, min_lifetime);
        dequeue_waiter(state, best_idx, &best_pool);
        grant(state, &best_pool, now);
        return Some(best_pool);
    }

    None
}

/// SELECT_BEST_WAITER: guaranteed first, then highest weight, then most waiting.
fn select_best_waiter_idx(state: &BudgetState) -> Option<usize> {
    if state.waiters.is_empty() {
        return None;
    }

    let mut best_idx = 0;
    let mut best_score = waiter_priority(state, &state.waiters[0]);

    for (i, pool_name) in state.waiters.iter().enumerate().skip(1) {
        let score = waiter_priority(state, pool_name);
        if score > best_score {
            best_score = score;
            best_idx = i;
        }
    }

    Some(best_idx)
}

/// Priority tuple: (is_guaranteed, weight, waiting_count).
fn waiter_priority(state: &BudgetState, pool_name: &str) -> (bool, u32, u32) {
    let ps = &state.pools[pool_name];
    let is_guaranteed = ps.held < ps.config.guaranteed;
    (is_guaranteed, ps.config.weight, ps.waiting)
}

/// Check if any waiter with strictly higher weight exists (excluding self).
pub(crate) fn has_higher_weight_waiter(
    state: &BudgetState,
    requesting_pool: &str,
    requesting_weight: u32,
) -> bool {
    state
        .waiters
        .iter()
        .any(|w| w != requesting_pool && state.pools[w].config.weight > requesting_weight)
}

/// FIND_EVICTABLE: above-guarantee, old enough, lower weight.
pub(crate) fn find_evictable(
    state: &BudgetState,
    requester: &str,
    requester_weight: u32,
    now: Instant,
    min_lifetime: Duration,
) -> Option<String> {
    find_evictable_with_blocked_count(state, requester, requester_weight, now, min_lifetime, None)
}

/// Like find_evictable but optionally counts blocked evictions (min_lifetime protection).
pub(crate) fn find_evictable_with_blocked_count(
    state: &BudgetState,
    requester: &str,
    requester_weight: u32,
    now: Instant,
    min_lifetime: Duration,
    blocked_count: Option<&mut u32>,
) -> Option<String> {
    let mut best: Option<(u32, Duration, String)> = None;
    let mut blocked = 0u32;

    for (name, ps) in &state.pools {
        if name == requester {
            continue;
        }
        if ps.held <= ps.config.guaranteed {
            continue;
        }
        if requester_weight != u32::MAX && ps.config.weight >= requester_weight {
            continue;
        }
        let mut has_eligible = false;
        let mut has_weight_match = false;
        for &t in &ps.connection_ages {
            if now.duration_since(t) >= min_lifetime {
                has_eligible = true;
                break;
            }
            has_weight_match = true;
        }
        if !has_eligible {
            if has_weight_match {
                blocked += 1;
            }
            continue;
        }
        let max_age = now.duration_since(*ps.connection_ages.front().unwrap());

        let dominated = match &best {
            None => true,
            Some((bw, ba, _)) => {
                ps.config.weight < *bw || (ps.config.weight == *bw && max_age > *ba)
            }
        };
        if dominated {
            best = Some((ps.config.weight, max_age, name.clone()));
        }
    }

    if let Some(count) = blocked_count {
        *count = blocked;
    }

    best.map(|(_, _, name)| name)
}
