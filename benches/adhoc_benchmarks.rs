//! Measures the production Parse tracker using precomputed hashes, without SQL
//! parsing. Workers persist across samples, so thread creation is not timed.
//! The baseline includes the same hash stream, loop and synchronization costs.
//!
//! Run with: cargo bench --bench adhoc_benchmarks -- --sample-size 10
//! --warm-up-time 1 --measurement-time 1

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use pg_doorman::pool::{adhoc::AdhocQueryControl, PoolIdentifier};
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tikv_jemallocator::Jemalloc;

thread_local! {
    // Only count allocations on the measured thread, after initialization.
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn record_allocation() {
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        Jemalloc.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        Jemalloc.alloc_zeroed(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Jemalloc.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        Jemalloc.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const REPEATED_HASHES: u64 = 1024;

// A bijective mixer produces uniformly distributed, distinct u64 hashes from
// disjoint per-worker sequences. It never hashes SQL or allocates memory.
#[inline]
fn hash(value: u64) -> u64 {
    let mut value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn new_hashes(worker: usize, workers: usize) -> impl FnMut() -> u64 {
    let mut next = REPEATED_HASHES + worker as u64;
    move || {
        let result = hash(next);
        next = next.wrapping_add(workers as u64);
        result
    }
}

fn repeated_stream(
    hashes: Arc<[u64; REPEATED_HASHES as usize]>,
    mut next: usize,
) -> impl FnMut() -> u64 {
    move || {
        let result = hashes[next & (REPEATED_HASHES as usize - 1)];
        next = next.wrapping_add(1);
        result
    }
}

fn mixed_hashes(
    worker: usize,
    workers: usize,
    hashes: Arc<[u64; REPEATED_HASHES as usize]>,
) -> impl FnMut() -> u64 {
    let mut new = new_hashes(worker, workers);
    let mut repeated = repeated_stream(hashes, worker);
    let mut next = worker as u64;
    move || {
        next = next.wrapping_add(1);
        if next % 10 == 0 {
            new()
        } else {
            repeated()
        }
    }
}

fn control(name: &str, enabled: bool) -> Arc<AdhocQueryControl> {
    Arc::new(AdhocQueryControl::new(
        PoolIdentifier::new(name, "adhoc_bench"),
        enabled,
    ))
}

fn repeated_hashes(control: &AdhocQueryControl) -> Arc<[u64; REPEATED_HASHES as usize]> {
    let hashes = Arc::new(std::array::from_fn(|index| hash(index as u64)));
    for &value in hashes.iter() {
        control.observe(value);
    }
    hashes
}

#[inline]
fn observe_batch<const TRACK: bool, G: FnMut() -> u64>(
    control: &AdhocQueryControl,
    stream: &mut G,
    count: u64,
) {
    for _ in 0..count {
        let value = black_box(stream());
        if TRACK {
            black_box(control.observe(value));
        }
    }
}

struct WorkerCommand {
    start: Barrier,
    finish: Barrier,
    count: AtomicU64,
    stop: AtomicBool,
}

struct Workers {
    command: Arc<WorkerCommand>,
    handles: Vec<JoinHandle<()>>,
}

impl Workers {
    fn new<const TRACK: bool, G: FnMut() -> u64 + Send + 'static>(
        control: Arc<AdhocQueryControl>,
        streams: Vec<G>,
    ) -> Self {
        let command = Arc::new(WorkerCommand {
            start: Barrier::new(streams.len() + 1),
            finish: Barrier::new(streams.len() + 1),
            count: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let handles = streams
            .into_iter()
            .map(|mut stream| {
                let command = Arc::clone(&command);
                let control = Arc::clone(&control);
                thread::spawn(move || loop {
                    command.start.wait();
                    if command.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    observe_batch::<TRACK, _>(
                        &control,
                        &mut stream,
                        command.count.load(Ordering::Relaxed),
                    );
                    command.finish.wait();
                })
            })
            .collect();
        let workers = Self { command, handles };
        // Warm ArcSwap's thread-local state and barrier synchronization before
        // Criterion starts recording samples.
        workers.run(1);
        workers
    }

    fn run(&self, count: u64) -> Duration {
        self.command.count.store(count, Ordering::Relaxed);
        let start = Instant::now();
        self.command.start.wait();
        self.command.finish.wait();
        start.elapsed()
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.command.stop.store(true, Ordering::Relaxed);
        self.command.start.wait();
        for handle in self.handles.drain(..) {
            handle.join().expect("benchmark worker panicked");
        }
    }
}

fn assert_allocation_free<G: FnMut() -> u64>(
    name: &str,
    control: &AdhocQueryControl,
    mut stream: G,
    observations: u64,
) {
    observe_batch::<true, _>(control, &mut stream, 1);
    ALLOCATIONS.with(|count| count.set(Some(0)));
    observe_batch::<true, _>(control, &mut stream, observations);
    let allocations = ALLOCATIONS.with(|count| count.replace(None).unwrap());
    assert_eq!(allocations, 0, "{name} allocated on the warmed Parse path");
}

fn allocation_checks() {
    let disabled = control("allocation_disabled", false);
    assert_allocation_free("disabled", &disabled, new_hashes(0, 1), 10_000);
    let enabled = control("allocation_enabled", true);
    let hashes = repeated_hashes(&enabled);
    assert_allocation_free(
        "repeated",
        &enabled,
        repeated_stream(Arc::clone(&hashes), 0),
        10_000,
    );
    assert_allocation_free("mixed", &enabled, mixed_hashes(0, 1, hashes), 10_000);
    // Exceeds the ~1.2M-hash saturation budget, covering in-place shard resets
    // as well as ordinary insertion. Construction and metric registration are
    // deliberately outside the allocation measurement.
    let reset_control = control("allocation_new_and_resets", true);
    assert_allocation_free(
        "new hashes and shard resets",
        &reset_control,
        new_hashes(0, 1),
        1_600_000,
    );
}

fn benchmark_stream<const TRACK: bool, G: FnMut() -> u64 + Send + 'static>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
    control: Arc<AdhocQueryControl>,
    streams: Vec<G>,
) {
    let workers = streams.len();
    group.throughput(Throughput::Elements(workers as u64));
    if workers == 1 {
        // Run directly on the Criterion thread for an unamortized
        // single-observation measurement without any synchronization.
        let mut streams = streams.into_iter();
        let mut stream = streams.next().unwrap();
        observe_batch::<TRACK, _>(&control, &mut stream, 1);
        group.bench_function(BenchmarkId::new(name, workers), move |b| {
            b.iter(|| observe_batch::<TRACK, _>(&control, &mut stream, 1));
        });
    } else {
        let workers = Workers::new::<TRACK, _>(control, streams);
        group.bench_function(BenchmarkId::new(name, workers.handles.len()), move |b| {
            b.iter_custom(|count| workers.run(count));
        });
    }
}

fn adhoc_benchmarks(c: &mut Criterion) {
    allocation_checks();
    let parallel_workers = thread::available_parallelism()
        .map(|value| value.get().min(4))
        .unwrap_or(2);
    let mut group = c.benchmark_group("adhoc_parse_tracker");
    let worker_counts: &[usize] = if parallel_workers == 1 {
        &[1]
    } else {
        &[1, parallel_workers]
    };
    for &workers in worker_counts {
        benchmark_stream::<false, _>(
            &mut group,
            "hash_loop_baseline",
            control("baseline", false),
            (0..workers)
                .map(|worker| new_hashes(worker, workers))
                .collect(),
        );
        benchmark_stream::<true, _>(
            &mut group,
            "disabled",
            control("disabled", false),
            (0..workers)
                .map(|worker| new_hashes(worker, workers))
                .collect(),
        );
        benchmark_stream::<true, _>(
            &mut group,
            "new",
            control("new", true),
            (0..workers)
                .map(|worker| new_hashes(worker, workers))
                .collect(),
        );
        let repeated_control = control("repeated", true);
        let hashes = repeated_hashes(&repeated_control);
        benchmark_stream::<true, _>(
            &mut group,
            "repeated",
            repeated_control,
            (0..workers)
                .map(|worker| repeated_stream(Arc::clone(&hashes), worker))
                .collect(),
        );
        let mixed_control = control("mixed", true);
        let hashes = repeated_hashes(&mixed_control);
        benchmark_stream::<true, _>(
            &mut group,
            "mixed_90pct_repeated",
            mixed_control,
            (0..workers)
                .map(|worker| mixed_hashes(worker, workers, Arc::clone(&hashes)))
                .collect(),
        );
    }
    group.finish();
}

criterion_group!(benches, adhoc_benchmarks);
criterion_main!(benches);
