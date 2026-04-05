use bytes::BytesMut;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

fn bytesmut_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytesmut_alloc");
    group.throughput(Throughput::Elements(1));

    for &size in &[100, 1024, 8192, 65536] {
        group.bench_with_input(
            BenchmarkId::new("with_capacity", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let buf = BytesMut::with_capacity(size);
                    std::hint::black_box(buf);
                });
            },
        );
    }

    group.finish();
}

fn vec_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("vec_alloc");
    group.throughput(Throughput::Elements(1));

    for &size in &[100, 1024, 8192, 65536] {
        group.bench_with_input(BenchmarkId::new("vec_zeroed", size), &size, |b, &size| {
            b.iter(|| {
                let v = vec![0u8; size];
                std::hint::black_box(v);
            });
        });
    }

    group.finish();
}

fn read_message_simulation(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_message");
    group.throughput(Throughput::Elements(1));

    for &size in &[100, 1024, 8192] {
        // Current: BytesMut alloc + Vec alloc + memcpy
        group.bench_with_input(
            BenchmarkId::new("current_alloc_vec_memcpy", size),
            &size,
            |b, &size| {
                let data = vec![42u8; size];
                b.iter(|| {
                    let mut buf = BytesMut::with_capacity(size + 5);
                    buf.extend_from_slice(&[b'Q', 0, 0, 0, 0]); // header
                    let tmp = vec![0u8; size]; // temporary Vec (simulates read)
                    std::hint::black_box(&tmp);
                    buf.extend_from_slice(&data); // memcpy
                    std::hint::black_box(buf);
                });
            },
        );

        // Proposed: BytesMut alloc + resize + direct read (no Vec, no memcpy)
        group.bench_with_input(
            BenchmarkId::new("proposed_resize_direct", size),
            &size,
            |b, &size| {
                let data = vec![42u8; size];
                b.iter(|| {
                    let mut buf = BytesMut::with_capacity(size + 5);
                    buf.extend_from_slice(&[b'Q', 0, 0, 0, 0]); // header
                    buf.resize(size + 5, 0);
                    buf[5..].copy_from_slice(&data); // direct write (simulates read_exact)
                    std::hint::black_box(buf);
                });
            },
        );

        // Best case: reuse buffer (clear + reserve, no alloc)
        group.bench_with_input(
            BenchmarkId::new("reuse_clear_reserve", size),
            &size,
            |b, &size| {
                let data = vec![42u8; size];
                let mut buf = BytesMut::with_capacity(size + 5);
                b.iter(|| {
                    buf.clear();
                    buf.reserve(size + 5);
                    buf.extend_from_slice(&[b'Q', 0, 0, 0, 0]);
                    buf.resize(size + 5, 0);
                    buf[5..].copy_from_slice(&data);
                    std::hint::black_box(&buf);
                });
            },
        );
    }

    group.finish();
}

fn clone_vs_split(c: &mut Criterion) {
    let mut group = c.benchmark_group("clone_vs_split");
    group.throughput(Throughput::Elements(1));

    for &size in &[8192, 32768, 65536] {
        let data = vec![42u8; size];

        group.bench_with_input(BenchmarkId::new("clone", size), &size, |b, _| {
            let mut buf = BytesMut::with_capacity(size);
            buf.extend_from_slice(&data);
            b.iter(|| {
                let cloned = buf.clone();
                std::hint::black_box(cloned);
            });
        });

        group.bench_with_input(BenchmarkId::new("split", size), &size, |b, _| {
            b.iter(|| {
                let mut buf = BytesMut::with_capacity(size);
                buf.extend_from_slice(&data);
                let split = buf.split();
                std::hint::black_box(split);
                // buf is now empty but backing is shared
                std::hint::black_box(buf);
            });
        });

        // split + drop + reuse (full lifecycle)
        group.bench_with_input(BenchmarkId::new("split_drop_reuse", size), &size, |b, _| {
            let mut buf = BytesMut::with_capacity(size);
            b.iter(|| {
                buf.clear();
                buf.reserve(size);
                buf.extend_from_slice(&data);
                let split = buf.split();
                std::hint::black_box(&split);
                drop(split);
                // buf should reclaim capacity after split dropped
            });
        });
    }

    group.finish();
}

fn shrink_policy(c: &mut Criterion) {
    let mut group = c.benchmark_group("shrink_policy");
    group.throughput(Throughput::Elements(1));

    // Simulate: large buffer (64KB) used for small message (1KB)
    // Option A: keep capacity (clear only)
    group.bench_function("keep_large_capacity", |b| {
        let mut buf = BytesMut::with_capacity(65536);
        buf.resize(65536, 42);
        b.iter(|| {
            buf.clear();
            buf.reserve(1024);
            buf.resize(1024, 0);
            std::hint::black_box(&buf);
        });
    });

    // Option B: replace with fresh 8KB buffer
    group.bench_function("replace_with_8kb", |b| {
        b.iter(|| {
            let mut buf = BytesMut::with_capacity(8192);
            buf.resize(1024, 0);
            std::hint::black_box(&buf);
        });
    });

    group.finish();
}

fn swap_vs_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("recv_return");
    group.throughput(Throughput::Elements(1));

    for &size in &[1024, 8192, 32768] {
        let data = vec![42u8; size];

        // Current: clone + clear (malloc + memcpy per cycle)
        group.bench_with_input(BenchmarkId::new("clone_clear", size), &size, |b, _| {
            let mut buf = BytesMut::with_capacity(size);
            b.iter(|| {
                buf.clear();
                buf.extend_from_slice(&data);
                let cloned = buf.clone();
                buf.clear();
                std::hint::black_box(cloned);
            });
        });

        // Proposed: swap with warm spare (zero alloc in steady state)
        group.bench_with_input(BenchmarkId::new("swap", size), &size, |b, _| {
            let mut buf = BytesMut::with_capacity(size);
            let mut spare = BytesMut::with_capacity(size);
            b.iter(|| {
                spare.clear();
                spare.extend_from_slice(&data);
                std::mem::swap(&mut buf, &mut spare);
                std::hint::black_box(&buf);
                buf.clear();
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bytesmut_allocation,
    vec_allocation,
    read_message_simulation,
    clone_vs_split,
    shrink_policy,
    swap_vs_clone,
);
criterion_main!(benches);
