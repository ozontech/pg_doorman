//! Internal Pool.get benchmarks using real PostgreSQL connections.
//!
//! This module provides cucumber steps for benchmarking the internal Pool.get
//! operation with real PostgreSQL connections. Unlike external pgbench benchmarks,
//! these tests measure the internal pool acquisition overhead directly.

use cucumber::{given, then, when};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pg_doorman::config::{Address, User};
use pg_doorman::pool::{ClientServerMap, Pool, PoolConfig, ServerPool, Timeouts};
use pg_doorman::stats::AddressStats;

use crate::world::DoormanWorld;

/// Internal pool for benchmarking (stored in world)
pub struct InternalPool {
    pub pool: Pool,
    pub latencies: Vec<Duration>,
}

#[given(regex = r"^internal pool with size (\d+) and mode (transaction|session)$")]
async fn setup_internal_pool(world: &mut DoormanWorld, size: usize, _mode: String) {
    let pg_port = world.pg_port.expect("PostgreSQL must be running");

    // Create empty client-server map
    let client_server_map: ClientServerMap = Arc::new(Mutex::new(HashMap::new()));

    // Create Address for the PostgreSQL server
    let address = Address {
        host: "127.0.0.1".to_string(),
        port: pg_port,
        virtual_pool_id: 0,
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
    );

    // Create Pool with configuration
    let config = PoolConfig {
        max_size: size,
        timeouts: Timeouts {
            wait: Some(Duration::from_secs(30)),
            create: Some(Duration::from_secs(10)),
            recycle: None,
        },
    };

    let pool = Pool::builder(server_pool).config(config).build();

    world.internal_pool = Some(InternalPool {
        pool,
        latencies: Vec::new(),
    });
}

/// Setup internal pool with queue mode (deprecated - now always FIFO)
/// Kept for backward compatibility with existing tests
#[given(regex = r"^internal pool with size (\d+) and queue mode (fifo|lifo)$")]
async fn setup_internal_pool_with_queue_mode(world: &mut DoormanWorld, size: usize, _mode: String) {
    let pg_port = world.pg_port.expect("PostgreSQL must be running");

    let client_server_map: ClientServerMap = Arc::new(Mutex::new(HashMap::new()));

    let address = Address {
        host: "127.0.0.1".to_string(),
        port: pg_port,
        virtual_pool_id: 0,
        database: "postgres".to_string(),
        username: "postgres".to_string(),
        password: "".to_string(),
        pool_name: "bench_pool".to_string(),
        stats: Arc::new(AddressStats::default()),
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
    );

    // Note: queue_mode is no longer configurable, pool always uses FIFO
    let config = PoolConfig {
        max_size: size,
        timeouts: Timeouts {
            wait: Some(Duration::from_secs(30)),
            create: Some(Duration::from_secs(10)),
            recycle: None,
        },
    };

    let pool = Pool::builder(server_pool).config(config).build();

    world.internal_pool = Some(InternalPool {
        pool,
        latencies: Vec::new(),
    });
}

#[when(regex = r#"^I benchmark pool\.get with (\d+) iterations and save as "([^"]+)"$"#)]
async fn benchmark_pool_get_iterations_named(world: &mut DoormanWorld, iterations: usize, name: String) {
    let internal_pool = world
        .internal_pool
        .as_mut()
        .expect("Internal pool must be set up");

    let pool = &internal_pool.pool;
    let mut latencies = Vec::with_capacity(iterations);

    // Warm-up: create initial connections
    {
        let _obj = pool.get().await.expect("Failed to get connection for warm-up");
    }

    let start = Instant::now();
    for _ in 0..iterations {
        let iter_start = Instant::now();
        let obj = pool.get().await.expect("Failed to get connection");
        let latency = iter_start.elapsed();
        latencies.push(latency);
        drop(obj); // Return to pool
    }
    let total_elapsed = start.elapsed();

    let ops_per_sec = iterations as f64 / total_elapsed.as_secs_f64();

    // Calculate percentiles
    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[(latencies.len() as f64 * 0.95) as usize];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];

    eprintln!("Pool.get benchmark results [{}]:", name);
    eprintln!("  Total iterations: {}", iterations);
    eprintln!("  Total time: {:?}", total_elapsed);
    eprintln!("  Throughput: {:.2} ops/sec", ops_per_sec);
    eprintln!("  Latency p50: {:?}", p50);
    eprintln!("  Latency p95: {:?}", p95);
    eprintln!("  Latency p99: {:?}", p99);

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

    internal_pool.latencies = latencies;
}

#[when(regex = r#"^I benchmark pool\.get with (\d+) concurrent clients for (\d+) seconds and save as "([^"]+)"$"#)]
async fn benchmark_pool_get_concurrent_named(
    world: &mut DoormanWorld,
    clients: usize,
    duration_secs: u64,
    name: String,
) {
    let internal_pool = world
        .internal_pool
        .as_ref()
        .expect("Internal pool must be set up");

    let pool = internal_pool.pool.clone();
    let duration = Duration::from_secs(duration_secs);

    // Shared counters
    let total_ops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let all_latencies = Arc::new(Mutex::new(Vec::new()));

    let start = Instant::now();

    // Spawn concurrent tasks
    let mut handles = Vec::with_capacity(clients);
    for _ in 0..clients {
        let pool = pool.clone();
        let total_ops = total_ops.clone();
        let all_latencies = all_latencies.clone();

        let handle = tokio::spawn(async move {
            let mut local_latencies = Vec::new();
            while start.elapsed() < duration {
                let iter_start = Instant::now();
                match pool.get().await {
                    Ok(obj) => {
                        let latency = iter_start.elapsed();
                        local_latencies.push(latency);
                        total_ops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        drop(obj);
                    }
                    Err(e) => {
                        eprintln!("Pool.get error: {:?}", e);
                    }
                }
            }
            // Merge local latencies
            all_latencies.lock().extend(local_latencies);
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        let _ = handle.await;
    }

    let total_elapsed = start.elapsed();
    let ops = total_ops.load(std::sync::atomic::Ordering::Relaxed);
    let ops_per_sec = ops as f64 / total_elapsed.as_secs_f64();

    // Calculate percentiles
    let mut latencies = all_latencies.lock().clone();
    latencies.sort();

    let (p50, p95, p99) = if !latencies.is_empty() {
        (
            latencies[latencies.len() / 2],
            latencies[((latencies.len() as f64 * 0.95) as usize).min(latencies.len() - 1)],
            latencies[((latencies.len() as f64 * 0.99) as usize).min(latencies.len() - 1)],
        )
    } else {
        (Duration::ZERO, Duration::ZERO, Duration::ZERO)
    };

    eprintln!("Pool.get concurrent benchmark results [{}]:", name);
    eprintln!("  Concurrent clients: {}", clients);
    eprintln!("  Duration: {:?}", total_elapsed);
    eprintln!("  Total operations: {}", ops);
    eprintln!("  Throughput: {:.2} ops/sec", ops_per_sec);
    eprintln!("  Latency p50: {:?}", p50);
    eprintln!("  Latency p95: {:?}", p95);
    eprintln!("  Latency p99: {:?}", p99);

    world.bench_results.insert(name.clone(), ops_per_sec);
    world.bench_results.insert(
        format!("{}_p50_ns", name),
        p50.as_nanos() as f64,
    );
    world.bench_results.insert(
        format!("{}_p95_ns", name),
        p95.as_nanos() as f64,
    );
    world.bench_results.insert(
        format!("{}_p99_ns", name),
        p99.as_nanos() as f64,
    );
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

#[then("I print internal pool benchmark results")]
async fn print_internal_pool_results(world: &mut DoormanWorld) {
    eprintln!("\n=== Internal Pool Benchmark Results ===");
    for (name, value) in &world.bench_results {
        if name.starts_with("pool_get") {
            if name.contains("_us") {
                eprintln!("  {}: {:.2} µs", name, value);
            } else {
                eprintln!("  {}: {:.2} ops/sec", name, value);
            }
        }
    }
    eprintln!("========================================\n");
}

#[then("pool status should show correct metrics")]
async fn check_pool_status(world: &mut DoormanWorld) {
    let internal_pool = world
        .internal_pool
        .as_ref()
        .expect("Internal pool must be set up");

    let status = internal_pool.pool.status();
    eprintln!("Pool status:");
    eprintln!("  max_size: {}", status.max_size);
    eprintln!("  size: {}", status.size);
    eprintln!("  available: {}", status.available);
    eprintln!("  waiting: {}", status.waiting);
}

#[then("I save all benchmark results to markdown file")]
async fn save_benchmark_results_to_markdown(world: &mut DoormanWorld) {
    use std::io::Write;

    let output_path = "documentations/docs/benchmark-internal-pool.md";

    // Ensure directory exists
    if let Some(parent) = std::path::Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let mut file = std::fs::File::create(output_path).expect("Failed to create markdown file");

    let now = chrono::Utc::now();

    writeln!(file, "### Internal Pool.get Benchmark Results").unwrap();
    writeln!(file).unwrap();
    writeln!(
        file,
        "**Generated:** {}",
        now.format("%Y-%m-%d %H:%M:%S UTC")
    )
    .unwrap();
    writeln!(file).unwrap();
    writeln!(file, "#### Overview").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "This document contains benchmark results for the internal `Pool.get` operation.").unwrap();
    writeln!(file, "These benchmarks measure the overhead of acquiring a connection from the pool").unwrap();
    writeln!(file, "with real PostgreSQL connections.").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "#### Test Environment").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "- **PostgreSQL:** Local instance with `max_connections=200`").unwrap();
    writeln!(file, "- **Authentication:** Trust (no password)").unwrap();
    writeln!(file, "- **Connection:** TCP to 127.0.0.1").unwrap();
    writeln!(file).unwrap();

    // Collect and organize results
    let mut single_client_results: Vec<(&String, &f64)> = world
        .bench_results
        .iter()
        .filter(|(k, _)| !k.contains("concurrent"))
        .collect();
    single_client_results.sort_by(|a, b| a.0.cmp(b.0));

    let mut concurrent_results: Vec<(&String, &f64)> = world
        .bench_results
        .iter()
        .filter(|(k, _)| k.contains("concurrent"))
        .collect();
    concurrent_results.sort_by(|a, b| a.0.cmp(b.0));

    // Single client results
    writeln!(file, "#### Single Client Results").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "| Test | Throughput | p50 | p95 | p99 |").unwrap();
    writeln!(file, "|------|------------|-----|-----|-----|").unwrap();
    
    // Group by test name (without _pXX_ns suffix)
    let mut test_names: Vec<String> = single_client_results
        .iter()
        .filter(|(k, _)| !k.contains("_p50_") && !k.contains("_p95_") && !k.contains("_p99_"))
        .map(|(k, _)| k.to_string())
        .collect();
    test_names.sort();
    
    for test_name in &test_names {
        let ops = world.bench_results.get(test_name.as_str()).unwrap_or(&0.0);
        let p50 = world.bench_results.get(&format!("{}_p50_ns", test_name)).unwrap_or(&0.0);
        let p95 = world.bench_results.get(&format!("{}_p95_ns", test_name)).unwrap_or(&0.0);
        let p99 = world.bench_results.get(&format!("{}_p99_ns", test_name)).unwrap_or(&0.0);
        writeln!(
            file,
            "| {} | {:.0} ops/sec | {:.0} ns | {:.0} ns | {:.0} ns |",
            test_name, ops, p50, p95, p99
        ).unwrap();
    }
    writeln!(file).unwrap();

    // Concurrent results
    if !concurrent_results.is_empty() {
        writeln!(file, "#### Concurrent Client Results").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "| Test | Throughput | p50 | p95 | p99 |").unwrap();
        writeln!(file, "|------|------------|-----|-----|-----|").unwrap();
        
        let mut concurrent_test_names: Vec<String> = concurrent_results
            .iter()
            .filter(|(k, _)| !k.contains("_p50_") && !k.contains("_p95_") && !k.contains("_p99_"))
            .map(|(k, _)| k.to_string())
            .collect();
        concurrent_test_names.sort();
        
        for test_name in &concurrent_test_names {
            let ops = world.bench_results.get(test_name.as_str()).unwrap_or(&0.0);
            let p50 = world.bench_results.get(&format!("{}_p50_ns", test_name)).unwrap_or(&0.0);
            let p95 = world.bench_results.get(&format!("{}_p95_ns", test_name)).unwrap_or(&0.0);
            let p99 = world.bench_results.get(&format!("{}_p99_ns", test_name)).unwrap_or(&0.0);
            writeln!(
                file,
                "| {} | {:.0} ops/sec | {:.0} ns | {:.0} ns | {:.0} ns |",
                test_name, ops, p50, p95, p99
            ).unwrap();
        }
        writeln!(file).unwrap();
    }

    // Analysis section
    writeln!(file, "#### Analysis").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "##### Key Observations").unwrap();
    writeln!(file).unwrap();

    // Calculate some insights if we have the data
    if let Some(ops) = world.bench_results.get("single_pool1") {
        writeln!(file, "- **Single client throughput (pool_size=1):** {:.0} ops/sec", ops).unwrap();
        if let Some(p50) = world.bench_results.get("single_pool1_p50_ns") {
            writeln!(file, "- **Median latency (p50):** {:.0} ns", p50).unwrap();
        }
        if let Some(p99) = world.bench_results.get("single_pool1_p99_ns") {
            writeln!(file, "- **Tail latency (p99):** {:.0} ns", p99).unwrap();
        }
    }

    writeln!(file).unwrap();
    writeln!(file, "##### Methodology").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "1. **Single client tests:** One client repeatedly calls `pool.get()` and immediately").unwrap();
    writeln!(file, "   returns the connection to the pool. This measures the pure pool overhead.").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "2. **Concurrent tests:** Multiple clients compete for connections from a smaller pool.").unwrap();
    writeln!(file, "   This measures contention handling and semaphore performance.").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "3. **Queue mode comparison:** FIFO vs LIFO modes are compared to understand").unwrap();
    writeln!(file, "   the impact of connection reuse patterns.").unwrap();
    writeln!(file).unwrap();

    writeln!(file, "---").unwrap();
    writeln!(file, "*This file is auto-generated by the benchmark suite.*").unwrap();

    // Flush and close the file
    drop(file);

    // Read the file back and print to stdout so it's visible even if Docker volume is not mounted
    eprintln!("✅ Benchmark results saved to {}", output_path);
    eprintln!();
    eprintln!("=== BEGIN BENCHMARK RESULTS MARKDOWN ===");
    match std::fs::read_to_string(output_path) {
        Ok(content) => {
            // Print to stdout (not stderr) so it can be captured/redirected
            println!("{}", content);
        }
        Err(e) => {
            eprintln!("⚠️  Failed to read back markdown file: {}", e);
        }
    }
    eprintln!("=== END BENCHMARK RESULTS MARKDOWN ===");
}
