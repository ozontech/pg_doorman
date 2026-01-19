//! Internal Pool.get benchmarks using real PostgreSQL connections.
//!
//! This module provides cucumber steps for benchmarking the internal Pool.get
//! operation with real PostgreSQL connections.

use cucumber::{given, then, when};
use pprof::ProfilerGuardBuilder;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Check if pprof profiling is enabled via PPROF=1 environment variable
fn is_pprof_enabled() -> bool {
    std::env::var("PPROF").map(|v| v == "1").unwrap_or(false)
}

use pg_doorman::config::{Address, User};
use pg_doorman::pool::{ClientServerMap, Pool, PoolConfig, QueueMode, ServerPool, Timeouts};
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
        pool_name: "bench_pool".to_string(),
        stats: Arc::new(AddressStats::default()),
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
        4, // max_concurrent_creates
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
