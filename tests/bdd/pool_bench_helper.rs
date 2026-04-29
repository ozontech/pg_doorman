//! Internal Pool.get benchmarks using real PostgreSQL connections.
//!
//! This module provides cucumber steps for benchmarking the internal Pool.get
//! operation with real PostgreSQL connections.

use cucumber::{given, then, when};
use pprof::ProfilerGuardBuilder;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Check if pprof profiling is enabled via PPROF=1 environment variable
fn is_pprof_enabled() -> bool {
    std::env::var("PPROF").map(|v| v == "1").unwrap_or(false)
}

use pg_doorman::config::{Address, User};
use pg_doorman::pool::{
    ClientServerMap, Pool, PoolConfig, QueueMode, ScalingConfig, ScalingStatsSnapshot, ServerPool,
    Timeouts,
};
use pg_doorman::stats::AddressStats;

use crate::world::DoormanWorld;

/// Internal pool for benchmarking (stored in world)
pub struct InternalPool {
    pub pool: Pool,
}

#[given(regex = r"^internal pool with size (\d+) and mode (transaction|session)$")]
async fn setup_internal_pool(world: &mut DoormanWorld, size: usize, _mode: String) {
    let pg_port = world.pg_port.expect("PostgreSQL must be running");

    // Create empty client-server map (use default 4 worker_threads for tests)
    let client_server_map: ClientServerMap = Arc::new(pg_doorman::utils::dashmap::new_dashmap(4));

    // Create Address for the PostgreSQL server
    let address = Address {
        host: "127.0.0.1".to_string(),
        port: pg_port,
        database: "postgres".to_string(),
        username: "postgres".to_string(),
        password: "".to_string(),
        backend_auth: None,
        pool_name: "bench_pool".to_string(),
        stats: Arc::new(AddressStats::default()),
        ..Address::default()
    };

    // Create User
    let user = User {
        username: "postgres".to_string(),
        password: "".to_string(),
        pool_size: size as u32,
        ..User::default()
    };

    // Create ServerPool (manager)
    let server_pool = ServerPool::new(
        address,
        user,
        "postgres",
        client_server_map,
        true,  // cleanup_connections
        false, // log_client_parameter_status_changes
        0,     // prepared_statement_cache_size
        "pool_bench".to_string(),
        4,                       // max_concurrent_creates
        0,                       // lifetime_ms (0 = unlimited)
        0,                       // idle_timeout_ms (0 = disabled)
        0,                       // idle_check_timeout_ms (0 = disabled)
        Duration::from_secs(10), // connect_timeout
        Duration::from_secs(10), // query_wait_timeout
        false,                   // session_mode
        None,                    // fallback_state
    );

    // Create Pool with configuration
    let config = PoolConfig {
        max_size: size,
        timeouts: Timeouts {
            wait: Some(Duration::from_secs(30)),
            create: Some(Duration::from_secs(10)),
            recycle: None,
        },
        queue_mode: QueueMode::Lifo,
        scaling: ScalingConfig::default(),
    };

    let pool = Pool::builder(server_pool).config(config).build();

    world.internal_pool = Some(InternalPool { pool });
}

#[when(regex = r#"^I benchmark pool\.get with (\d+) iterations and save as "([^"]+)"$"#)]
async fn benchmark_pool_get_iterations_named(
    world: &mut DoormanWorld,
    iterations: usize,
    name: String,
) {
    // Single client sequential benchmark
    benchmark_pool_get_impl(world, 1, iterations, name).await;
}

#[when(
    regex = r#"^I benchmark pool\.get with (\d+) concurrent clients and (\d+) iterations per client and save as "([^"]+)"$"#
)]
async fn benchmark_pool_get_concurrent(
    world: &mut DoormanWorld,
    clients: usize,
    iterations_per_client: usize,
    name: String,
) {
    benchmark_pool_get_impl(world, clients, iterations_per_client, name).await;
}

/// Internal implementation for pool.get benchmarks supporting both single and concurrent clients
async fn benchmark_pool_get_impl(
    world: &mut DoormanWorld,
    clients: usize,
    iterations_per_client: usize,
    name: String,
) {
    let internal_pool = world
        .internal_pool
        .as_ref()
        .expect("Internal pool must be set up");

    let pool = &internal_pool.pool;
    let total_iterations = clients * iterations_per_client;

    // Warm-up: create initial connections
    {
        let _obj = pool
            .get()
            .await
            .expect("Failed to get connection for warm-up");
    }

    // Start CPU profiler with pprof only if PPROF=1 is set
    let pprof_enabled = is_pprof_enabled();
    let guard = if pprof_enabled {
        Some(
            ProfilerGuardBuilder::default()
                .frequency(1000) // 1000 Hz sampling
                .blocklist(&["libc", "libgcc", "pthread", "vdso"])
                .build()
                .expect("Failed to build profiler guard"),
        )
    } else {
        None
    };

    let latencies = if clients == 1 {
        // Single client sequential benchmark
        let mut latencies = Vec::with_capacity(iterations_per_client);
        for _ in 0..iterations_per_client {
            let iter_start = Instant::now();
            let obj = pool.get().await.expect("Failed to get connection");
            let latency = iter_start.elapsed();
            latencies.push(latency);
            drop(obj); // Return to pool
        }
        latencies
    } else {
        // Concurrent clients benchmark using tokio tasks
        let pool = pool.clone();
        let mut handles = Vec::with_capacity(clients);

        for _ in 0..clients {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                let mut latencies = Vec::with_capacity(iterations_per_client);
                for _ in 0..iterations_per_client {
                    let iter_start = Instant::now();
                    let obj = pool.get().await.expect("Failed to get connection");
                    let latency = iter_start.elapsed();
                    latencies.push(latency);
                    drop(obj); // Return to pool
                }
                latencies
            }));
        }

        // Collect results from all clients
        let mut all_latencies = Vec::with_capacity(total_iterations);
        for handle in handles {
            let client_latencies = handle.await.expect("Client task failed");
            all_latencies.extend(client_latencies);
        }
        all_latencies
    };

    // Note: we measure wall-clock time for the concurrent case differently
    // For accurate throughput, we re-run the benchmark with timing
    let (latencies, total_elapsed) = if clients == 1 {
        // For single client, latencies are already collected above, just sum them
        let total: Duration = latencies.iter().sum();
        (latencies, total)
    } else {
        // For concurrent clients, run again with wall-clock timing
        let pool = pool.clone();
        let mut handles = Vec::with_capacity(clients);

        let start = Instant::now();
        for _ in 0..clients {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                let mut latencies = Vec::with_capacity(iterations_per_client);
                for _ in 0..iterations_per_client {
                    let iter_start = Instant::now();
                    let obj = pool.get().await.expect("Failed to get connection");
                    let latency = iter_start.elapsed();
                    latencies.push(latency);
                    drop(obj);
                }
                latencies
            }));
        }

        let mut all_latencies = Vec::with_capacity(total_iterations);
        for handle in handles {
            let client_latencies = handle.await.expect("Client task failed");
            all_latencies.extend(client_latencies);
        }
        let total_elapsed = start.elapsed();
        (all_latencies, total_elapsed)
    };

    let ops_per_sec = total_iterations as f64 / total_elapsed.as_secs_f64();

    // Calculate percentiles
    let mut latencies = latencies;
    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[(latencies.len() as f64 * 0.95) as usize];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];

    // Output to stdout
    println!("\n=== Pool.get Benchmark Results [{}] ===", name);
    println!("Concurrent clients: {}", clients);
    println!("Iterations per client: {}", iterations_per_client);
    println!("Total iterations: {}", total_iterations);
    println!("Total time: {:?}", total_elapsed);
    println!("Throughput: {:.0} ops/sec", ops_per_sec);
    println!("Latency p50: {:?}", p50);
    println!("Latency p95: {:?}", p95);
    println!("Latency p99: {:?}", p99);

    // Print pprof CPU timing breakdown only if profiling was enabled
    let total_samples = if let Some(guard) = guard {
        let report = guard
            .report()
            .build()
            .expect("Failed to build pprof report");

        println!("\n--- CPU Profile (pprof) Top Functions ---");

        // Aggregate samples by function name across all stack frames
        let mut func_samples: HashMap<String, usize> = HashMap::new();
        let mut total_samples: usize = 0;

        for (frames, count) in report.data.iter() {
            total_samples += *count as usize;
            // Iterate through all stack frames to find meaningful function names
            for symbols in frames.frames.iter() {
                for symbol in symbols.iter() {
                    let name = symbol.name();
                    // Skip backtrace/profiler internal functions
                    if name.contains("backtrace::")
                        || name.contains("pprof::")
                        || name.contains("__pthread")
                        || name.contains("_sigtramp")
                        || name.starts_with("_")
                    {
                        continue;
                    }
                    *func_samples.entry(name.to_string()).or_insert(0) += *count as usize;
                }
            }
        }

        // Sort by sample count
        let mut frame_times: Vec<(String, usize)> = func_samples.into_iter().collect();
        frame_times.sort_by(|a, b| b.1.cmp(&a.1));

        println!("Total CPU samples: {}", total_samples);
        println!("| Function | Samples | % |");
        println!("|----------|---------|---|");
        for (func, count) in frame_times.iter().take(20) {
            let pct = if total_samples > 0 {
                (*count as f64 / total_samples as f64) * 100.0
            } else {
                0.0
            };
            // Truncate long function names
            let display_name = if func.len() > 70 {
                format!("{}...", &func[..67])
            } else {
                func.clone()
            };
            println!("| {} | {} | {:.1}% |", display_name, count, pct);
        }
        println!("==========================================\n");

        total_samples
    } else {
        println!("(pprof profiling disabled, set PPROF=1 to enable)\n");
        0
    };

    world.bench_results.insert(name.clone(), ops_per_sec);
    world
        .bench_results
        .insert(format!("{}_p50_ns", name), p50.as_nanos() as f64);
    world
        .bench_results
        .insert(format!("{}_p95_ns", name), p95.as_nanos() as f64);
    world
        .bench_results
        .insert(format!("{}_p99_ns", name), p99.as_nanos() as f64);
    world
        .bench_results
        .insert(format!("{}_cpu_samples", name), total_samples as f64);
}

#[then(regex = r#"^benchmark result "([^"]+)" should exist$"#)]
async fn benchmark_result_should_exist(world: &mut DoormanWorld, name: String) {
    assert!(
        world.bench_results.contains_key(&name),
        "Benchmark result '{}' not found. Available: {:?}",
        name,
        world.bench_results.keys().collect::<Vec<_>>()
    );
}

#[then("I print benchmark results to stdout")]
async fn print_benchmark_results_to_stdout(world: &mut DoormanWorld) {
    println!("\n=== All Benchmark Results ===");
    println!("| Test | Throughput | p50 | p95 | p99 |");
    println!("|------|------------|-----|-----|-----|");

    // Find main test names (without _pXX_ns suffix)
    let mut test_names: Vec<String> = world
        .bench_results
        .keys()
        .filter(|k| !k.contains("_p50_") && !k.contains("_p95_") && !k.contains("_p99_"))
        .cloned()
        .collect();
    test_names.sort();

    for test_name in &test_names {
        let ops = world.bench_results.get(test_name.as_str()).unwrap_or(&0.0);
        let p50 = world
            .bench_results
            .get(&format!("{}_p50_ns", test_name))
            .unwrap_or(&0.0);
        let p95 = world
            .bench_results
            .get(&format!("{}_p95_ns", test_name))
            .unwrap_or(&0.0);
        let p99 = world
            .bench_results
            .get(&format!("{}_p99_ns", test_name))
            .unwrap_or(&0.0);
        println!(
            "| {} | {:.0} ops/sec | {:.0} ns | {:.0} ns | {:.0} ns |",
            test_name, ops, p50, p95, p99
        );
    }
    println!("=============================\n");
}

/// Variant of `setup_internal_pool` that exposes `server_lifetime_ms` and
/// `idle_timeout_ms`. Used by pressure scenarios that want to observe how
/// `recycle()` and the retain loop behave under saturation — the default
/// bench pool uses 0/0 (no lifetime, no idle timeout), which hides the
/// interesting cases.
#[given(
    regex = r"^internal pool with size (\d+), server_lifetime (\d+) ms, idle_timeout (\d+) ms$"
)]
async fn setup_internal_pool_with_lifetimes(
    world: &mut DoormanWorld,
    size: usize,
    lifetime_ms: u64,
    idle_timeout_ms: u64,
) {
    let pg_port = world.pg_port.expect("PostgreSQL must be running");

    let client_server_map: ClientServerMap = Arc::new(pg_doorman::utils::dashmap::new_dashmap(4));

    let address = Address {
        host: "127.0.0.1".to_string(),
        port: pg_port,
        database: "postgres".to_string(),
        username: "postgres".to_string(),
        password: "".to_string(),
        backend_auth: None,
        pool_name: "bench_pool".to_string(),
        stats: Arc::new(AddressStats::default()),
        ..Address::default()
    };

    let user = User {
        username: "postgres".to_string(),
        password: "".to_string(),
        pool_size: size as u32,
        ..User::default()
    };

    let server_pool = ServerPool::new(
        address,
        user,
        "postgres",
        client_server_map,
        true,
        false,
        0,
        "pool_bench".to_string(),
        4,
        lifetime_ms,
        idle_timeout_ms,
        0,
        Duration::from_secs(10),
        Duration::from_secs(10),
        false,
        None,
    );

    let config = PoolConfig {
        max_size: size,
        timeouts: Timeouts {
            wait: Some(Duration::from_secs(30)),
            create: Some(Duration::from_secs(10)),
            recycle: None,
        },
        queue_mode: QueueMode::Lifo,
        scaling: ScalingConfig::default(),
    };

    let pool = Pool::builder(server_pool).config(config).build();
    world.internal_pool = Some(InternalPool { pool });
}

/// Cascade load generator. Spawns N concurrent clients that each loop
/// `pool.get() → hold → drop` for the specified wall-clock duration.
/// Captures per-acquire latency and snapshots `scaling_stats()` before
/// and after so the downstream assertions can pin both tail latency and
/// the pool's internal bookkeeping (creates, fallbacks, anticipation
/// wakes) against regressions.
///
/// This is the regression harness for any future Phase 4 shortcut or
/// pool tuning work: run this step before and after a change, compare
/// the cached metrics.
#[when(
    regex = r#"^I run cascade load "([^"]+)" with (\d+) clients for (\d+) seconds holding (\d+) ms$"#
)]
async fn run_cascade_load(
    world: &mut DoormanWorld,
    name: String,
    clients: usize,
    duration_secs: u64,
    hold_ms: u64,
) {
    let pool = world
        .internal_pool
        .as_ref()
        .expect("Internal pool must be set up")
        .pool
        .clone();

    // Warm-up: force Phase 1 allocation for at least one slot so the very
    // first burst does not measure cold-start create latency.
    {
        let _warm = pool.get().await.expect("warm-up acquire failed");
    }

    let stats_before = pool.scaling_stats();
    let size_before = pool.status().size;

    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    let hold = Duration::from_millis(hold_ms);
    let errors = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::with_capacity(clients);
    for _ in 0..clients {
        let pool = pool.clone();
        let errors = Arc::clone(&errors);
        handles.push(tokio::spawn(async move {
            let mut local_latencies: Vec<Duration> = Vec::new();
            while Instant::now() < deadline {
                let acquire_start = Instant::now();
                match pool.get().await {
                    Ok(obj) => {
                        local_latencies.push(acquire_start.elapsed());
                        if !hold.is_zero() {
                            tokio::time::sleep(hold).await;
                        }
                        drop(obj);
                    }
                    Err(_) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            local_latencies
        }));
    }

    // Per-client iteration counts — surface fairness skews. A client stuck
    // in `pool.get()` while peers race ahead will show up as a tiny
    // `iter_count` against the rest. Without this, starvation hides
    // behind an aggregate latency p99.
    let mut per_client_iters: Vec<usize> = Vec::with_capacity(clients);
    let mut all_latencies: Vec<Duration> = Vec::new();
    for h in handles {
        let client_latencies = h.await.expect("cascade client task panicked");
        per_client_iters.push(client_latencies.len());
        all_latencies.extend(client_latencies);
    }

    let stats_after = pool.scaling_stats();
    let size_after = pool.status().size;

    let errors_total = errors.load(Ordering::Relaxed);
    assert!(
        !all_latencies.is_empty(),
        "cascade load produced no successful acquires"
    );
    all_latencies.sort();

    let p50 = all_latencies[all_latencies.len() / 2];
    let p95 = all_latencies[(all_latencies.len() as f64 * 0.95) as usize];
    let p99 = all_latencies[(all_latencies.len() as f64 * 0.99) as usize];
    let max_lat = *all_latencies.last().unwrap();

    let delta = StatsDelta::new(&stats_before, &stats_after);

    // Fairness stats: starvation shows up here as min_iters << max_iters.
    // A single client stuck 30 seconds while peers race ahead will have
    // iter_count=1 against max_iters in the thousands.
    per_client_iters.sort();
    let min_iters = *per_client_iters.first().unwrap_or(&0);
    let max_iters = *per_client_iters.last().unwrap_or(&0);
    let median_iters = per_client_iters[per_client_iters.len() / 2];

    println!("\n=== Cascade Load [{}] ===", name);
    println!(
        "clients={} duration={}s hold={}ms acquires={} errors={}",
        clients,
        duration_secs,
        hold_ms,
        all_latencies.len(),
        errors_total
    );
    println!(
        "latency p50={:?} p95={:?} p99={:?} max={:?}",
        p50, p95, p99, max_lat
    );
    println!(
        "per-client iters min={} median={} max={}",
        min_iters, median_iters, max_iters
    );
    println!(
        "pool size {} → {} (Δ={:+})",
        size_before,
        size_after,
        size_after as i64 - size_before as i64
    );
    println!(
        "scaling Δ: creates_started={} create_fallback={} burst_gate_waits={} \
         antic_notify={} antic_timeout={} replenish_deferred={}",
        delta.creates_started,
        delta.create_fallback,
        delta.burst_gate_waits,
        delta.anticipation_wakes_notify,
        delta.anticipation_wakes_timeout,
        delta.replenish_deferred,
    );

    world
        .bench_results
        .insert(format!("{}_p50_ns", name), p50.as_nanos() as f64);
    world
        .bench_results
        .insert(format!("{}_p95_ns", name), p95.as_nanos() as f64);
    world
        .bench_results
        .insert(format!("{}_p99_ns", name), p99.as_nanos() as f64);
    world
        .bench_results
        .insert(format!("{}_max_ns", name), max_lat.as_nanos() as f64);
    world
        .bench_results
        .insert(format!("{}_errors", name), errors_total as f64);
    world
        .bench_results
        .insert(format!("{}_min_iters", name), min_iters as f64);
    world
        .bench_results
        .insert(format!("{}_max_iters", name), max_iters as f64);
    world
        .bench_results
        .insert(format!("{}_median_iters", name), median_iters as f64);
    world
        .bench_results
        .insert(format!("{}_size_before", name), size_before as f64);
    world
        .bench_results
        .insert(format!("{}_size_after", name), size_after as f64);
    world.bench_results.insert(
        format!("{}_creates_started", name),
        delta.creates_started as f64,
    );
    world.bench_results.insert(
        format!("{}_create_fallback", name),
        delta.create_fallback as f64,
    );
    world.bench_results.insert(
        format!("{}_burst_gate_waits", name),
        delta.burst_gate_waits as f64,
    );
    world.bench_results.insert(
        format!("{}_antic_notify", name),
        delta.anticipation_wakes_notify as f64,
    );
    world.bench_results.insert(
        format!("{}_antic_timeout", name),
        delta.anticipation_wakes_timeout as f64,
    );
}

struct StatsDelta {
    creates_started: u64,
    burst_gate_waits: u64,
    anticipation_wakes_notify: u64,
    anticipation_wakes_timeout: u64,
    create_fallback: u64,
    replenish_deferred: u64,
}

impl StatsDelta {
    fn new(before: &ScalingStatsSnapshot, after: &ScalingStatsSnapshot) -> Self {
        Self {
            creates_started: after.creates_started.saturating_sub(before.creates_started),
            burst_gate_waits: after
                .burst_gate_waits
                .saturating_sub(before.burst_gate_waits),
            anticipation_wakes_notify: after
                .anticipation_wakes_notify
                .saturating_sub(before.anticipation_wakes_notify),
            anticipation_wakes_timeout: after
                .anticipation_wakes_timeout
                .saturating_sub(before.anticipation_wakes_timeout),
            create_fallback: after.create_fallback.saturating_sub(before.create_fallback),
            replenish_deferred: after
                .replenish_deferred
                .saturating_sub(before.replenish_deferred),
        }
    }
}

#[then(regex = r#"^cascade "([^"]+)" reports zero errors$"#)]
async fn cascade_zero_errors(world: &mut DoormanWorld, name: String) {
    let errors = world
        .bench_results
        .get(&format!("{}_errors", name))
        .copied()
        .unwrap_or(f64::NAN);
    assert_eq!(
        errors, 0.0,
        "cascade '{}' expected zero errors, got {}",
        name, errors
    );
}

#[then(regex = r#"^cascade "([^"]+)" p99 latency is below (\d+) ms$"#)]
async fn cascade_p99_below(world: &mut DoormanWorld, name: String, limit_ms: u64) {
    let p99_ns = world
        .bench_results
        .get(&format!("{}_p99_ns", name))
        .copied()
        .expect("cascade step must run before assertion");
    let p99_ms = p99_ns / 1_000_000.0;
    assert!(
        p99_ms < limit_ms as f64,
        "cascade '{}' p99 {:.2}ms exceeded limit {}ms",
        name,
        p99_ms,
        limit_ms,
    );
}

#[then(regex = r#"^cascade "([^"]+)" creates_started is at most (\d+)$"#)]
async fn cascade_creates_at_most(world: &mut DoormanWorld, name: String, limit: u64) {
    let creates = world
        .bench_results
        .get(&format!("{}_creates_started", name))
        .copied()
        .expect("cascade step must run before assertion");
    assert!(
        (creates as u64) <= limit,
        "cascade '{}' creates_started {} exceeded limit {}",
        name,
        creates as u64,
        limit,
    );
}

#[then(regex = r#"^cascade "([^"]+)" creates_started is at least (\d+)$"#)]
async fn cascade_creates_at_least(world: &mut DoormanWorld, name: String, minimum: u64) {
    let creates = world
        .bench_results
        .get(&format!("{}_creates_started", name))
        .copied()
        .expect("cascade step must run before assertion");
    assert!(
        (creates as u64) >= minimum,
        "cascade '{}' creates_started {} below minimum {}",
        name,
        creates as u64,
        minimum,
    );
}

#[then(regex = r#"^cascade "([^"]+)" create_fallback is at most (\d+)$"#)]
async fn cascade_create_fallback_at_most(world: &mut DoormanWorld, name: String, limit: u64) {
    let fb = world
        .bench_results
        .get(&format!("{}_create_fallback", name))
        .copied()
        .expect("cascade step must run before assertion");
    assert!(
        (fb as u64) <= limit,
        "cascade '{}' create_fallback {} exceeded limit {}",
        name,
        fb as u64,
        limit,
    );
}

#[then(regex = r#"^cascade "([^"]+)" pool size is at most (\d+)$"#)]
async fn cascade_size_at_most(world: &mut DoormanWorld, name: String, limit: u64) {
    let size = world
        .bench_results
        .get(&format!("{}_size_after", name))
        .copied()
        .expect("cascade step must run before assertion");
    assert!(
        (size as u64) <= limit,
        "cascade '{}' final pool size {} exceeded limit {}",
        name,
        size as u64,
        limit,
    );
}

#[then(regex = r#"^cascade "([^"]+)" pool size is at least (\d+)$"#)]
async fn cascade_size_at_least(world: &mut DoormanWorld, name: String, minimum: u64) {
    let size = world
        .bench_results
        .get(&format!("{}_size_after", name))
        .copied()
        .expect("cascade step must run before assertion");
    assert!(
        (size as u64) >= minimum,
        "cascade '{}' final pool size {} below minimum {}",
        name,
        size as u64,
        minimum,
    );
}

/// Tail latency gate: the slowest single acquire must not exceed the
/// given multiple of the p50 latency. A healthy pool keeps max / p50
/// within a small ratio — aggregate p99 hides single stuck acquires
/// because one outlier is buried in the percentile arithmetic. A
/// `pool.get()` that wakes up 30 seconds late while its median peer
/// returns in 60ms is exactly the failure mode this assertion surfaces.
#[then(regex = r#"^cascade "([^"]+)" max latency is bounded by (\d+)x p50$"#)]
async fn cascade_max_bounded_by_p50(world: &mut DoormanWorld, name: String, multiple: u64) {
    let p50_ns = world
        .bench_results
        .get(&format!("{}_p50_ns", name))
        .copied()
        .expect("cascade step must run before assertion");
    let max_ns = world
        .bench_results
        .get(&format!("{}_max_ns", name))
        .copied()
        .expect("cascade step must run before assertion");

    assert!(
        p50_ns > 0.0,
        "cascade '{}' p50 latency is zero — cannot bound max against p50",
        name
    );
    let ratio = max_ns / p50_ns;
    assert!(
        ratio <= multiple as f64,
        "cascade '{}' tail latency blow-up: max={:.2}ms p50={:.2}ms ratio={:.1}x \
         (limit {}x) — a single acquire lagged far behind the median, likely \
         stuck in semaphore wake or pool resize mid-acquire",
        name,
        max_ns / 1_000_000.0,
        p50_ns / 1_000_000.0,
        ratio,
        multiple,
    );
}

/// Fairness gate: the slowest client in a cascade must not lag by more
/// than the given multiple of the median iteration count. A starved
/// client waiting in `pool.get()` while peers race ahead would show
/// min_iters=1 against median_iters in the hundreds. Complements
/// `max latency is bounded by Nx p50`: that catches single stuck
/// acquires, this one catches clients stuck across their entire run.
#[then(regex = r#"^cascade "([^"]+)" iteration spread is bounded by (\d+)x median$"#)]
async fn cascade_iter_spread_bounded(world: &mut DoormanWorld, name: String, multiple: u64) {
    let min = world
        .bench_results
        .get(&format!("{}_min_iters", name))
        .copied()
        .expect("cascade step must run before assertion") as u64;
    let median = world
        .bench_results
        .get(&format!("{}_median_iters", name))
        .copied()
        .expect("cascade step must run before assertion") as u64;
    let max_iters = world
        .bench_results
        .get(&format!("{}_max_iters", name))
        .copied()
        .expect("cascade step must run before assertion") as u64;

    assert!(
        median > 0,
        "cascade '{}' median iteration count is zero — no successful acquires",
        name
    );

    // If min_iters * multiple >= median, the slowest client is within
    // the fairness envelope. Otherwise one or more clients were starved.
    let lower_bound = median.div_ceil(multiple);
    assert!(
        min >= lower_bound,
        "cascade '{}' starvation: min_iters={} median_iters={} max_iters={} \
         (slowest client < median/{}) — at least one client lagged far behind \
         the pack, likely stuck in pool.get() semaphore wait",
        name,
        min,
        median,
        max_iters,
        multiple,
    );
}

/// Assert that server-side query_time p99 (from the address stats histogram)
/// is below a threshold. Catches session-mode bugs where query_time accumulates
/// the entire session duration instead of individual query time.
#[then(regex = r#"^cascade "([^"]+)" server query_p99 is below (\d+) ms$"#)]
async fn cascade_server_query_p99_below(world: &mut DoormanWorld, _name: String, limit_ms: u64) {
    let pool = world
        .internal_pool
        .as_ref()
        .expect("Internal pool must be set up")
        .pool
        .clone();
    let (_, _, _, query_p99_us) = pool.server_pool().address().stats.get_query_percentiles();
    let query_p99_ms = query_p99_us as f64 / 1_000.0;
    assert!(
        query_p99_ms < limit_ms as f64,
        "server-side query_p99 {:.2}ms exceeded limit {}ms \
         (likely session-mode accumulation bug — query_time not reset per-query)",
        query_p99_ms,
        limit_ms,
    );
}
