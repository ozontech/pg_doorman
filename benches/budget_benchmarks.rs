use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::{Duration, Instant};

use pg_doorman::pool::budget::{AcquireResult, BudgetController, PoolBudgetConfig};

fn cfg(guaranteed: u32, weight: u32, max: u32) -> PoolBudgetConfig {
    PoolBudgetConfig {
        guaranteed,
        weight,
        max_pool_size: max,
    }
}

/// Hot path: acquire within guarantee, pool not full.
fn bench_acquire_guaranteed(c: &mut Criterion) {
    let mut group = c.benchmark_group("acquire_guaranteed");

    for pool_count in [4, 10, 50] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("pools", pool_count),
            &pool_count,
            |b, &count| {
                let bc = BudgetController::new(count * 10, Duration::from_secs(30));
                for i in 0..count {
                    bc.register_pool(&format!("pool_{}", i), cfg(5, 100, 10));
                }
                let now = Instant::now();
                b.iter(|| {
                    let result = bc.try_acquire("pool_0", now);
                    assert_eq!(result, AcquireResult::Granted);
                    bc.release("pool_0", now);
                });
            },
        );
    }

    group.finish();
}

/// Hot path: acquire above guarantee, pool not full, no competition.
fn bench_acquire_above_guarantee(c: &mut Criterion) {
    let mut group = c.benchmark_group("acquire_above_guarantee");

    let bc = BudgetController::new(100, Duration::from_secs(30));
    bc.register_pool("user", cfg(5, 100, 50));
    let now = Instant::now();
    // Fill guaranteed first
    for _ in 0..5 {
        bc.try_acquire("user", now);
    }

    group.throughput(Throughput::Elements(1));
    group.bench_function("no_competition", |b| {
        b.iter(|| {
            let result = bc.try_acquire("user", now);
            assert_eq!(result, AcquireResult::Granted);
            bc.release("user", now);
        });
    });

    group.finish();
}

/// Release + schedule with waiters.
fn bench_release_with_schedule(c: &mut Criterion) {
    let mut group = c.benchmark_group("release_with_schedule");

    for waiter_count in [1, 5, 20] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("waiters", waiter_count),
            &waiter_count,
            |b, &count| {
                b.iter_custom(|iters| {
                    let bc = BudgetController::new(1, Duration::from_secs(0));
                    for i in 0..count + 1 {
                        bc.register_pool(&format!("pool_{}", i), cfg(0, 100, 5));
                    }
                    let now = Instant::now();
                    bc.try_acquire("pool_0", now);
                    for i in 1..=count {
                        bc.try_acquire(&format!("pool_{}", i), now);
                    }

                    let start = Instant::now();
                    for _ in 0..iters {
                        bc.release("pool_0", now);
                        // Restore: the scheduled pool got a slot, release it
                        // and re-enqueue pool_0
                        for i in 1..=count {
                            let name = format!("pool_{}", i);
                            if bc.held(&name) > 0 {
                                bc.release(&name, now);
                                bc.try_acquire(&name, now);
                                break;
                            }
                        }
                        bc.try_acquire("pool_0", now);
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

/// Eviction: pool full, find and evict from lowest weight.
fn bench_eviction(c: &mut Criterion) {
    let mut group = c.benchmark_group("eviction");

    for pool_count in [4, 10, 50] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("pools", pool_count),
            &pool_count,
            |b, &count| {
                b.iter_custom(|iters| {
                    let bc = BudgetController::new(count, Duration::from_secs(0));
                    bc.register_pool("high", cfg(0, 100, count));
                    for i in 0..count - 1 {
                        bc.register_pool(&format!("low_{}", i), cfg(0, 10, 2));
                    }
                    let now = Instant::now();
                    // Fill pool with low-weight connections
                    for i in 0..count - 1 {
                        bc.try_acquire(&format!("low_{}", i), now);
                    }
                    bc.try_acquire("high", now); // takes last slot normally

                    // Now repeatedly: release high, low fills, high evicts
                    let start = Instant::now();
                    for _ in 0..iters {
                        bc.release("high", now);
                        // A low pool gets the slot via schedule (if any waiting)
                        // Force re-fill
                        bc.try_acquire(&format!("low_0"), now);
                        // high evicts
                        let result = bc.try_acquire("high", now);
                        assert!(matches!(result, AcquireResult::GrantedAfterEviction { .. }));
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_acquire_guaranteed,
    bench_acquire_above_guarantee,
    bench_release_with_schedule,
    bench_eviction,
);
criterion_main!(benches);
