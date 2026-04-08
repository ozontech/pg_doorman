//! Microbenchmarks for the anticipation + bounded burst path in `Pool::timeout_get`.
//!
//! These bench the pure helpers and primitives used by the new code path:
//!
//! 1. `try_take_burst_slot` happy path  — the cost paid on every new-connection
//!    create when the pool is below the burst cap. Must stay in the few-ns
//!    range so it does not regress the create hot path.
//! 2. `try_take_burst_slot` rejection   — the cost when the cap is reached.
//!    Includes the rollback `fetch_sub`. Should also be in the few-ns range.
//! 3. Concurrent burst gate             — N tokio tasks racing the gate with
//!    cap=2. Verifies the gate scales without contention surprises.
//! 4. `Notify` wake one waiter          — the cost of the anticipation signal:
//!    `notified()` registration + `notify_one` + the woken task progressing.
//! 5. `Notify` register-before-check    — the buffered-notify pattern used in
//!    the cooldown zone. Verifies that a notify fired before the await still
//!    resolves immediately, with realistic timings.
//!
//! These helpers are duplicated from `src/pool/inner.rs` on purpose: keeping
//! them private avoids leaking test/bench scaffolding into the public API
//! surface, and the duplication is twenty trivial lines.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[inline]
fn try_take_burst_slot(counter: &AtomicUsize, max: usize) -> bool {
    let prev = counter.fetch_add(1, Ordering::AcqRel);
    if prev < max {
        return true;
    }
    counter.fetch_sub(1, Ordering::Release);
    false
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

/// Happy path: counter is below the cap, slot is granted.
/// Caller (the bench loop) immediately releases it. Two atomic ops.
fn burst_slot_happy_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("pool_anticipation/burst_slot_happy_path");
    group.throughput(Throughput::Elements(1));

    group.bench_function("take_release", |b| {
        let counter = AtomicUsize::new(0);
        b.iter(|| {
            let took = try_take_burst_slot(&counter, 4);
            assert!(took);
            counter.fetch_sub(1, Ordering::Release);
        });
    });

    group.finish();
}

/// Rejection path: counter is already at the cap, slot is denied and the
/// optimistic increment must be rolled back. Two atomic ops + branch.
fn burst_slot_reject_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("pool_anticipation/burst_slot_reject_path");
    group.throughput(Throughput::Elements(1));

    group.bench_function("fetch_add_rollback", |b| {
        let counter = AtomicUsize::new(2);
        b.iter(|| {
            let took = try_take_burst_slot(&counter, 2);
            assert!(!took);
        });
    });

    group.finish();
}

/// Concurrent gate: N tokio tasks race the burst gate with cap=2.
/// Each accepted slot is held for one yield then released. Measures the
/// effective wall clock to drain a bounded burst under realistic contention.
fn burst_gate_concurrent(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("pool_anticipation/burst_gate_concurrent");
    group.throughput(Throughput::Elements(1));
    group.sample_size(50);

    for &(tasks, cap) in &[(4usize, 2usize), (8, 2), (16, 2), (32, 2), (32, 4)] {
        group.bench_with_input(
            BenchmarkId::new(format!("{tasks}t_cap{cap}"), tasks),
            &(tasks, cap),
            |b, &(tasks, cap)| {
                b.iter(|| {
                    let counter = Arc::new(AtomicUsize::new(0));
                    rt.block_on(async {
                        let mut handles = Vec::with_capacity(tasks);
                        for _ in 0..tasks {
                            let counter = Arc::clone(&counter);
                            handles.push(tokio::spawn(async move {
                                // Spin until we get a slot — mirrors the
                                // bounded burst loop without notify wakes.
                                while !try_take_burst_slot(&counter, cap) {
                                    tokio::task::yield_now().await;
                                }
                                tokio::task::yield_now().await;
                                counter.fetch_sub(1, Ordering::Release);
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

/// Notify wake throughput: one task waits on `notified()`, another fires
/// `notify_one`. Measures the round-trip cost of the anticipation wake path.
fn notify_one_wake(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("pool_anticipation/notify_one_wake");
    group.throughput(Throughput::Elements(1));

    group.bench_function("register_signal_wake", |b| {
        let notify = Arc::new(Notify::new());
        b.iter(|| {
            let n = Arc::clone(&notify);
            rt.block_on(async move {
                let waiter = tokio::spawn({
                    let n = Arc::clone(&n);
                    async move {
                        n.notified().await;
                    }
                });
                // Yield once so the waiter has a chance to register.
                tokio::task::yield_now().await;
                n.notify_one();
                waiter.await.unwrap();
            });
        });
    });

    group.finish();
}

/// Buffered-notify pattern: `notify_one` fires BEFORE `notified()` is awaited.
/// The cooldown anticipation zone relies on this — measures that the buffered
/// signal resolves the await immediately without any wall time spent.
fn notify_buffered_pattern(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("pool_anticipation/notify_buffered");
    group.throughput(Throughput::Elements(1));

    group.bench_function("register_after_signal", |b| {
        let notify = Arc::new(Notify::new());
        b.iter(|| {
            rt.block_on(async {
                let notified = notify.notified();
                notify.notify_one();
                // Must resolve immediately — buffered signal.
                tokio::time::timeout(Duration::from_millis(10), notified)
                    .await
                    .expect("buffered notify must wake");
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    burst_slot_happy_path,
    burst_slot_reject_path,
    burst_gate_concurrent,
    notify_one_wake,
    notify_buffered_pattern,
);
criterion_main!(benches);
