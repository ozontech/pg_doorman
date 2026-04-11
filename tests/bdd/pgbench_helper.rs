use crate::utils::create_temp_file;
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given, then, when};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

/// Default timeout for pgbench commands (60 seconds)
const PGBENCH_TIMEOUT_SECS: u64 = 60;

/// Latency percentiles (p50, p95, p99) in milliseconds
#[derive(Debug, Clone)]
pub struct LatencyPercentiles {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

/// Status of a pgbench execution
enum PgbenchStatus {
    Success,
    Failed,
    TimedOut,
    SpawnError,
}

/// Result from a pgbench run, carrying output text and log directory for latency computation
struct PgbenchResult {
    /// Combined stdout+stderr output
    output: String,
    /// Execution status
    status: PgbenchStatus,
    /// Temp directory containing --log files (auto-cleaned on drop)
    log_dir: Option<TempDir>,
}

/// Compute a specific percentile index for a given collection size.
/// Uses nearest-rank method: index = ceil(n * p) - 1, clamped to valid range.
fn percentile_index(n: usize, p: f64) -> usize {
    ((n as f64 * p).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1)
}

/// Parse pgbench --log files and compute latency percentiles (p50, p95, p99) in milliseconds.
/// pgbench log format (PG14): client_id transaction_no time_us script_no epoch epoch_us
/// Column 2 (0-indexed) = transaction latency in microseconds.
fn compute_latency_percentiles(log_dir: &std::path::Path) -> Option<LatencyPercentiles> {
    let mut latencies_us: Vec<f64> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("pgbench_log") {
                continue;
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                for line in contents.lines() {
                    if let Some(Ok(latency)) =
                        line.split_whitespace().nth(2).map(|s| s.parse::<f64>())
                    {
                        latencies_us.push(latency);
                    }
                }
            }
        }
    }

    if latencies_us.is_empty() {
        return None;
    }

    let n = latencies_us.len();

    // Sort once — needed for multiple percentiles.
    // For 3 percentiles, full sort is simpler and not much slower than 3x select_nth.
    latencies_us.sort_unstable_by(f64::total_cmp);

    let p50_us = latencies_us[percentile_index(n, 0.50)];
    let p95_us = latencies_us[percentile_index(n, 0.95)];
    let p99_us = latencies_us[percentile_index(n, 0.99)];

    Some(LatencyPercentiles {
        p50: p50_us / 1000.0,
        p95: p95_us / 1000.0,
        p99: p99_us / 1000.0,
    })
}

/// Create a pgbench script file once at the beginning of the scenario
/// The file will be reused for all pgbench runs via ${PGBENCH_FILE} placeholder
#[given("pgbench script file:")]
pub async fn create_pgbench_script_file(world: &mut DoormanWorld, step: &Step) {
    let script_content = step
        .docstring
        .as_ref()
        .expect("pgbench script content not found in docstring")
        .to_string();

    let script_file = create_temp_file(&script_content);

    eprintln!("Created pgbench script file: {:?}", script_file.path());
    eprintln!("Script content:\n{}", script_content);

    world.pgbench_script_file = Some(script_file);
}

/// Run pgbench command, injecting --log flags for latency collection.
/// Returns PgbenchResult with output text, status, and temp log directory.
fn run_pgbench(command: &str, target: &str, timeout: Duration) -> PgbenchResult {
    let shell = if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "sh"
    };
    let shell_arg = if cfg!(target_os = "windows") {
        "/C"
    } else {
        "-c"
    };

    // Create temp directory for pgbench log files
    let log_dir = TempDir::new().ok();
    let command = if let Some(ref dir) = log_dir {
        let prefix = format!("{}/pgbench_log", dir.path().display());
        command.replacen(
            "pgbench ",
            &format!("pgbench -l --log-prefix={} ", prefix),
            1,
        )
    } else {
        command.to_string()
    };

    let mut cmd = Command::new(shell);
    cmd.arg(shell_arg).arg(&command);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Clone target for use in threads
    let target_stdout = target.to_string();
    let target_stderr = target.to_string();

    match cmd.spawn() {
        Ok(mut child) => {
            let start = std::time::Instant::now();

            // Take stdout and stderr handles
            let stdout_handle = child.stdout.take();
            let stderr_handle = child.stderr.take();

            // Create channels for collecting output from threads
            let (stdout_tx, stdout_rx) = std::sync::mpsc::channel::<String>();
            let (stderr_tx, stderr_rx) = std::sync::mpsc::channel::<String>();

            // Spawn thread to read stdout - always stream output for bench tests
            let stdout_thread = stdout_handle.map(|stdout| {
                std::thread::spawn(move || {
                    let reader = BufReader::new(stdout);
                    let mut collected = String::new();
                    for line in reader.lines().map_while(Result::ok) {
                        eprintln!("[{} stdout] {}", target_stdout, line);
                        collected.push_str(&line);
                        collected.push('\n');
                    }
                    let _ = stdout_tx.send(collected);
                })
            });

            // Spawn thread to read stderr - always stream output for bench tests
            let stderr_thread = stderr_handle.map(|stderr| {
                std::thread::spawn(move || {
                    let reader = BufReader::new(stderr);
                    let mut collected = String::new();
                    for line in reader.lines().map_while(Result::ok) {
                        eprintln!("[{} stderr] {}", target_stderr, line);
                        collected.push_str(&line);
                        collected.push('\n');
                    }
                    let _ = stderr_tx.send(collected);
                })
            });

            // Wait for the command with timeout
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Process finished, wait for output threads to complete
                        if let Some(t) = stdout_thread {
                            let _ = t.join();
                        }
                        if let Some(t) = stderr_thread {
                            let _ = t.join();
                        }

                        let stdout = stdout_rx.recv().unwrap_or_default();
                        let stderr = stderr_rx.recv().unwrap_or_default();

                        return if status.success() {
                            PgbenchResult {
                                output: format!("{}\n{}", stdout, stderr),
                                status: PgbenchStatus::Success,
                                log_dir,
                            }
                        } else {
                            PgbenchResult {
                                output: format!(
                                    "pgbench failed with exit code {:?}\nstdout:\n{}\nstderr:\n{}",
                                    status.code(),
                                    stdout,
                                    stderr
                                ),
                                status: PgbenchStatus::Failed,
                                log_dir,
                            }
                        };
                    }
                    Ok(None) => {
                        let elapsed = start.elapsed();

                        if elapsed > timeout {
                            let _ = child.kill();
                            let _ = child.wait();

                            if let Some(t) = stdout_thread {
                                let _ = t.join();
                            }
                            if let Some(t) = stderr_thread {
                                let _ = t.join();
                            }

                            let stdout = stdout_rx.recv().unwrap_or_default();
                            let stderr = stderr_rx.recv().unwrap_or_default();

                            return PgbenchResult {
                                output: format!(
                                    "pgbench timed out after {} seconds\nstdout:\n{}\nstderr:\n{}",
                                    timeout.as_secs(),
                                    stdout,
                                    stderr
                                ),
                                status: PgbenchStatus::TimedOut,
                                log_dir,
                            };
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => {
                        return PgbenchResult {
                            output: format!("Error waiting for pgbench: {}", e),
                            status: PgbenchStatus::SpawnError,
                            log_dir,
                        };
                    }
                }
            }
        }
        Err(e) => PgbenchResult {
            output: format!("Failed to execute pgbench: {}", e),
            status: PgbenchStatus::SpawnError,
            log_dir,
        },
    }
}

/// Parse TPS (transactions per second) from pgbench output
/// Looks for patterns like:
/// - "tps = 1234.567890 (without initial connection time)"
/// - "tps = 1234.567890 (including connections establishing)"
fn parse_tps(output: &str) -> Option<f64> {
    // Try to find "tps = " pattern and extract the number
    for line in output.lines() {
        if let Some(pos) = line.find("tps = ") {
            let after_tps = &line[pos + 6..];
            // Extract the number (until space or end of string)
            let num_str: String = after_tps
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(tps) = num_str.parse::<f64>() {
                return Some(tps);
            }
        }
    }
    None
}

/// Parse TPS from progress lines when pgbench times out or hangs
/// Looks for patterns like:
/// - "progress: 28.0 s, 26161.5 tps, lat 3.739 ms stddev 2.837, 0 failed"
///   Returns average of all non-zero TPS values found
fn parse_progress_tps(output: &str) -> Option<f64> {
    let mut tps_values: Vec<f64> = Vec::new();

    for line in output.lines() {
        // Look for progress lines with tps
        if line.contains("progress:") && line.contains(" tps,") {
            // Find the tps value: "progress: 28.0 s, 26161.5 tps,"
            if let Some(tps_pos) = line.find(" tps,") {
                // Go backwards from " tps," to find the number
                let before_tps = &line[..tps_pos];
                // Find the last comma before tps
                if let Some(comma_pos) = before_tps.rfind(", ") {
                    let num_str = before_tps[comma_pos + 2..].trim();
                    if let Ok(tps) = num_str.parse::<f64>() {
                        if tps > 0.0 {
                            tps_values.push(tps);
                        }
                    }
                }
            }
        }
    }

    if tps_values.is_empty() {
        None
    } else {
        let avg = tps_values.iter().sum::<f64>() / tps_values.len() as f64;
        Some(avg)
    }
}

/// Handle pgbench result: parse TPS, compute latency percentiles, store both
fn handle_pgbench_result(
    result: PgbenchResult,
    target: &str,
    bench_results: &mut HashMap<String, f64>,
    bench_latency: &mut HashMap<String, LatencyPercentiles>,
) {
    // Compute latency percentiles from log files (before log_dir drops)
    let percentiles = result
        .log_dir
        .as_ref()
        .and_then(|dir| compute_latency_percentiles(dir.path()));

    match result.status {
        PgbenchStatus::Success => {
            if let Some(tps) = parse_tps(&result.output) {
                eprintln!("\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m", target, tps);
                bench_results.insert(target.to_string(), tps);
            } else {
                panic!(
                    "Failed to parse TPS from pgbench output for {}:\n{}",
                    target, result.output
                );
            }
        }
        PgbenchStatus::Failed => {
            if result.output.contains("prepared statement")
                && result.output.contains("does not exist")
            {
                eprintln!(
                    "\x1b[1;33m⚠ TPS for {}: 0.00 (prepared statements not supported)\x1b[0m",
                    target
                );
                bench_results.insert(target.to_string(), 0.0);
            } else {
                panic!("pgbench failed for {}: {}", target, result.output);
            }
        }
        PgbenchStatus::TimedOut => {
            if let Some(tps) =
                parse_tps(&result.output).or_else(|| parse_progress_tps(&result.output))
            {
                eprintln!(
                    "\x1b[1;33m⚠ TPS for {} (from progress, timed out): {:.2}\x1b[0m",
                    target, tps
                );
                bench_results.insert(target.to_string(), tps);
            } else {
                eprintln!(
                    "\x1b[1;31m✗ TPS for {}: 0.00 (timed out, no progress data)\x1b[0m",
                    target
                );
                bench_results.insert(target.to_string(), 0.0);
            }
        }
        PgbenchStatus::SpawnError => {
            panic!(
                "Failed to execute pgbench for {}: {}",
                target, result.output
            );
        }
    }

    // Store latency percentiles if computed
    if let Some(p) = percentiles {
        eprintln!(
            "\x1b[1;36m  latency for {}: p50={:.2} ms, p95={:.2} ms, p99={:.2} ms\x1b[0m",
            target, p.p50, p.p95, p.p99
        );
        bench_latency.insert(target.to_string(), p);
    }
    // result.log_dir drops here, cleaning up temp files
}

/// Run pgbench and store result for a target
#[when(expr = "I run pgbench for {string} with:")]
pub async fn run_pgbench_for_target(world: &mut DoormanWorld, target: String, step: &Step) {
    let command = step
        .docstring
        .as_ref()
        .expect("pgbench command not found in docstring")
        .to_string();

    let command = world.replace_placeholders(&command);

    eprintln!("Running pgbench for {}: {}", target, command);

    let result = run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS));
    handle_pgbench_result(
        result,
        &target,
        &mut world.bench_results,
        &mut world.bench_latency,
    );
}

/// Run pgbench with inline command (single line)
/// The command should be clean pgbench options like: "-h 127.0.0.1 -p 5432 -U postgres -c 10 -j 2 -T 10 postgres -f ${PGBENCH_FILE}"
#[when(expr = "I run pgbench for {string} with {string}")]
pub async fn run_pgbench_for_target_inline(
    world: &mut DoormanWorld,
    target: String,
    options: String,
) {
    // Record start time on first pgbench run
    if world.bench_start_time.is_none() {
        world.bench_start_time = Some(chrono::Utc::now());
    }

    let options = world.replace_placeholders(&options);

    // Build the full pgbench command
    let command = format!("pgbench {}", options);

    eprintln!("Running pgbench for {}: {}", target, command);

    let result = run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS));
    handle_pgbench_result(
        result,
        &target,
        &mut world.bench_results,
        &mut world.bench_latency,
    );
}

/// Run pgbench with inline command and explicit environment variables
/// Example: When I run pgbench for "target" with "-h 127.0.0.1 -p 5432 ..." and env "PGSSLMODE=require"
#[when(expr = "I run pgbench for {string} with {string} and env {string}")]
pub async fn run_pgbench_for_target_with_env(
    world: &mut DoormanWorld,
    target: String,
    options: String,
    env_vars: String,
) {
    // Record start time on first pgbench run
    if world.bench_start_time.is_none() {
        world.bench_start_time = Some(chrono::Utc::now());
    }

    let options = world.replace_placeholders(&options);

    // Build the full pgbench command with env prefix
    let command = format!("{} pgbench {}", env_vars, options);

    eprintln!("Running pgbench for {}: {}", target, command);

    let result = run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS));
    handle_pgbench_result(
        result,
        &target,
        &mut world.bench_results,
        &mut world.bench_latency,
    );
}

/// Run pgbench with a script file (script content in docstring, options inline)
/// Example: When I run pgbench for "postgresql" with options "-h 127.0.0.1 -p 5432 -U postgres -c 10 -j 2 -T 10 postgres" and script:
/// For SSL, use env prefix: "PGSSLMODE=require" will be extracted and set as environment variable
#[when(expr = "I run pgbench for {string} with options {string} and script:")]
pub async fn run_pgbench_with_script(
    world: &mut DoormanWorld,
    target: String,
    options: String,
    step: &Step,
) {
    let script_content = step
        .docstring
        .as_ref()
        .expect("pgbench script not found in docstring")
        .to_string();

    // Create a temporary file for the script
    let script_file = create_temp_file(&script_content);
    let script_path = script_file.path().to_str().unwrap().to_string();

    // Replace placeholders in options
    let options = world.replace_placeholders(&options);

    // Extract environment variables from options (e.g., "PGSSLMODE=require -h ...")
    let (env_prefix, pgbench_options) = extract_env_prefix(&options);

    // Build the pgbench command with -f option
    let command = format!(
        "{}pgbench -f {} {}",
        env_prefix, script_path, pgbench_options
    );

    eprintln!(
        "Running pgbench for {} with script file: {}",
        target, command
    );
    eprintln!("Script content:\n{}", script_content);

    let result = run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS));
    handle_pgbench_result(
        result,
        &target,
        &mut world.bench_results,
        &mut world.bench_latency,
    );
}

/// Extract environment variable prefix from options string
/// E.g., "PGSSLMODE=require -h 127.0.0.1" -> ("PGSSLMODE=require ", "-h 127.0.0.1")
fn extract_env_prefix(options: &str) -> (String, String) {
    let mut env_vars = Vec::new();
    let mut remaining_parts = Vec::new();

    for part in options.split_whitespace() {
        if part.contains('=') && !part.starts_with('-') {
            // This looks like an environment variable (e.g., PGSSLMODE=require)
            env_vars.push(part.to_string());
        } else {
            remaining_parts.push(part.to_string());
        }
    }

    if env_vars.is_empty() {
        (String::new(), options.to_string())
    } else {
        (
            format!("{} ", env_vars.join(" ")),
            remaining_parts.join(" "),
        )
    }
}

/// Verify that benchmark results exist for a target
#[then(expr = "benchmark result for {string} should exist")]
pub async fn benchmark_result_should_exist(world: &mut DoormanWorld, target: String) {
    if !world.bench_results.contains_key(&target) {
        panic!(
            "Benchmark result for '{}' not found. Available results: {:?}",
            target,
            world.bench_results.keys().collect::<Vec<_>>()
        );
    }
}

/// Print benchmark results summary
#[then("I print benchmark results")]
pub async fn print_benchmark_results(world: &mut DoormanWorld) {
    eprintln!("\n=== Benchmark Results ===");

    // Group results by client count
    let client_counts = ["c1", "c10", "c50", "c100", "c200"];

    for client_count in &client_counts {
        let baseline_key = format!("postgresql_{}", client_count);
        let baseline_tps = world.bench_results.get(&baseline_key).copied();

        eprintln!("\n--- {} clients ---", &client_count[1..]); // Remove 'c' prefix for display

        // Print baseline first
        if let Some(tps) = baseline_tps {
            eprintln!("  postgresql: {:.2} tps (baseline)", tps);
        }

        // Print other results for this client count
        for (target, tps) in &world.bench_results {
            if target.ends_with(&format!("_{}", client_count)) && !target.starts_with("postgresql_")
            {
                let latency_info = world
                    .bench_latency
                    .get(target)
                    .map(|p| {
                        format!(
                            " | p50={:.2}ms p95={:.2}ms p99={:.2}ms",
                            p.p50, p.p95, p.p99
                        )
                    })
                    .unwrap_or_default();

                if let Some(baseline) = baseline_tps {
                    if baseline > 0.0 {
                        let normalized = tps / baseline;
                        let pooler_name = target
                            .strip_suffix(&format!("_{}", client_count))
                            .unwrap_or(target);
                        eprintln!(
                            "  {}: {:.2} tps (normalized: {:.4}x){}",
                            pooler_name, tps, normalized, latency_info
                        );
                    }
                } else {
                    eprintln!("  {}: {:.2} tps{}", target, tps, latency_info);
                }
            }
        }
    }
}

/// Generate a Markdown table with benchmark results and save to file
/// The table shows relative performance: pg_doorman/pgbouncer and pg_doorman/odyssey
/// Value > 1.0 means pg_doorman is faster than competitor
#[then("I generate benchmark markdown table")]
pub async fn generate_benchmark_markdown_table(world: &mut DoormanWorld) {
    eprintln!("\n=== Generating Benchmark Markdown Table ===");

    // Helper function to format ratio as percentage difference or multiplier
    // 1.10 -> "+10%" (pg_doorman is 10% faster)
    // 0.90 -> "-10%" (pg_doorman is 10% slower)
    // 2.50 -> "x2.5" (pg_doorman is 2.5x faster, used when difference > 100%)
    // 0.40 -> "x0.4" (pg_doorman is 2.5x slower, used when difference > 100%)
    let format_ratio = |doorman: f64, competitor_tps: Option<f64>| -> String {
        match competitor_tps {
            Some(v) if v > 0.0 && doorman > 0.0 => {
                let ratio = doorman / v;
                let percent = (ratio - 1.0) * 100.0;
                if percent.abs() < 3.0 {
                    "≈0%".to_string()
                } else if percent > 100.0 {
                    // More than 2x faster - show as multiplier
                    format!("x{:.1}", ratio)
                } else if percent < -50.0 {
                    // More than 2x slower - show as multiplier
                    format!("x{:.1}", ratio)
                } else if percent > 0.0 {
                    format!("+{:.0}%", percent)
                } else {
                    format!("{:.0}%", percent)
                }
            }
            Some(_) if doorman > 0.0 => "∞".to_string(), // competitor failed, pg_doorman wins
            Some(_) => "N/A".to_string(),
            None => "-".to_string(),
        }
    };

    // Helper function to generate table rows for a set of test configs
    let generate_table = |configs: &[(&str, &str)],
                          results: &std::collections::HashMap<String, f64>|
     -> Vec<String> {
        let mut rows = Vec::new();
        rows.push("| Test | vs pgbouncer | vs odyssey |".to_string());
        rows.push("|------|--------------|------------|".to_string());

        for (suffix, display_name) in configs {
            let doorman_key = format!("pg_doorman_{}", suffix);
            let pgbouncer_key = format!("pgbouncer_{}", suffix);
            let odyssey_key = format!("odyssey_{}", suffix);

            let doorman_tps = results.get(&doorman_key).copied();
            let pgbouncer_tps = results.get(&pgbouncer_key).copied();
            let odyssey_tps = results.get(&odyssey_key).copied();

            // Skip if no pg_doorman results for this test
            if doorman_tps.is_none() {
                continue;
            }

            let doorman = doorman_tps.unwrap_or(0.0);

            let row = format!(
                "| {} | {} | {} |",
                display_name,
                format_ratio(doorman, pgbouncer_tps),
                format_ratio(doorman, odyssey_tps)
            );
            rows.push(row);
        }
        rows
    };

    // Simple Protocol tests
    let simple_configs: Vec<(&str, &str)> = vec![
        ("simple_c1", "1 client"),
        ("simple_c40", "40 clients"),
        ("simple_c120", "120 clients"),
        ("simple_c500", "500 clients"),
        ("simple_c10000", "10,000 clients"),
        ("simple_connect_c1", "1 client + Reconnect"),
        ("simple_connect_c40", "40 clients + Reconnect"),
        ("simple_connect_c120", "120 clients + Reconnect"),
        ("simple_connect_c500", "500 clients + Reconnect"),
        ("simple_connect_c10000", "10,000 clients + Reconnect"),
        ("ssl_simple_c1", "1 client + SSL"),
        ("ssl_simple_c40", "40 clients + SSL"),
        ("ssl_simple_c120", "120 clients + SSL"),
        ("ssl_simple_c500", "500 clients + SSL"),
        ("ssl_simple_c10000", "10,000 clients + SSL"),
    ];

    // Extended Protocol tests
    let extended_configs: Vec<(&str, &str)> = vec![
        ("extended_c1", "1 client"),
        ("extended_c40", "40 clients"),
        ("extended_c120", "120 clients"),
        ("extended_c500", "500 clients"),
        ("extended_c10000", "10,000 clients"),
        ("extended_connect_c1", "1 client + Reconnect"),
        ("extended_connect_c40", "40 clients + Reconnect"),
        ("extended_connect_c120", "120 clients + Reconnect"),
        ("extended_connect_c500", "500 clients + Reconnect"),
        ("extended_connect_c10000", "10,000 clients + Reconnect"),
        ("ssl_extended_c1", "1 client + SSL"),
        ("ssl_extended_c40", "40 clients + SSL"),
        ("ssl_extended_c120", "120 clients + SSL"),
        ("ssl_extended_c500", "500 clients + SSL"),
        ("ssl_extended_c10000", "10,000 clients + SSL"),
        ("ssl_connect_c1", "1 client + SSL + Reconnect"),
        ("ssl_connect_c40", "40 clients + SSL + Reconnect"),
        ("ssl_connect_c120", "120 clients + SSL + Reconnect"),
        ("ssl_connect_c500", "500 clients + SSL + Reconnect"),
        ("ssl_connect_c10000", "10,000 clients + SSL + Reconnect"),
    ];

    // Prepared Protocol tests
    let prepared_configs: Vec<(&str, &str)> = vec![
        ("prepared_c1", "1 client"),
        ("prepared_c40", "40 clients"),
        ("prepared_c120", "120 clients"),
        ("prepared_c500", "500 clients"),
        ("prepared_c10000", "10,000 clients"),
        ("prepared_connect_c1", "1 client + Reconnect"),
        ("prepared_connect_c40", "40 clients + Reconnect"),
        ("prepared_connect_c120", "120 clients + Reconnect"),
        ("prepared_connect_c500", "500 clients + Reconnect"),
        ("prepared_connect_c10000", "10,000 clients + Reconnect"),
        ("ssl_prepared_c1", "1 client + SSL"),
        ("ssl_prepared_c40", "40 clients + SSL"),
        ("ssl_prepared_c120", "120 clients + SSL"),
        ("ssl_prepared_c500", "500 clients + SSL"),
        ("ssl_prepared_c10000", "10,000 clients + SSL"),
    ];

    let simple_table = generate_table(&simple_configs, &world.bench_results);
    let extended_table = generate_table(&extended_configs, &world.bench_results);
    let prepared_table = generate_table(&prepared_configs, &world.bench_results);

    // Helper to generate latency percentile table.
    // Each cell shows "p50 / p95 / p99" in ms — compact enough for 4 columns.
    let generate_latency_table =
        |configs: &[(&str, &str)],
         latency: &std::collections::HashMap<String, LatencyPercentiles>|
         -> Vec<String> {
            let mut rows = Vec::new();
            rows.push("| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |".to_string());
            rows.push("|------|----------------|----------------|--------------|".to_string());

            let fmt = |key: &str| -> String {
                match latency.get(key) {
                    Some(p) => format!("{:.2} / {:.2} / {:.2}", p.p50, p.p95, p.p99),
                    None => "-".to_string(),
                }
            };

            for (suffix, display_name) in configs {
                let doorman_key = format!("pg_doorman_{}", suffix);
                if !latency.contains_key(&doorman_key) {
                    continue;
                }
                let pgbouncer_key = format!("pgbouncer_{}", suffix);
                let odyssey_key = format!("odyssey_{}", suffix);

                rows.push(format!(
                    "| {} | {} | {} | {} |",
                    display_name,
                    fmt(&doorman_key),
                    fmt(&pgbouncer_key),
                    fmt(&odyssey_key)
                ));
            }
            rows
        };

    let simple_latency = generate_latency_table(&simple_configs, &world.bench_latency);
    let extended_latency = generate_latency_table(&extended_configs, &world.bench_latency);
    let prepared_latency = generate_latency_table(&prepared_configs, &world.bench_latency);

    // Environment and parameter info
    let fargate_cpu = std::env::var("FARGATE_CPU").ok().filter(|s| !s.is_empty());
    let fargate_memory = std::env::var("FARGATE_MEMORY")
        .ok()
        .filter(|s| !s.is_empty());
    let doorman_workers = std::env::var("BENCH_DOORMAN_WORKERS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "12".to_string());
    let odyssey_workers = std::env::var("BENCH_ODYSSEY_WORKERS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "12".to_string());
    let pgbench_jobs = std::env::var("BENCH_PGBENCH_JOBS")
        .ok()
        .filter(|s| !s.is_empty());

    let mut env_info = Vec::new();
    if let (Some(cpu), Some(mem)) = (fargate_cpu, fargate_memory) {
        let vcpu = cpu.parse::<f64>().unwrap_or(0.0) / 1024.0;
        let gb = mem.parse::<f64>().unwrap_or(0.0) / 1024.0;
        env_info.push(format!(
            "- **Instance**: AWS Fargate ({:.0} vCPU, {:.0} GB RAM)",
            vcpu, gb
        ));
    }

    env_info.push(format!(
        "- **Workers**: pg_doorman: {}, odyssey: {}",
        doorman_workers, odyssey_workers
    ));

    if let Some(jobs) = pgbench_jobs {
        env_info.push(format!("- **pgbench jobs**: {} (global override)", jobs));
    } else {
        env_info.push(
            "- **pgbench jobs**: variable (c1: 1, c40: 4, c120: 4, c500: 4, c10k: 4)".to_string(),
        );
    }

    // Record end time
    let end_time = chrono::Utc::now();
    world.bench_end_time = Some(end_time);

    // Calculate duration
    let duration_info = if let Some(start_time) = world.bench_start_time {
        let duration = end_time.signed_duration_since(start_time);
        let hours = duration.num_hours();
        let minutes = duration.num_minutes() % 60;
        let seconds = duration.num_seconds() % 60;
        format!(
            "- **Started**: {}\n- **Finished**: {}\n- **Total duration**: {}h {}m {}s",
            start_time.format("%Y-%m-%d %H:%M:%S UTC"),
            end_time.format("%Y-%m-%d %H:%M:%S UTC"),
            hours,
            minutes,
            seconds
        )
    } else {
        format!(
            "- **Finished**: {}",
            end_time.format("%Y-%m-%d %H:%M:%S UTC")
        )
    };

    // Generate the full markdown content
    let now = chrono::Utc::now();
    let header = format!(
        r#"---
title: Benchmarks
---

# Performance Benchmarks

## Automated Benchmark Results

Last updated: {}

These benchmarks are automatically generated by the CI pipeline using `pgbench`.

### Test Environment

- **Pool size**: 40 connections
- **Test duration**: 30 seconds per test
{}
{}

### How to Read

**TPS tables** — relative throughput: pg_doorman vs pgbouncer and odyssey.

| Symbol | Meaning |
|--------|---------|
| +N% | pg_doorman is N% faster |
| -N% | pg_doorman is N% slower |
| ≈0% | Within 3%, effectively equal |
| xN | N times faster/slower (large gap) |
| ∞ | Competitor scored 0 TPS |
| N/A | Not supported by this pooler |
| - | Not executed |

**Latency tables** — absolute transaction latency in milliseconds.
Each cell shows `p50 / p95 / p99` percentiles. Lower is better.
"#,
        now.format("%Y-%m-%d %H:%M UTC"),
        env_info.join("\n"),
        duration_info,
    );

    // Combine sections
    let full_markdown = format!(
        "{}\n---\n\n\
         ## Simple Protocol\n\n\
         ### Throughput\n\n{}\n\n\
         ### Latency — p50 / p95 / p99 (ms)\n\n{}\n\n\
         ---\n\n\
         ## Extended Protocol\n\n\
         ### Throughput\n\n{}\n\n\
         ### Latency — p50 / p95 / p99 (ms)\n\n{}\n\n\
         ---\n\n\
         ## Prepared Protocol\n\n\
         ### Throughput\n\n{}\n\n\
         ### Latency — p50 / p95 / p99 (ms)\n\n{}\n\n\
         ---\n\n\
         ### Notes\n\n\
         - Odyssey has poor support for extended query protocol in transaction pooling mode\n\
         - TPS values are **relative ratios**, not absolute numbers — \
         they stay consistent across sequential runs on the same hardware\n\
         - Latency percentiles are **absolute** values collected via `pgbench --log`\n",
        header,
        simple_table.join("\n"),
        simple_latency.join("\n"),
        extended_table.join("\n"),
        extended_latency.join("\n"),
        prepared_table.join("\n"),
        prepared_latency.join("\n")
    );

    // Write to file
    let file_path = "documentation/en/src/benchmarks.md";
    match std::fs::write(file_path, &full_markdown) {
        Ok(_) => {
            eprintln!(
                "\x1b[1;32m✓ Benchmark table written to {}\x1b[0m",
                file_path
            );
            eprintln!("\n{}", header);
        }
        Err(e) => {
            eprintln!(
                "\x1b[1;31m✗ Failed to write benchmark table to {}: {}\x1b[0m",
                file_path, e
            );
        }
    }
}

#[cfg(test)]
#[allow(unused)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tps() {
        let output1 = r#"
starting vacuum...end.
transaction type: <builtin: TPC-B (sort of)>
scaling factor: 1
query mode: simple
number of clients: 10
number of threads: 2
duration: 10 s
number of transactions actually processed: 12345
latency average = 8.123 ms
tps = 1234.567890 (without initial connection time)
"#;
        assert_eq!(parse_tps(output1), Some(1234.567890));

        let output2 = "tps = 999.123 (including connections establishing)";
        assert_eq!(parse_tps(output2), Some(999.123));

        let output3 = "no tps here";
        assert_eq!(parse_tps(output3), None);

        let output4 = "tps = 0.0 (without initial connection time)";
        assert_eq!(parse_tps(output4), Some(0.0));
    }

    #[test]
    fn test_compute_latency_percentiles_100_values() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("pgbench_log.12345");

        // 100 transactions: latencies 1000, 2000, ..., 100000 microseconds
        let mut content = String::new();
        for i in 1..=100 {
            // client_id  transaction_no  time_us  script_no  epoch  epoch_us
            content.push_str(&format!("0 {} {} 0 0 0\n", i, i * 1000));
        }
        std::fs::write(&log_path, &content).unwrap();

        let p = compute_latency_percentiles(dir.path()).unwrap();
        assert!((p.p50 - 50.0).abs() < 0.01);
        assert!((p.p95 - 95.0).abs() < 0.01);
        assert!((p.p99 - 99.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_latency_percentiles_empty_log() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("pgbench_log.12345");
        std::fs::write(&log_path, "").unwrap();

        assert!(compute_latency_percentiles(dir.path()).is_none());
    }

    #[test]
    fn test_compute_latency_percentiles_no_files() {
        let dir = TempDir::new().unwrap();
        assert!(compute_latency_percentiles(dir.path()).is_none());
    }

    #[test]
    fn test_compute_latency_percentiles_multiple_thread_files() {
        let dir = TempDir::new().unwrap();

        // Thread 0: latencies 1000..50000
        let mut content1 = String::new();
        for i in 1..=50 {
            content1.push_str(&format!("0 {} {} 0 0 0\n", i, i * 1000));
        }
        std::fs::write(dir.path().join("pgbench_log.123.0"), &content1).unwrap();

        // Thread 1: latencies 51000..100000
        let mut content2 = String::new();
        for i in 51..=100 {
            content2.push_str(&format!("0 {} {} 0 0 0\n", i, i * 1000));
        }
        std::fs::write(dir.path().join("pgbench_log.123.1"), &content2).unwrap();

        let p = compute_latency_percentiles(dir.path()).unwrap();
        assert!((p.p50 - 50.0).abs() < 0.01);
        assert!((p.p95 - 95.0).abs() < 0.01);
        assert!((p.p99 - 99.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_latency_percentiles_single_transaction() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("pgbench_log.12345");
        std::fs::write(&log_path, "0 1 5000 0 0 0\n").unwrap();

        let p = compute_latency_percentiles(dir.path()).unwrap();
        // Single value: all percentiles equal
        assert!((p.p50 - 5.0).abs() < 0.01);
        assert!((p.p95 - 5.0).abs() < 0.01);
        assert!((p.p99 - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_latency_percentiles_ignores_non_log_files() {
        let dir = TempDir::new().unwrap();

        // Valid log file
        std::fs::write(
            dir.path().join("pgbench_log.123"),
            "0 1 1000 0 0 0\n0 2 2000 0 0 0\n",
        )
        .unwrap();

        // Non-log file that should be ignored
        std::fs::write(dir.path().join("other_file.txt"), "0 1 99999999 0 0 0\n").unwrap();

        let p = compute_latency_percentiles(dir.path()).unwrap();
        // Only 2 values: 1000 and 2000 us -> 1.0 and 2.0 ms
        assert!(p.p99 <= 2.0);
    }

    #[test]
    fn test_percentile_index() {
        assert_eq!(percentile_index(100, 0.99), 98);
        assert_eq!(percentile_index(100, 0.50), 49);
        assert_eq!(percentile_index(100, 0.95), 94);
        assert_eq!(percentile_index(1, 0.99), 0);
        assert_eq!(percentile_index(10, 0.99), 9);
    }
}
