use super::*;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

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

#[test]
fn reset_all_clears_state_and_schedules_waiters() {
    let bc = BudgetController::new(5, Duration::from_secs(30));
    bc.register_pool("a", cfg(0, 100, 5));
    bc.register_pool("b", cfg(0, 100, 5));
    bc.register_pool("c", cfg(0, 100, 5));
    let now = Instant::now();

    for _ in 0..5 {
        bc.try_acquire("a", now);
    }
    assert_eq!(bc.total_held(), 5);
    bc.try_acquire("b", now);
    bc.try_acquire("c", now);
    assert_eq!(bc.waiting("b"), 1);
    assert_eq!(bc.waiting("c"), 1);

    bc.reset_all(now);

    assert_eq!(bc.held("a"), 0);
    assert_eq!(bc.total_held(), 2);
    assert_eq!(bc.held("b"), 1);
    assert_eq!(bc.held("c"), 1);
    assert_eq!(bc.waiting("b"), 0);
    assert_eq!(bc.waiting("c"), 0);
    assert_eq!(bc.metrics().resets.load(Ordering::Relaxed), 1);
}

#[test]
fn reset_all_with_no_waiters() {
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.register_pool("a", cfg(0, 100, 10));
    let now = Instant::now();

    for _ in 0..10 {
        bc.try_acquire("a", now);
    }
    assert_eq!(bc.total_held(), 10);

    bc.reset_all(now);

    assert_eq!(bc.held("a"), 0);
    assert_eq!(bc.total_held(), 0);
}

#[test]
fn reconcile_fixes_drift_and_schedules_waiters() {
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.register_pool("drifted", cfg(0, 100, 15));
    bc.register_pool("waiter", cfg(0, 100, 5));
    let now = Instant::now();

    bc.set_held_with_age("drifted", 10, now);
    bc.try_acquire("waiter", now);
    assert_eq!(bc.waiting("waiter"), 1);

    bc.reconcile("drifted", 3, now);

    assert_eq!(bc.held("drifted"), 3);
    assert_eq!(bc.total_held(), 4);
    assert_eq!(bc.held("waiter"), 1);
    assert_eq!(bc.waiting("waiter"), 0);
    assert_eq!(bc.metrics().reconciliations.load(Ordering::Relaxed), 1);
}

#[test]
fn reconcile_no_op_when_counters_match() {
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.register_pool("pool", cfg(0, 100, 10));
    let now = Instant::now();

    for _ in 0..5 {
        bc.try_acquire("pool", now);
    }

    bc.reconcile("pool", 5, now);

    assert_eq!(bc.held("pool"), 5);
    assert_eq!(bc.metrics().reconciliations.load(Ordering::Relaxed), 0);
}

#[test]
fn reconcile_upward_drift() {
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.register_pool("pool", cfg(0, 100, 10));
    let now = Instant::now();

    for _ in 0..3 {
        bc.try_acquire("pool", now);
    }
    assert_eq!(bc.held("pool"), 3);

    bc.reconcile("pool", 7, now);

    assert_eq!(bc.held("pool"), 7);
    assert_eq!(bc.total_held(), 7);
}

#[test]
fn reconcile_unknown_pool_is_no_op() {
    let bc = BudgetController::new(10, Duration::from_secs(30));
    let now = Instant::now();

    bc.reconcile("nonexistent", 5, now);

    assert_eq!(bc.total_held(), 0);
    assert_eq!(bc.metrics().reconciliations.load(Ordering::Relaxed), 0);
}

#[test]
fn cancel_wait_decrements_waiting() {
    let bc = BudgetController::new(1, Duration::from_secs(30));
    bc.register_pool("holder", cfg(0, 100, 5));
    bc.register_pool("waiter", cfg(0, 100, 5));
    let now = Instant::now();

    bc.try_acquire("holder", now);
    bc.try_acquire("waiter", now);
    bc.try_acquire("waiter", now);
    bc.try_acquire("waiter", now);
    assert_eq!(bc.waiting("waiter"), 3);

    bc.cancel_wait("waiter");
    assert_eq!(bc.waiting("waiter"), 2);
    assert_eq!(bc.metrics().denied_timeout.load(Ordering::Relaxed), 1);

    bc.cancel_wait("waiter");
    assert_eq!(bc.waiting("waiter"), 1);

    bc.cancel_wait("waiter");
    assert_eq!(bc.waiting("waiter"), 0);
}

#[test]
fn cancel_wait_on_unknown_pool_is_no_op() {
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.cancel_wait("nonexistent");
    assert_eq!(bc.metrics().denied_timeout.load(Ordering::Relaxed), 0);
}

#[test]
fn cancel_wait_on_pool_with_zero_waiting_is_no_op() {
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.register_pool("pool", cfg(0, 100, 10));

    bc.cancel_wait("pool");
    assert_eq!(bc.waiting("pool"), 0);
    assert_eq!(bc.metrics().denied_timeout.load(Ordering::Relaxed), 0);
}

#[test]
fn metrics_track_all_acquire_outcomes() {
    let bc = BudgetController::new(2, Duration::from_secs(0));
    bc.register_pool("high", cfg(1, 100, 2));
    bc.register_pool("low", cfg(0, 10, 2));
    let now = Instant::now();

    bc.try_acquire("high", now);
    assert_eq!(bc.metrics().grants_guaranteed.load(Ordering::Relaxed), 1);

    bc.try_acquire("high", now);
    assert_eq!(bc.metrics().grants_above.load(Ordering::Relaxed), 1);

    let result = bc.try_acquire("low", now);
    assert_eq!(result, AcquireResult::WouldBlock);
    assert_eq!(bc.metrics().would_block.load(Ordering::Relaxed), 1);

    bc.try_acquire("nonexistent", now);
    assert_eq!(bc.metrics().denied_unknown.load(Ordering::Relaxed), 1);

    bc.register_pool("maxed", cfg(0, 100, 0));
    bc.try_acquire("maxed", now);
    assert_eq!(bc.metrics().denied_user_max.load(Ordering::Relaxed), 1);
}

#[test]
fn metrics_track_eviction_and_release() {
    let bc = BudgetController::new(5, Duration::from_secs(0));
    bc.register_pool("high", cfg(0, 100, 5));
    bc.register_pool("low", cfg(0, 10, 5));
    let now = Instant::now();

    for _ in 0..5 {
        bc.try_acquire("low", now);
    }

    bc.try_acquire("high", now);
    assert_eq!(bc.metrics().evictions.load(Ordering::Relaxed), 1);
    assert_eq!(
        bc.metrics().grants_after_eviction.load(Ordering::Relaxed),
        1
    );

    bc.release("high", now);
    assert_eq!(bc.metrics().releases.load(Ordering::Relaxed), 1);
}

#[test]
fn metrics_track_evictions_blocked_by_min_lifetime() {
    let bc = BudgetController::new(5, Duration::from_secs(30));
    bc.register_pool("high", cfg(0, 100, 5));
    bc.register_pool("low", cfg(0, 10, 5));
    let now = Instant::now();

    for _ in 0..5 {
        bc.try_acquire("low", now);
    }

    bc.try_acquire("high", now);
    assert!(bc.metrics().evictions_blocked.load(Ordering::Relaxed) >= 1);
}

// --- SHOW BUDGET / SHOW BUDGET_POOLS ---

#[test]
fn show_budget_row_reflects_state() {
    let bc = BudgetController::new(20, Duration::from_secs(30));
    bc.register_pool("svc", cfg(5, 100, 15));
    bc.register_pool("batch", cfg(3, 50, 10));
    let now = Instant::now();

    for _ in 0..8 {
        bc.try_acquire("svc", now);
    }
    for _ in 0..5 {
        bc.try_acquire("batch", now);
    }

    let row = bc.show_budget_row();
    assert_eq!(row[0], "20"); // max_connections
    assert_eq!(row[1], "13"); // total_held
    assert_eq!(row[2], "0"); // total_waiting
    assert_eq!(row[3], "2"); // pools_registered
    assert_eq!(row[4], "30000"); // min_lifetime_ms
}

#[test]
fn show_budget_pools_rows_sorted_and_complete() {
    let bc = BudgetController::new(20, Duration::from_secs(30));
    bc.register_pool("svc", cfg(5, 100, 15));
    bc.register_pool("analytics", cfg(0, 10, 5));
    let now = Instant::now();

    for _ in 0..8 {
        bc.try_acquire("svc", now);
    }
    for _ in 0..3 {
        bc.try_acquire("analytics", now);
    }

    let rows = bc.show_budget_pools_rows();
    assert_eq!(rows.len(), 2);

    // Sorted by name: analytics first
    assert_eq!(rows[0][0], "analytics");
    assert_eq!(rows[0][1], "0"); // guaranteed
    assert_eq!(rows[0][2], "10"); // weight
    assert_eq!(rows[0][3], "5"); // max
    assert_eq!(rows[0][4], "3"); // held
    assert_eq!(rows[0][5], "3"); // above_guarantee
    assert_eq!(rows[0][6], "0"); // waiting
    assert_eq!(rows[0][7], "0"); // is_waiter

    assert_eq!(rows[1][0], "svc");
    assert_eq!(rows[1][4], "8"); // held
    assert_eq!(rows[1][5], "3"); // above_guarantee (8-5)
}

#[test]
fn show_budget_pools_shows_waiters() {
    let bc = BudgetController::new(5, Duration::from_secs(30));
    bc.register_pool("holder", cfg(0, 100, 5));
    bc.register_pool("waiter", cfg(0, 100, 5));
    let now = Instant::now();

    for _ in 0..5 {
        bc.try_acquire("holder", now);
    }
    bc.try_acquire("waiter", now); // WouldBlock
    bc.try_acquire("waiter", now); // WouldBlock

    let rows = bc.show_budget_pools_rows();
    let waiter_row = rows.iter().find(|r| r[0] == "waiter").unwrap();
    assert_eq!(waiter_row[6], "2"); // waiting=2
    assert_eq!(waiter_row[7], "1"); // is_waiter=true
}

#[test]
fn show_budget_header_matches_row_length() {
    let header = BudgetController::show_budget_header();
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.register_pool("test", cfg(0, 100, 5));
    let row = bc.show_budget_row();
    assert_eq!(header.len(), row.len());
}

#[test]
fn show_budget_pools_header_matches_row_length() {
    let header = BudgetController::show_budget_pools_header();
    let bc = BudgetController::new(10, Duration::from_secs(30));
    bc.register_pool("test", cfg(0, 100, 5));
    let now = Instant::now();
    bc.try_acquire("test", now);
    let rows = bc.show_budget_pools_rows();
    assert_eq!(header.len(), rows[0].len());
}

// --- DBA-1: RAII guard for CREATE failure rollback (Contract 3) ---

#[test]
fn acquire_guard_auto_releases_on_drop() {
    let bc = BudgetController::new(5, Duration::from_secs(30));
    bc.register_pool("svc", cfg(0, 100, 5));
    let now = Instant::now();

    // Simulate: acquire succeeds, but CREATE fails → guard drops
    {
        let guard = bc.try_acquire_guard("svc", now);
        assert!(guard.is_some());
        assert_eq!(bc.held("svc"), 1);
        assert_eq!(bc.total_held(), 1);
        // guard drops here without confirm() → release
    }
    assert_eq!(bc.held("svc"), 0);
    assert_eq!(bc.total_held(), 0);
}

#[test]
fn acquire_guard_confirm_keeps_slot() {
    let bc = BudgetController::new(5, Duration::from_secs(30));
    bc.register_pool("svc", cfg(0, 100, 5));
    let now = Instant::now();

    {
        let guard = bc.try_acquire_guard("svc", now);
        assert!(guard.is_some());
        guard.unwrap().confirm(); // CREATE succeeded, keep the slot
    }
    assert_eq!(bc.held("svc"), 1); // slot kept
    assert_eq!(bc.total_held(), 1);
}

#[test]
fn acquire_guard_denied_returns_none() {
    let bc = BudgetController::new(1, Duration::from_secs(30));
    bc.register_pool("svc", cfg(0, 100, 1));
    let now = Instant::now();

    bc.try_acquire("svc", now); // takes the 1 slot
    let guard = bc.try_acquire_guard("svc", now); // DeniedUserMax
    assert!(guard.is_none());
    assert_eq!(bc.held("svc"), 1); // unchanged
}

// --- DBA-2: set_max_connections for maintenance windows ---

#[test]
fn set_max_connections_shrinks_budget() {
    let bc = BudgetController::new(10, Duration::from_secs(0));
    bc.register_pool("svc", cfg(0, 100, 10));
    let now = Instant::now();

    for _ in 0..8 {
        bc.try_acquire("svc", now);
    }
    assert_eq!(bc.total_held(), 8);

    // DBA lowers PG max_connections during maintenance
    bc.set_max_connections(5, now);
    assert_eq!(bc.max_connections(), 5);

    // New acquires should be denied (8 > 5, pool over budget)
    assert_eq!(bc.try_acquire("svc", now), AcquireResult::WouldBlock);
}

#[test]
fn set_max_connections_grows_budget_serves_waiters() {
    let bc = BudgetController::new(2, Duration::from_secs(30));
    bc.register_pool("a", cfg(0, 100, 5));
    bc.register_pool("b", cfg(0, 100, 5));
    let now = Instant::now();

    bc.try_acquire("a", now);
    bc.try_acquire("b", now);
    // Pool full (2/2). a wants more.
    bc.try_acquire("a", now); // WouldBlock
    assert_eq!(bc.waiting("a"), 1);

    // DBA grows budget
    bc.set_max_connections(5, now);
    // Waiter should be served immediately
    assert_eq!(bc.held("a"), 2);
    assert_eq!(bc.waiting("a"), 0);
}
