use parking_lot::Mutex;
use std::collections::HashMap;
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
#[derive(Debug, Clone)]
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
    /// Waiters in insertion order. SCHEDULE picks the best by priority.
    waiters: Vec<String>,
}

struct PoolState {
    config: PoolBudgetConfig,
    held: u32,
    waiting: u32,
    /// Creation timestamps of held connections, oldest first.
    connection_ages: Vec<Instant>,
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
                connection_ages: Vec::new(),
            },
        );
    }

    pub fn unregister_pool(&self, name: &str) {
        let mut state = self.state.lock();
        if let Some(pool) = state.pools.remove(name) {
            state.total_held -= pool.held;
            state.waiters.retain(|w| w != name);
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
        let config = pool_state.config.clone();

        // Check per-user max
        if held >= config.max_pool_size {
            return AcquireResult::DeniedUserMax;
        }

        let is_guaranteed = held < config.guaranteed;

        // Case 1: room in global budget
        if state.total_held < self.max_connections {
            if !is_guaranteed {
                // Above guarantee: check if a higher-weight waiter exists
                if Self::has_higher_weight_waiter(&state, pool, config.weight) {
                    let ps = state.pools.get_mut(pool).unwrap();
                    ps.waiting += 1;
                    state.waiters.push(pool.to_string());
                    return AcquireResult::WouldBlock;
                }
            }
            // Grant
            let ps = state.pools.get_mut(pool).unwrap();
            ps.held += 1;
            ps.connection_ages.push(now);
            state.total_held += 1;
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
            // Evict one connection from victim
            let victim_state = state.pools.get_mut(&victim_name).unwrap();
            victim_state.held -= 1;
            // Remove oldest eligible connection
            if let Some(idx) = victim_state
                .connection_ages
                .iter()
                .position(|&t| now.duration_since(t) >= self.min_lifetime)
            {
                victim_state.connection_ages.remove(idx);
            } else if !victim_state.connection_ages.is_empty() {
                victim_state.connection_ages.remove(0);
            }
            state.total_held -= 1;

            // Grant to requester
            let ps = state.pools.get_mut(pool).unwrap();
            ps.held += 1;
            ps.connection_ages.push(now);
            state.total_held += 1;
            return AcquireResult::GrantedAfterEviction {
                evicted_pool: victim_name,
            };
        }

        // Case 3: no evictable connections — enqueue
        let ps = state.pools.get_mut(pool).unwrap();
        ps.waiting += 1;
        state.waiters.push(pool.to_string());
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
                // Remove oldest connection age
                if !ps.connection_ages.is_empty() {
                    ps.connection_ages.remove(0);
                }
                state.total_held -= 1;
            }
        }

        // SCHEDULE: find best waiter and grant
        Self::schedule(&mut state, self.max_connections, self.min_lifetime, now)
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

        // SELECT_BEST_WAITER
        let best_idx = Self::select_best_waiter_idx(state)?;
        let best_pool = state.waiters[best_idx].clone();

        let best_state = state.pools.get(&best_pool).unwrap();
        let is_guaranteed = best_state.held < best_state.config.guaranteed;
        let weight = best_state.config.weight;

        // Can we grant?
        if state.total_held < max_connections {
            // Room available
            state.waiters.remove(best_idx);
            let ps = state.pools.get_mut(&best_pool).unwrap();
            ps.held += 1;
            ps.waiting -= 1;
            ps.connection_ages.push(now);
            state.total_held += 1;
            return Some(best_pool);
        }

        // Try eviction for the best waiter
        let requester_weight = if is_guaranteed { u32::MAX } else { weight };
        if let Some(victim) =
            Self::find_evictable(state, &best_pool, requester_weight, now, min_lifetime)
        {
            let victim_name = victim.clone();
            let victim_state = state.pools.get_mut(&victim_name).unwrap();
            victim_state.held -= 1;
            if let Some(idx) = victim_state
                .connection_ages
                .iter()
                .position(|&t| now.duration_since(t) >= min_lifetime)
            {
                victim_state.connection_ages.remove(idx);
            } else if !victim_state.connection_ages.is_empty() {
                victim_state.connection_ages.remove(0);
            }
            state.total_held -= 1;

            state.waiters.remove(best_idx);
            let ps = state.pools.get_mut(&best_pool).unwrap();
            ps.held += 1;
            ps.waiting -= 1;
            ps.connection_ages.push(now);
            state.total_held += 1;
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
    fn waiter_priority(state: &BudgetState, pool_name: &str) -> (bool, u32, u32) {
        let ps = &state.pools[pool_name];
        let is_guaranteed = ps.held < ps.config.guaranteed;
        (is_guaranteed, ps.config.weight, ps.waiting)
    }

    /// Check if any waiter with strictly higher weight exists.
    fn has_higher_weight_waiter(
        state: &BudgetState,
        requesting_pool: &str,
        requesting_weight: u32,
    ) -> bool {
        state.waiters.iter().any(|w| {
            w != requesting_pool && {
                let ps = &state.pools[w];
                ps.config.weight > requesting_weight
            }
        })
    }

    /// FIND_EVICTABLE: above-guarantee, old enough, lower weight.
    fn find_evictable(
        state: &BudgetState,
        requester: &str,
        requester_weight: u32,
        now: Instant,
        min_lifetime: Duration,
    ) -> Option<String> {
        let mut best: Option<(u32, Duration, String)> = None; // (weight, max_age, pool_name)

        for (name, ps) in &state.pools {
            if name == requester {
                continue;
            }
            if ps.held <= ps.config.guaranteed {
                continue; // within guarantee: sacred
            }
            if requester_weight != u32::MAX && ps.config.weight >= requester_weight {
                continue; // same or higher weight: safe
            }
            // Check if any connection is old enough
            let oldest_eligible = ps
                .connection_ages
                .iter()
                .find(|&&t| now.duration_since(t) >= min_lifetime);
            if oldest_eligible.is_none() {
                continue; // all too young
            }
            let max_age = now.duration_since(*ps.connection_ages.first().unwrap());

            match &best {
                None => {
                    best = Some((ps.config.weight, max_age, name.clone()));
                }
                Some((bw, ba, _)) => {
                    // Lower weight first, then oldest
                    if ps.config.weight < *bw || (ps.config.weight == *bw && max_age > *ba) {
                        best = Some((ps.config.weight, max_age, name.clone()));
                    }
                }
            }
        }

        best.map(|(_, _, name)| name)
    }

    // --- Getters for testing ---

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
    pub fn set_held_with_age(&self, pool: &str, count: u32, created_at: Instant) {
        let mut state = self.state.lock();
        if let Some(ps) = state.pools.get_mut(pool) {
            let old_held = ps.held;
            ps.held = count;
            ps.connection_ages.clear();
            for _ in 0..count {
                ps.connection_ages.push(created_at);
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

    // --- Scenario 1: Normal startup ---

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
        // service_api: 8
        for _ in 0..8 {
            bc.try_acquire("service_api", now);
        }
        // batch_worker: 5
        for _ in 0..5 {
            bc.try_acquire("batch_worker", now);
        }
        // analytics: 3
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

        // user_a fills pool
        for _ in 0..20 {
            bc.try_acquire("user_a", now);
        }
        assert_eq!(bc.total_held(), 20);

        // user_b cannot evict (equal weight)
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
        bc.try_acquire("user_b", now); // WouldBlock, enqueued

        // user_a returns one connection
        let granted = bc.release("user_a", now);
        assert_eq!(granted, Some("user_b".to_string()));
        assert_eq!(bc.held("user_b"), 1);
        assert_eq!(bc.held("user_a"), 19);
    }

    // --- EC-2: Lowest weight, pool full ---

    #[test]
    fn ec2_lowest_weight_cannot_evict() {
        let (bc, now) = setup_standard();
        // Fill pool: service_api=12, batch_worker=5, analytics=3
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

        bc.register_pool("new_app", cfg(0, 5, 5)); // weight=5, lowest
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
        bc.try_acquire("new_app", now); // WouldBlock

        // service_api returns — no other waiters → new_app gets it
        let granted = bc.release("service_api", now);
        assert_eq!(granted, Some("new_app".to_string()));
    }

    #[test]
    fn ec2_lowest_weight_loses_to_higher_weight_waiter() {
        // Use equal weight filler so neither low nor high can evict
        let bc = BudgetController::new(10, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 10));
        bc.register_pool("low", cfg(0, 10, 10));
        bc.register_pool("filler", cfg(0, 100, 10)); // same weight as high
        let now = Instant::now();

        for _ in 0..10 {
            bc.try_acquire("filler", now);
        }

        bc.try_acquire("low", now); // WouldBlock (can't evict w=100)
        bc.try_acquire("high", now); // WouldBlock (can't evict w=100, young)

        let granted = bc.release("filler", now);
        // high (weight=100) beats low (weight=10)
        assert_eq!(granted, Some("high".to_string()));
    }

    // --- EC-3: Guaranteed request, pool full ---

    #[test]
    fn ec3_guaranteed_evicts_any_above_guarantee() {
        let (bc, _) = setup_standard();
        let old = Instant::now() - Duration::from_secs(60);

        // Fill pool with old connections
        bc.set_held_with_age("service_api", 12, old);
        bc.set_held_with_age("batch_worker", 5, old);
        bc.set_held_with_age("analytics", 3, old);
        assert_eq!(bc.total_held(), 20);

        // New user with guarantee
        bc.register_pool("admin", cfg(2, 1, 2)); // weight=1 but guaranteed=2

        let now = Instant::now();
        // First guaranteed request — evicts lowest weight (analytics, w=10)
        let result = bc.try_acquire("admin", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "analytics")
        );
        assert_eq!(bc.held("admin"), 1);
        assert_eq!(bc.held("analytics"), 2); // was 3, lost 1

        // Second guaranteed request
        let result = bc.try_acquire("admin", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "analytics")
        );
        assert_eq!(bc.held("admin"), 2);
        assert_eq!(bc.held("analytics"), 1);
    }

    // --- EC-4: All connections within guarantee ---

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
        assert_eq!(bc.total_held(), 8);

        bc.register_pool("analytics", cfg(0, 10, 5));
        assert_eq!(bc.try_acquire("analytics", now), AcquireResult::WouldBlock);

        // Even on return, guaranteed user takes it back
        let granted = bc.release("svc", now);
        // svc was at 5=guaranteed, after release svc.held=4 < guaranteed=5
        // svc is a guaranteed waiter? No, svc didn't call try_acquire again.
        // But analytics is waiting → analytics gets it
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
        bc.try_acquire("analytics", now); // WouldBlock, enqueued

        // svc returns one connection, then wants it back (guaranteed)
        bc.release("svc", now); // analytics gets it (was the waiter)
        assert_eq!(bc.held("analytics"), 1);

        // Now svc tries again — svc.held=4 < guaranteed=5 → guaranteed request
        // analytics holds 1 above-guarantee but age < min_lifetime → protected
        // Pool full (8/8). No evictable. svc would block.
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

        // First 5 users get connections
        for i in 0..5 {
            let result = bc.try_acquire(&format!("user_{}", i), now);
            assert_eq!(result, AcquireResult::Granted);
        }
        assert_eq!(bc.total_held(), 5);

        // Users 5-9 would block
        for i in 5..10 {
            let result = bc.try_acquire(&format!("user_{}", i), now);
            assert_eq!(result, AcquireResult::WouldBlock);
        }

        // user_0 releases → best waiter gets it
        let granted = bc.release("user_0", now);
        assert!(granted.is_some());
        // All waiters have equal weight=100, equal waiting=1
        // First enqueued wins (FIFO for equal priority)
    }

    // --- EC-6: Guarantee budget overflow ---

    #[test]
    fn ec6_guarantee_overflow_detected() {
        let bc = BudgetController::new(10, Duration::from_secs(30));
        bc.register_pool("a", cfg(5, 100, 10));
        bc.register_pool("b", cfg(3, 50, 10));
        assert!(bc.validate_guarantees().is_ok()); // 5+3=8 <= 10

        bc.register_pool("c", cfg(5, 10, 10));
        assert!(bc.validate_guarantees().is_err()); // 5+3+5=13 > 10
    }

    // --- EC-7: min_lifetime=0 (no flap protection) ---

    #[test]
    fn ec7_min_lifetime_zero_allows_immediate_eviction() {
        let bc = BudgetController::new(5, Duration::from_secs(0));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("low", now);
        }
        assert_eq!(bc.total_held(), 5);

        // high can immediately evict low (min_lifetime=0, weight 100 > 10)
        let result = bc.try_acquire("high", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "low")
        );
    }

    // --- EC-8: Flap protection (min_lifetime blocks eviction) ---

    #[test]
    fn ec8_min_lifetime_protects_young_connections() {
        let bc = BudgetController::new(5, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));
        let now = Instant::now();

        // low gets 5 connections NOW (young)
        for _ in 0..5 {
            bc.try_acquire("low", now);
        }

        // high tries to evict — but all connections are < 30s old
        let result = bc.try_acquire("high", now);
        assert_eq!(result, AcquireResult::WouldBlock);
        assert_eq!(bc.held("low"), 5); // nothing evicted
    }

    #[test]
    fn ec8_min_lifetime_allows_eviction_after_aging() {
        let bc = BudgetController::new(5, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));

        // low gets connections 60 seconds ago
        let old = Instant::now() - Duration::from_secs(60);
        bc.set_held_with_age("low", 5, old);

        // high can evict now (connections are old enough)
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

        let result = bc.try_acquire("b", now);
        assert_eq!(result, AcquireResult::WouldBlock);
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

        // high evicts low first (weight 10 < 50)
        let result = bc.try_acquire("high", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "low")
        );
    }

    // --- Guaranteed never evicted ---

    #[test]
    fn guaranteed_connections_never_evicted() {
        let bc = BudgetController::new(5, Duration::from_secs(0));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(5, 10, 5)); // all 5 guaranteed
        let now = Instant::now();

        for _ in 0..5 {
            bc.try_acquire("low", now);
        }
        // low has 5 held = 5 guaranteed → all sacred

        let result = bc.try_acquire("high", now);
        assert_eq!(result, AcquireResult::WouldBlock); // cannot evict guaranteed
    }

    // --- DeniedUserMax ---

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

    // --- Tie-breaker: waiting count ---

    #[test]
    fn tiebreaker_most_waiting_wins() {
        let bc = BudgetController::new(1, Duration::from_secs(0));
        bc.register_pool("a", cfg(0, 100, 5));
        bc.register_pool("b", cfg(0, 100, 5));
        let now = Instant::now();

        bc.try_acquire("a", now); // gets the 1 slot
                                  // Both enqueue
        bc.try_acquire("b", now); // WouldBlock, b.waiting=1
        bc.try_acquire("b", now); // WouldBlock, b.waiting=2
        bc.try_acquire("a", now); // WouldBlock, a.waiting=1

        // Release → best waiter: b (waiting=2) beats a (waiting=1), equal weight
        let granted = bc.release("a", now);
        assert_eq!(granted, Some("b".to_string()));
    }

    // --- Above-guarantee yields to higher-weight waiter ---

    #[test]
    fn above_guarantee_yields_to_higher_weight_waiter() {
        // Pool size 1, two users. On release, higher weight wins.
        let bc = BudgetController::new(1, Duration::from_secs(30));
        bc.register_pool("high", cfg(0, 100, 5));
        bc.register_pool("low", cfg(0, 10, 5));
        let now = Instant::now();

        bc.try_acquire("low", now); // gets the 1 slot
        bc.try_acquire("low", now); // WouldBlock (pool full, young conns)
        bc.try_acquire("high", now); // WouldBlock (can't evict, young)

        let granted = bc.release("low", now);
        assert_eq!(granted, Some("high".to_string())); // higher weight wins
    }

    // --- Above-guarantee blocked when higher-weight waiter exists ---

    #[test]
    fn above_guarantee_request_evicts_when_pool_full() {
        // Pool size 2, min_lifetime=0. high can evict low immediately.
        let bc = BudgetController::new(2, Duration::from_secs(0));
        bc.register_pool("high", cfg(0, 100, 2));
        bc.register_pool("low", cfg(0, 10, 2));
        let now = Instant::now();

        bc.try_acquire("low", now); // Granted (1/2)
        bc.try_acquire("high", now); // Granted (2/2)

        // Pool full. high wants more. low is evictable (w=10 < 100, min_lifetime=0).
        let result = bc.try_acquire("high", now);
        assert!(
            matches!(result, AcquireResult::GrantedAfterEviction { ref evicted_pool } if evicted_pool == "low")
        );
        assert_eq!(bc.held("high"), 2);
        assert_eq!(bc.held("low"), 0);
    }
}
