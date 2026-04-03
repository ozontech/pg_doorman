use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::Arc;
use tokio::sync::Semaphore;

use pg_doorman::pool::pool_coordinator::{CoordinatorConfig, EvictionSource, PoolCoordinator};

struct NoOpEviction;
impl EvictionSource for NoOpEviction {
    fn try_evict_one(&self, _user: &str) -> bool {
        false
    }
    fn queued_clients(&self, _user: &str) -> usize {
        0
    }
    fn is_starving(&self, _user: &str) -> bool {
        false
    }
}

fn make_config(max: usize) -> CoordinatorConfig {
    CoordinatorConfig {
        max_db_connections: max,
        min_connection_lifetime_ms: 5000,
        reserve_pool_size: 0,
        reserve_pool_timeout_ms: 100,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

/// Baseline: raw tokio Semaphore try_acquire + forget.
/// This is the absolute floor — the cost of the atomic CAS that
/// PoolCoordinator adds to the hot path.
fn baseline_semaphore(c: &mut Criterion) {
    let mut group = c.benchmark_group("baseline_semaphore");
    group.throughput(Throughput::Elements(1));

    for &permits in &[10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::new("try_acquire_forget", permits),
            &permits,
            |b, &permits| {
                let sem = Arc::new(Semaphore::new(permits));
                b.iter(|| {
                    let p = sem.try_acquire().unwrap();
                    p.forget();
                    sem.add_permits(1);
                });
            },
        );
    }

    group.finish();
}

/// PoolCoordinator::try_acquire() happy path — low contention.
/// Measures: Arc clone + Semaphore try_acquire + AtomicUsize increment + struct alloc.
fn coordinator_happy_path(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("coordinator_happy_path");
    group.throughput(Throughput::Elements(1));

    for &max in &[10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::new("try_acquire_drop", max),
            &max,
            |b, &max| {
                let coord = rt.block_on(async {
                    PoolCoordinator::new("bench_db".to_string(), make_config(max))
                });

                b.iter(|| {
                    let permit = coord.try_acquire().unwrap();
                    drop(permit);
                });
            },
        );
    }

    group.finish();
}

/// Full round-trip: try_acquire + drop (permit lifecycle).
/// Measures: acquire overhead + RAII drop overhead (semaphore add_permits + Notify + atomic dec).
fn permit_lifecycle(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("permit_lifecycle");
    group.throughput(Throughput::Elements(1));

    let coord =
        rt.block_on(async { PoolCoordinator::new("bench_db".to_string(), make_config(100)) });

    group.bench_function("acquire_drop_cycle", |b| {
        b.iter(|| {
            let permit = coord.try_acquire().unwrap();
            std::hint::black_box(&permit);
            drop(permit);
        });
    });

    group.finish();
}

/// Contention: multiple threads compete for permits via try_acquire.
/// Simulates the hot path under real pooler load.
fn coordinator_contention(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("coordinator_contention");
    group.throughput(Throughput::Elements(1));
    group.sample_size(50);

    for &(tasks, permits) in &[(4, 100), (8, 100), (16, 100), (32, 100)] {
        group.bench_with_input(
            BenchmarkId::new(format!("{tasks}t"), permits),
            &(tasks, permits),
            |b, &(tasks, permits)| {
                let coord = rt.block_on(async {
                    PoolCoordinator::new("bench_db".to_string(), make_config(permits))
                });

                b.iter(|| {
                    rt.block_on(async {
                        let mut handles = Vec::with_capacity(tasks);
                        for _ in 0..tasks {
                            let c = coord.clone();
                            handles.push(tokio::spawn(async move {
                                for _ in 0..100 {
                                    let permit = c.try_acquire().unwrap();
                                    tokio::task::yield_now().await;
                                    drop(permit);
                                }
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                    });
                });
            },
        );
    }

    group.finish();
}

/// Full acquire path (async) — no contention.
/// Measures overhead of the async acquire vs sync try_acquire.
fn coordinator_async_acquire(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("coordinator_async_acquire");
    group.throughput(Throughput::Elements(1));

    let coord =
        rt.block_on(async { PoolCoordinator::new("bench_db".to_string(), make_config(100)) });
    let eviction = NoOpEviction;

    group.bench_function("acquire_no_contention", |b| {
        b.iter(|| {
            rt.block_on(async {
                let permit = coord
                    .acquire("bench_db", "bench_user", &eviction)
                    .await
                    .unwrap();
                std::hint::black_box(&permit);
                drop(permit);
            });
        });
    });

    group.finish();
}

/// Simulates the existing pool checkout hot path WITHOUT coordinator:
/// semaphore try_acquire + mutex lock + VecDeque pop + push back (return).
/// This is the actual baseline to compare coordinator overhead against.
fn pool_checkout_without_coordinator(c: &mut Criterion) {
    use std::collections::VecDeque;

    let mut group = c.benchmark_group("pool_checkout_comparison");
    group.throughput(Throughput::Elements(1));

    // Simulate: semaphore(pool_size) + Mutex<VecDeque> with one idle connection
    group.bench_function("without_coordinator", |b| {
        let sem = Arc::new(Semaphore::new(10));
        let slots = Arc::new(parking_lot::Mutex::new(VecDeque::from([1u64, 2, 3, 4, 5])));

        b.iter(|| {
            let p = sem.try_acquire().unwrap();
            p.forget();
            let mut guard = slots.lock();
            let conn = guard.pop_front().unwrap();
            std::hint::black_box(conn);
            guard.push_back(conn);
            drop(guard);
            sem.add_permits(1);
        });
    });

    // Same thing + coordinator try_acquire + permit drop
    group.bench_function("with_coordinator", |b| {
        let rt = runtime();
        let sem = Arc::new(Semaphore::new(10));
        let slots = Arc::new(parking_lot::Mutex::new(VecDeque::from([1u64, 2, 3, 4, 5])));
        let coord =
            rt.block_on(async { PoolCoordinator::new("bench_db".to_string(), make_config(100)) });

        b.iter(|| {
            // Coordinator overhead (on new connection creation path)
            let coord_permit = coord.try_acquire().unwrap();

            // Normal pool checkout
            let p = sem.try_acquire().unwrap();
            p.forget();
            let mut guard = slots.lock();
            let conn = guard.pop_front().unwrap();
            std::hint::black_box(conn);
            guard.push_back(conn);
            drop(guard);
            sem.add_permits(1);

            // Permit lives with connection, dropped on connection destroy
            drop(coord_permit);
        });
    });

    // Idle connection reuse — coordinator not involved (permit already inside)
    // This is the TRUE hot path: no coordinator overhead at all.
    group.bench_function("idle_reuse_no_coordinator_hit", |b| {
        let sem = Arc::new(Semaphore::new(10));
        let slots = Arc::new(parking_lot::Mutex::new(VecDeque::from([1u64, 2, 3, 4, 5])));

        b.iter(|| {
            let p = sem.try_acquire().unwrap();
            p.forget();
            let mut guard = slots.lock();
            let conn = guard.pop_front().unwrap();
            std::hint::black_box(conn);
            guard.push_back(conn);
            drop(guard);
            sem.add_permits(1);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    baseline_semaphore,
    coordinator_happy_path,
    permit_lifecycle,
    coordinator_contention,
    coordinator_async_acquire,
    pool_checkout_without_coordinator,
);
criterion_main!(benches);
