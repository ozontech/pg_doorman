use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Result of a non-blocking acquire attempt.
#[derive(Debug, PartialEq)]
pub enum AcquireResult {
    /// Slot granted immediately.
    Granted,
    /// Slot granted after evicting a connection from another pool.
    GrantedAfterEviction { evicted_pool: String },
    /// Pool is full, no evictable connections. Caller should wait.
    WouldBlock,
    /// User is at their max_pool_size.
    DeniedUserMax,
    /// Pool not registered.
    DeniedUnknownPool,
}

/// Configuration for a pool participant in the budget.
#[derive(Debug, Clone, Copy)]
pub struct PoolBudgetConfig {
    pub guaranteed: u32,
    pub weight: u32,
    pub max_pool_size: u32,
}

/// Instance-level budget controller for server connections.
///
/// Controls the total number of server connections to a PostgreSQL
/// server (host:port) across all pools. Each pool has:
/// - guaranteed: connections always available, opens immediately
/// - weight: priority for above-guarantee competition
/// - max_pool_size: per-pool hard cap
///
/// Eviction rules:
/// - Only above-guarantee connections can be evicted
/// - Only by a requester with strictly higher weight (or guaranteed request = weight ∞)
/// - Only if the connection is older than min_connection_lifetime
pub struct BudgetController {
    max_connections: u32,
    min_lifetime: Duration,
    state: Mutex<BudgetState>,
}

struct BudgetState {
    pools: HashMap<String, PoolState>,
    total_held: u32,
    /// Deduplicated waiter set in insertion order.
    /// Each pool appears at most once; PoolState.waiting tracks the count.
    waiters: Vec<String>,
}

struct PoolState {
    config: PoolBudgetConfig,
    held: u32,
    waiting: u32,
    /// Creation timestamps of held connections, oldest first (front = oldest).
    connection_ages: VecDeque<Instant>,
}

impl BudgetController {
    pub fn new(max_connections: u32, min_lifetime: Duration) -> Self {
        Self {
            max_connections,
            min_lifetime,
            state: Mutex::new(BudgetState {
                pools: HashMap::new(),
                total_held: 0,
                waiters: Vec::new(),
            }),
        }
    }

    pub fn register_pool(&self, name: &str, config: PoolBudgetConfig) {
        let mut state = self.state.lock();
        state.pools.insert(
            name.to_string(),
            PoolState {
                config,
                held: 0,
                waiting: 0,
                connection_ages: VecDeque::new(),
            },
        );
    }

    pub fn unregister_pool(&self, name: &str, now: Instant) {
        let mut state = self.state.lock();
        if let Some(pool) = state.pools.remove(name) {
            state.total_held -= pool.held;
            state.waiters.retain(|w| w != name);

            // Drain: freed capacity may satisfy queued waiters
            while Self::schedule(&mut state, self.max_connections, self.min_lifetime, now).is_some()
            {
            }
        }
    }

    /// Check if the guaranteed budget invariant holds.
    pub fn validate_guarantees(&self) -> Result<(), String> {
        let state = self.state.lock();
        let sum: u32 = state.pools.values().map(|p| p.config.guaranteed).sum();
        if sum > self.max_connections {
            return Err(format!(
                "sum(guaranteed)={} > max_connections={}",
                sum, self.max_connections
            ));
        }
        Ok(())
    }

    /// Non-blocking attempt to acquire a server connection slot.
    ///
    /// `now` is passed explicitly for testability (no clock mocking needed).
    pub fn try_acquire(&self, pool: &str, now: Instant) -> AcquireResult {
        let mut state = self.state.lock();

        let pool_state = match state.pools.get(pool) {
            Some(p) => p,
            None => return AcquireResult::DeniedUnknownPool,
        };

        let held = pool_state.held;
        let config = pool_state.config;

        if held >= config.max_pool_size {
            return AcquireResult::DeniedUserMax;
        }

        let is_guaranteed = held < config.guaranteed;

        // Case 1: room in global budget
        if state.total_held < self.max_connections {
            if !is_guaranteed && Self::has_higher_weight_waiter(&state, pool, config.weight) {
                Self::enqueue_waiter(&mut state, pool);
                return AcquireResult::WouldBlock;
            }
            Self::grant(&mut state, pool, now);
            return AcquireResult::Granted;
        }

        // Case 2: global budget full — try eviction
        let requester_weight = if is_guaranteed {
            u32::MAX
        } else {
            config.weight
        };
        if let Some(victim) =
            Self::find_evictable(&state, pool, requester_weight, now, self.min_lifetime)
        {
            let victim_name = victim.clone();
            Self::evict_one(&mut state, &victim_name, now, self.min_lifetime);
            Self::grant(&mut state, pool, now);
            return AcquireResult::GrantedAfterEviction {
                evicted_pool: victim_name,
            };
        }

        // Case 3: no evictable connections — enqueue
        Self::enqueue_waiter(&mut state, pool);
        AcquireResult::WouldBlock
    }

    /// Release a server connection slot. Triggers SCHEDULE.
    ///
    /// Returns the pool name that was auto-granted a slot (if any).
    pub fn release(&self, pool: &str, now: Instant) -> Option<String> {
        let mut state = self.state.lock();

        if let Some(ps) = state.pools.get_mut(pool) {
            if ps.held > 0 {
                ps.held -= 1;
                ps.connection_ages.pop_front();
                state.total_held -= 1;
            }
        }

        Self::schedule(&mut state, self.max_connections, self.min_lifetime, now)
    }

    // --- Internal helpers ---

    /// Grant one slot to `pool`: increment held, record connection age.
    fn grant(state: &mut BudgetState, pool: &str, now: Instant) {
        let ps = state.pools.get_mut(pool).unwrap();
        ps.held += 1;
        ps.connection_ages.push_back(now);
        state.total_held += 1;
    }

    /// Evict one above-guarantee connection from `victim_pool`.
    fn evict_one(state: &mut BudgetState, victim_pool: &str, now: Instant, min_lifetime: Duration) {
        let vs = state.pools.get_mut(victim_pool).unwrap();
        vs.held -= 1;
        // Remove oldest eligible connection
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
    fn enqueue_waiter(state: &mut BudgetState, pool: &str) {
        let ps = state.pools.get_mut(pool).unwrap();
        if ps.waiting == 0 {
            state.waiters.push(pool.to_string());
        }
        ps.waiting += 1;
    }

    /// SCHEDULE: pick the best waiter and grant them a slot.
    fn schedule(
        state: &mut BudgetState,
        max_connections: u32,
        min_lifetime: Duration,
        now: Instant,
    ) -> Option<String> {
        if state.waiters.is_empty() {
            return None;
        }

        let best_idx = Self::select_best_waiter_idx(state)?;
        let best_pool = state.waiters[best_idx].clone();

        let best_state = state.pools.get(&best_pool).unwrap();
        let is_guaranteed = best_state.held < best_state.config.guaranteed;
        let weight = best_state.config.weight;

        if state.total_held < max_connections {
            Self::dequeue_waiter(state, best_idx, &best_pool);
            Self::grant(state, &best_pool, now);
            return Some(best_pool);
        }

        // Try eviction for the best waiter
        let requester_weight = if is_guaranteed { u32::MAX } else { weight };
        if let Some(victim) =
            Self::find_evictable(state, &best_pool, requester_weight, now, min_lifetime)
        {
            let victim_name = victim.clone();
            Self::evict_one(state, &victim_name, now, min_lifetime);
            Self::dequeue_waiter(state, best_idx, &best_pool);
            Self::grant(state, &best_pool, now);
            return Some(best_pool);
        }

        None
    }

    /// Remove one waiter request for `pool`. Remove from waiters vec if count reaches 0.
    fn dequeue_waiter(state: &mut BudgetState, waiter_idx: usize, pool: &str) {
        let ps = state.pools.get_mut(pool).unwrap();
        ps.waiting -= 1;
        if ps.waiting == 0 {
            state.waiters.remove(waiter_idx);
        }
    }

    /// SELECT_BEST_WAITER: guaranteed first, then highest weight, then most waiting.
    fn select_best_waiter_idx(state: &BudgetState) -> Option<usize> {
        if state.waiters.is_empty() {
            return None;
        }

        let mut best_idx = 0;
        let mut best_score = Self::waiter_priority(state, &state.waiters[0]);

        for (i, pool_name) in state.waiters.iter().enumerate().skip(1) {
            let score = Self::waiter_priority(state, pool_name);
            if score > best_score {
                best_score = score;
                best_idx = i;
            }
        }

        Some(best_idx)
    }

    /// Priority tuple: (is_guaranteed, weight, waiting_count).
    /// Compared lexicographically descending: guaranteed > not, higher weight > lower,
    /// more queued requests > fewer.
    fn waiter_priority(state: &BudgetState, pool_name: &str) -> (bool, u32, u32) {
        let ps = &state.pools[pool_name];
        let is_guaranteed = ps.held < ps.config.guaranteed;
        (is_guaranteed, ps.config.weight, ps.waiting)
    }

    /// Check if any waiter with strictly higher weight exists (excluding self).
    fn has_higher_weight_waiter(
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
    fn find_evictable(
        state: &BudgetState,
        requester: &str,
        requester_weight: u32,
        now: Instant,
        min_lifetime: Duration,
    ) -> Option<String> {
        let mut best: Option<(u32, Duration, String)> = None;

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
            let has_eligible = ps
                .connection_ages
                .iter()
                .any(|&t| now.duration_since(t) >= min_lifetime);
            if !has_eligible {
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

        best.map(|(_, _, name)| name)
    }

    // --- Getters ---

    pub fn held(&self, pool: &str) -> u32 {
        self.state.lock().pools.get(pool).map_or(0, |p| p.held)
    }

    pub fn total_held(&self) -> u32 {
        self.state.lock().total_held
    }

    pub fn waiting(&self, pool: &str) -> u32 {
        self.state.lock().pools.get(pool).map_or(0, |p| p.waiting)
    }

    pub fn max_connections(&self) -> u32 {
        self.max_connections
    }

    pub fn above_guarantee(&self, pool: &str) -> u32 {
        let state = self.state.lock();
        state
            .pools
            .get(pool)
            .map_or(0, |p| p.held.saturating_sub(p.config.guaranteed))
    }

    /// Inject connections with specific ages (for testing eviction scenarios).
    #[cfg(test)]
    pub fn set_held_with_age(&self, pool: &str, count: u32, created_at: Instant) {
        let mut state = self.state.lock();
        if let Some(ps) = state.pools.get_mut(pool) {
            let old_held = ps.held;
            ps.held = count;
            ps.connection_ages.clear();
            for _ in 0..count {
                ps.connection_ages.push_back(created_at);
            }
            state.total_held = state.total_held - old_held + count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(guaranteed: u32, weight: u32, max: u32) -> PoolBudgetConfig {
        PoolBudgetConfig {
            guaranteed,
            weight,
            max_pool_size: max,
        }
    }

    fn setup_standard() -> (BudgetController, Instant) {
        let bc = BudgetController::new(20, Duration::from_secs(30));
        bc.register_pool("service_api", cfg(5, 100, 15));
        bc.register_pool("batch_worker", cfg(3, 50, 10));
        bc.register_pool("analytics", cfg(0, 10, 5));
        (bc, Instant::now())
    }

    // --- Normal operation ---

    #[test]
    fn guaranteed_connections_granted_immediately() {
        let (bc, now) = setup_standard();
        for _ in 0..5 {
            assert_eq!(bc.try_acquire("service_api", now), AcquireResult::Granted);
        }
        assert_eq!(bc.held("service_api"), 5);
        assert_eq!(bc.total_held(), 5);
    }

    #[test]
    fn above_guarantee_granted_when_pool_not_full() {
        let (bc, now) = setup_standard();
        for _ in 0..8 {
            assert_eq!(bc.try_acquire("service_api", now), AcquireResult::Granted);
        }
        assert_eq!(bc.held("service_api"), 8);
        assert_eq!(bc.above_guarantee("service_api"), 3);
    }

    #[test]
    fn multiple_users_fill_pool() {
        let (bc, now) = setup_standard();
        for _ in 0..8 {
            bc.try_acquire("service_api", now);
        }
        for _ in 0..5 {
            bc.try_acquire("batch_worker", now);
        }
        for _ in 0..3 {
            bc.try_acquire("analytics", now);
        }
        assert_eq!(bc.total_held(), 16);
        assert_eq!(bc.held("service_api"), 8);
        assert_eq!(bc.held("batch_worker"), 5);
        assert_eq!(bc.held("analytics"), 3);
    }

    // --- EC-1: Equal weight, pool full ---

    #[test]
    fn ec1_equal_weight_pool_full_would_block() {
        let bc = BudgetController::new(20, Duration::from_secs(30));
        bc.register_pool("user_a", cfg(0, 100, 20));
        bc.register_pool("user_b", cfg(0, 100, 10));
        let now = Instant::now();

        for _ in 0..20 {
            bc.try_acquire("user_a", now);
        }
        assert_eq!(bc.try_acquire("user_b", now), AcquireResult::WouldBlock);
        assert_eq!(bc.waiting("user_b"), 1);
    }

    #[test]
    fn ec1_equal_weight_gets_connection_on_return() {
        let bc = BudgetController::new(20, Duration::from_secs(30));
        bc.register_pool("user_a", cfg(0, 100, 20));
        bc.register_pool("user_b", cfg(0, 100, 10));
        let now = Instant::now();

        for _ in 0..20 {
            bc.try_acquire("user_a", now);
        }
        bc.try_acquire("user_b", now);

        let granted = bc.release("user_a", now);
        assert_eq!(granted, Some("user_b".to_string()));
        assert_eq!(bc.held("user_b"), 1);
        assert_eq!(bc.held("user_a"), 19);
    }

    // --- EC-2: Lowest weight, pool full ---

    #[test]
    fn ec2_lowest_weight_cannot_evict() {
        let (bc, now) = setup_standard();
        for _ in 0..12 {
            bc.try_acquire("service_api", now);
        }
        for _ in 0..5 {
            bc.try_acquire("batch_worker", now);
        }
        for _ in 0..3 {
            bc.try_acquire("analytics", now);
        }
        assert_eq!(bc.total_held(), 20);

        bc.register_pool("new_app", cfg(0, 5, 5));
        assert_eq!(bc.try_acquire("new_app", now), AcquireResult::WouldBlock);
    }

    #[test]
    fn ec2_lowest_weight_served_when_no_higher_waiter() {
        let (bc, now) = setup_standard();
        for _ in 0..12 {
            bc.try_acquire("service_api", now);
        }
        for _ in 0..5 {
            bc.try_acquire("batch_worker", now);
        }
        for _ in 0..3 {
            bc.try_acquire("analytics", now);
        }

        bc.register_pool("new_app", cfg(0, 5, 5));
        bc.try_acquire("new_app", now);

        let granted = bc.release("service_api", now);
        assert_eq!(granted, Some("new_app".to_string()));
    }

    #[test]
    fn ec2_lowest_weight_loses_to_higher_weight_waiter() {
        let bc = BudgetController::new(10, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 10));
        bc.register_pool("low", cfg(0, 10, 10));
        bc.register_pool("filler", cfg(0, 100, 10));
        let now = Instant::now();

        for _ in 0..10 {
            bc.try_acquire("filler", now);
        }
        bc.try_acquire("low", now);
        bc.try_acquire("high", now);

        let granted = bc.release("filler", now);
        assert_eq!(granted, Some("high".to_string()));
    }

    // --- EC-3: Guaranteed evicts any weight ---

    #[test]
    fn ec3_guaranteed_evicts_any_above_guarantee() {
        let (bc, _) = setup_standard();
        let old = Instant::now() - Duration::from_secs(60);

        bc.set_held_with_age("service_api", 12, old);
        bc.set_held_with_age("batch_worker", 5, old);
        bc.set_held_with_age("analytics", 3, old);

        bc.register_pool("admin", cfg(2, 1, 2));

        let now = Instant::now();
        let result = bc.try_acquire("admin", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "analytics")
        );
        assert_eq!(bc.held("admin"), 1);
        assert_eq!(bc.held("analytics"), 2);

        let result = bc.try_acquire("admin", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "analytics")
        );
        assert_eq!(bc.held("admin"), 2);
        assert_eq!(bc.held("analytics"), 1);
    }

    // --- EC-4: All within guarantee ---

    #[test]
    fn ec4_all_within_guarantee_no_evictable() {
        let bc = BudgetController::new(8, Duration::from_secs(30));
        bc.register_pool("svc", cfg(5, 100, 5));
        bc.register_pool("batch", cfg(3, 50, 3));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("svc", now);
        }
        for _ in 0..3 {
            bc.try_acquire("batch", now);
        }

        bc.register_pool("analytics", cfg(0, 10, 5));
        assert_eq!(bc.try_acquire("analytics", now), AcquireResult::WouldBlock);

        let granted = bc.release("svc", now);
        assert_eq!(granted, Some("analytics".to_string()));
    }

    #[test]
    fn ec4_guaranteed_return_beats_above_guarantee_waiter() {
        let bc = BudgetController::new(8, Duration::from_secs(30));
        bc.register_pool("svc", cfg(5, 100, 5));
        bc.register_pool("batch", cfg(3, 50, 3));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("svc", now);
        }
        for _ in 0..3 {
            bc.try_acquire("batch", now);
        }

        bc.register_pool("analytics", cfg(0, 10, 5));
        bc.try_acquire("analytics", now);

        bc.release("svc", now);
        assert_eq!(bc.held("analytics"), 1);

        let result = bc.try_acquire("svc", now);
        assert_eq!(result, AcquireResult::WouldBlock);
    }

    // --- EC-5: Many dynamic users ---

    #[test]
    fn ec5_many_dynamic_users_round_robin() {
        let bc = BudgetController::new(5, Duration::from_secs(0));
        let now = Instant::now();

        for i in 0..10 {
            bc.register_pool(&format!("user_{}", i), cfg(0, 100, 5));
        }
        for i in 0..5 {
            assert_eq!(
                bc.try_acquire(&format!("user_{}", i), now),
                AcquireResult::Granted
            );
        }
        assert_eq!(bc.total_held(), 5);

        for i in 5..10 {
            assert_eq!(
                bc.try_acquire(&format!("user_{}", i), now),
                AcquireResult::WouldBlock
            );
        }

        let granted = bc.release("user_0", now);
        assert!(granted.is_some());
    }

    // --- EC-6: Guarantee overflow ---

    #[test]
    fn ec6_guarantee_overflow_detected() {
        let bc = BudgetController::new(10, Duration::from_secs(30));
        bc.register_pool("a", cfg(5, 100, 10));
        bc.register_pool("b", cfg(3, 50, 10));
        assert!(bc.validate_guarantees().is_ok());

        bc.register_pool("c", cfg(5, 10, 10));
        assert!(bc.validate_guarantees().is_err());
    }

    // --- EC-7: min_lifetime=0 ---

    #[test]
    fn ec7_min_lifetime_zero_allows_immediate_eviction() {
        let bc = BudgetController::new(5, Duration::from_secs(0));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("low", now);
        }

        let result = bc.try_acquire("high", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "low")
        );
    }

    // --- EC-8: Flap protection ---

    #[test]
    fn ec8_min_lifetime_protects_young_connections() {
        let bc = BudgetController::new(5, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("low", now);
        }

        assert_eq!(bc.try_acquire("high", now), AcquireResult::WouldBlock);
        assert_eq!(bc.held("low"), 5);
    }

    #[test]
    fn ec8_min_lifetime_allows_eviction_after_aging() {
        let bc = BudgetController::new(5, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));

        let old = Instant::now() - Duration::from_secs(60);
        bc.set_held_with_age("low", 5, old);

        let now = Instant::now();
        let result = bc.try_acquire("high", now);
        assert!(matches!(result, AcquireResult::GrantedAfterEviction { .. }));
        assert_eq!(bc.held("low"), 4);
        assert_eq!(bc.held("high"), 1);
    }

    // --- Weight competition ---

    #[test]
    fn higher_weight_evicts_lower_weight() {
        let bc = BudgetController::new(10, Duration::from_secs(0));
        bc.register_pool("svc", cfg(0, 100, 10));
        bc.register_pool("analytics", cfg(0, 10, 10));
        let now = Instant::now();

        for _ in 0..10 {
            bc.try_acquire("analytics", now);
        }

        let result = bc.try_acquire("svc", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "analytics")
        );
    }

    #[test]
    fn equal_weight_cannot_evict() {
        let bc = BudgetController::new(10, Duration::from_secs(0));
        bc.register_pool("a", cfg(0, 100, 10));
        bc.register_pool("b", cfg(0, 100, 10));
        let now = Instant::now();

        for _ in 0..10 {
            bc.try_acquire("a", now);
        }
        assert_eq!(bc.try_acquire("b", now), AcquireResult::WouldBlock);
    }

    #[test]
    fn eviction_targets_lowest_weight_first() {
        let bc = BudgetController::new(10, Duration::from_secs(0));
        bc.register_pool("high", cfg(0, 100, 10));
        bc.register_pool("mid", cfg(0, 50, 5));
        bc.register_pool("low", cfg(0, 10, 5));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("mid", now);
        }
        for _ in 0..5 {
            bc.try_acquire("low", now);
        }

        let result = bc.try_acquire("high", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "low")
        );
    }

    #[test]
    fn guaranteed_connections_never_evicted() {
        let bc = BudgetController::new(5, Duration::from_secs(0));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(5, 10, 5));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("low", now);
        }
        assert_eq!(bc.try_acquire("high", now), AcquireResult::WouldBlock);
    }

    #[test]
    fn denied_when_at_user_max() {
        let bc = BudgetController::new(100, Duration::from_secs(0));
        bc.register_pool("user", cfg(0, 100, 3));
        let now = Instant::now();

        for _ in 0..3 {
            bc.try_acquire("user", now);
        }
        assert_eq!(bc.try_acquire("user", now), AcquireResult::DeniedUserMax);
    }

    // --- Tie-breaker ---

    #[test]
    fn tiebreaker_most_waiting_wins() {
        let bc = BudgetController::new(1, Duration::from_secs(0));
        bc.register_pool("a", cfg(0, 100, 5));
        bc.register_pool("b", cfg(0, 100, 5));
        let now = Instant::now();

        bc.try_acquire("a", now);
        bc.try_acquire("b", now); // WouldBlock, b.waiting=1
        bc.try_acquire("b", now); // WouldBlock, b.waiting=2
        bc.try_acquire("a", now); // WouldBlock, a.waiting=1

        let granted = bc.release("a", now);
        assert_eq!(granted, Some("b".to_string()));
    }

    // --- Above guarantee + eviction ---

    #[test]
    fn above_guarantee_yields_to_higher_weight_waiter() {
        let bc = BudgetController::new(1, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));
        let now = Instant::now();

        bc.try_acquire("low", now);
        bc.try_acquire("low", now);
        bc.try_acquire("high", now);

        let granted = bc.release("low", now);
        assert_eq!(granted, Some("high".to_string()));
    }

    #[test]
    fn above_guarantee_request_evicts_when_pool_full() {
        let bc = BudgetController::new(2, Duration::from_secs(0));
        bc.register_pool("high", cfg(0, 100, 2));
        bc.register_pool("low", cfg(0, 10, 2));
        let now = Instant::now();

        bc.try_acquire("low", now);
        bc.try_acquire("high", now);

        let result = bc.try_acquire("high", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "low")
        );
        assert_eq!(bc.held("high"), 2);
        assert_eq!(bc.held("low"), 0);
    }

    // --- MAJOR-3 fix: unregister_pool drains waiters ---

    #[test]
    fn unregister_pool_schedules_remaining_waiters() {
        let bc = BudgetController::new(5, Duration::from_secs(30));
        bc.register_pool("filler", cfg(0, 100, 5));
        bc.register_pool("waiter_a", cfg(0, 100, 5));
        bc.register_pool("waiter_b", cfg(0, 100, 5));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("filler", now);
        }
        bc.try_acquire("waiter_a", now);
        bc.try_acquire("waiter_b", now);

        bc.unregister_pool("filler", now);

        assert_eq!(bc.held("waiter_a"), 1);
        assert_eq!(bc.held("waiter_b"), 1);
        assert_eq!(bc.waiting("waiter_a"), 0);
        assert_eq!(bc.waiting("waiter_b"), 0);
        assert_eq!(bc.total_held(), 2);
    }

    #[test]
    fn unregister_pool_prevents_deadlock() {
        let bc = BudgetController::new(10, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("filler", cfg(0, 100, 10));
        let now = Instant::now();

        for _ in 0..10 {
            bc.try_acquire("filler", now);
        }
        bc.try_acquire("high", now);

        bc.unregister_pool("filler", now);

        assert_eq!(bc.held("high"), 1);
        assert_eq!(bc.total_held(), 1);
        assert_eq!(bc.try_acquire("high", now), AcquireResult::Granted);
        assert_eq!(bc.held("high"), 2);
    }

    // --- Waiter deduplication ---

    #[test]
    fn multiple_waiters_same_pool_counted_correctly() {
        let bc = BudgetController::new(1, Duration::from_secs(30));
        bc.register_pool("holder", cfg(0, 100, 5));
        bc.register_pool("waiter", cfg(0, 100, 5));
        let now = Instant::now();

        bc.try_acquire("holder", now);
        bc.try_acquire("waiter", now);
        bc.try_acquire("waiter", now);
        bc.try_acquire("waiter", now);
        assert_eq!(bc.waiting("waiter"), 3);

        bc.release("holder", now);
        assert_eq!(bc.held("waiter"), 1);
        assert_eq!(bc.waiting("waiter"), 2);

        bc.release("waiter", now);
        assert_eq!(bc.held("waiter"), 1);
        assert_eq!(bc.waiting("waiter"), 1);
    }
}
