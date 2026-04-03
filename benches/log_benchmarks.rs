use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use log::Log;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

struct CountingSink(AtomicU64);

impl CountingSink {
    fn new() -> Self {
        Self(AtomicU64::new(0))
    }
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.fetch_add(buf.len() as u64, Ordering::Relaxed);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Direct text logger (same as new production TextLogger, but writes to counting sink).
struct DirectTextLogger;

impl Log for DirectTextLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let now = chrono::Utc::now();
        let _ = writeln!(
            std::io::sink(),
            "{} {:>5} {}: {}",
            now.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {}
}

fn log_overhead(c: &mut Criterion) {
    // Use new direct logger — NO tracing bridge.
    // This registers as the global log implementation.
    log::set_logger(&DirectTextLogger).unwrap();
    log::set_max_level(log::LevelFilter::Info);

    let pool_name = "production_db";
    let username = "app_user";
    let addr = "10.0.0.5:43210";

    let mut group = c.benchmark_group("log_direct");
    group.throughput(Throughput::Elements(1));

    group.bench_function("debug_at_info_level", |b| {
        b.iter(|| {
            log::debug!("[{}@{}] talking to server {}", username, pool_name, addr);
        });
    });

    group.bench_function("info_direct_logger", |b| {
        b.iter(|| {
            log::info!(
                "[{}@{}] client connected from {}",
                username, pool_name, addr
            );
        });
    });

    group.bench_function("warn_direct_logger", |b| {
        b.iter(|| {
            log::warn!(
                "[{}@{}] pool exhausted cl_waiting=15 sv_active=50",
                username, pool_name
            );
        });
    });

    group.bench_function("noop_baseline", |b| {
        b.iter(|| {
            std::hint::black_box(42);
        });
    });

    group.bench_function("format_string_alloc", |b| {
        b.iter(|| {
            let msg = format!(
                "[{}@{}] client connected from {}",
                username, pool_name, addr
            );
            std::hint::black_box(&msg);
        });
    });

    group.bench_function("writeln_to_counting_sink", |b| {
        let mut sink = CountingSink::new();
        b.iter(|| {
            let _ = writeln!(
                sink,
                "INFO [{}@{}] client connected from {}",
                username, pool_name, addr
            );
        });
    });

    group.bench_function("debug_expensive_args_at_info", |b| {
        let data: Vec<u32> = (0..100).collect();
        b.iter(|| {
            log::debug!(
                "[{}@{}] eviction candidates: {:?}",
                username,
                pool_name,
                data
            );
        });
    });

    group.bench_function("info_4_threads_contention", |b| {
        b.iter(|| {
            std::thread::scope(|s| {
                for i in 0..4 {
                    s.spawn(move || {
                        for _ in 0..100 {
                            log::info!("[user_{}@db] query completed", i);
                        }
                    });
                }
            });
        });
    });

    group.finish();
}

criterion_group!(benches, log_overhead);
criterion_main!(benches);
