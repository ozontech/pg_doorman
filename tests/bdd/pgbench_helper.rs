use crate::utils::create_temp_file;
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given, then, when};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Default timeout for pgbench commands (60 seconds)
const PGBENCH_TIMEOUT_SECS: u64 = 60;

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

/// Run pgbench command and return stdout/stderr
fn run_pgbench(command: &str, target: &str, timeout: Duration) -> Result<String, String> {
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

    let mut cmd = Command::new(shell);
    cmd.arg(shell_arg).arg(command);
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
                        // Always print pgbench output to stderr (bench tests should show progress)
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
                        // Always print pgbench output to stderr (bench tests should show progress)
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

                        // Collect output from channels
                        let stdout = stdout_rx.recv().unwrap_or_default();
                        let stderr = stderr_rx.recv().unwrap_or_default();

                        return if status.success() {
                            // Combine stdout and stderr for parsing
                            Ok(format!("{}\n{}", stdout, stderr))
                        } else {
                            Err(format!(
                                "pgbench failed with exit code {:?}\nstdout:\n{}\nstderr:\n{}",
                                status.code(),
                                stdout,
                                stderr
                            ))
                        };
                    }
                    Ok(None) => {
                        let elapsed = start.elapsed();

                        // Check timeout
                        if elapsed > timeout {
                            // Timeout reached, kill the process
                            let _ = child.kill();
                            let _ = child.wait();

                            // Wait for output threads
                            if let Some(t) = stdout_thread {
                                let _ = t.join();
                            }
                            if let Some(t) = stderr_thread {
                                let _ = t.join();
                            }

                            let stdout = stdout_rx.recv().unwrap_or_default();
                            let stderr = stderr_rx.recv().unwrap_or_default();

                            return Err(format!(
                                "pgbench timed out after {} seconds\nstdout:\n{}\nstderr:\n{}",
                                timeout.as_secs(),
                                stdout,
                                stderr
                            ));
                        }
                        // Sleep a bit before checking again
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => {
                        return Err(format!("Error waiting for pgbench: {}", e));
                    }
                }
            }
        }
        Err(e) => Err(format!("Failed to execute pgbench: {}", e)),
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
/// Returns average of all non-zero TPS values found
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

    match run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS)) {
        Ok(output) => {
            if let Some(tps) = parse_tps(&output) {
                // Print TPS with bright colors for visibility
                eprintln!("\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m", target, tps);
                world.bench_results.insert(target, tps);
            } else {
                panic!(
                    "Failed to parse TPS from pgbench output for {}:\n{}",
                    target, output
                );
            }
        }
        Err(e) => {
            panic!("pgbench failed for {}: {}", target, e);
        }
    }
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

    match run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS)) {
        Ok(output) => {
            if let Some(tps) = parse_tps(&output) {
                // Print TPS with bright colors for visibility
                eprintln!("\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m", target, tps);
                world.bench_results.insert(target, tps);
            } else {
                panic!(
                    "Failed to parse TPS from pgbench output for {}:\n{}",
                    target, output
                );
            }
        }
        Err(e) => {
            // Check if this is a "prepared statement does not exist" error (e.g., odyssey doesn't support prepared protocol)
            if e.contains("prepared statement") && e.contains("does not exist") {
                eprintln!(
                    "\x1b[1;33m⚠ TPS for {}: 0.00 (prepared statements not supported)\x1b[0m",
                    target
                );
                world.bench_results.insert(target, 0.0);
            } else if e.contains("timed out") {
                // Try to parse progress lines for average TPS on timeout
                if let Some(tps) = parse_tps(&e).or_else(|| parse_progress_tps(&e)) {
                    eprintln!(
                        "\x1b[1;33m⚠ TPS for {} (from progress, timed out): {:.2}\x1b[0m",
                        target, tps
                    );
                    world.bench_results.insert(target, tps);
                } else {
                    eprintln!(
                        "\x1b[1;31m✗ TPS for {}: 0.00 (timed out, no progress data)\x1b[0m",
                        target
                    );
                    world.bench_results.insert(target, 0.0);
                }
            } else {
                panic!("pgbench failed for {}: {}", target, e);
            }
        }
    }
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

    match run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS)) {
        Ok(output) => {
            if let Some(tps) = parse_tps(&output) {
                // Print TPS with bright colors for visibility
                eprintln!("\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m", target, tps);
                world.bench_results.insert(target, tps);
            } else {
                panic!(
                    "Failed to parse TPS from pgbench output for {}:\n{}",
                    target, output
                );
            }
        }
        Err(e) => {
            // Check if this is a "prepared statement does not exist" error (e.g., odyssey doesn't support prepared protocol)
            if e.contains("prepared statement") && e.contains("does not exist") {
                eprintln!(
                    "\x1b[1;33m⚠ TPS for {}: 0.00 (prepared statements not supported)\x1b[0m",
                    target
                );
                world.bench_results.insert(target, 0.0);
            } else if e.contains("timed out") {
                // Try to parse progress lines for average TPS on timeout
                if let Some(tps) = parse_tps(&e).or_else(|| parse_progress_tps(&e)) {
                    eprintln!(
                        "\x1b[1;33m⚠ TPS for {} (from progress, timed out): {:.2}\x1b[0m",
                        target, tps
                    );
                    world.bench_results.insert(target, tps);
                } else {
                    eprintln!(
                        "\x1b[1;31m✗ TPS for {}: 0.00 (timed out, no progress data)\x1b[0m",
                        target
                    );
                    world.bench_results.insert(target, 0.0);
                }
            } else {
                panic!("pgbench failed for {}: {}", target, e);
            }
        }
    }
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

    match run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS)) {
        Ok(output) => {
            if let Some(tps) = parse_tps(&output) {
                // Print TPS with bright colors for visibility
                eprintln!("\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m", target, tps);
                world.bench_results.insert(target, tps);
            } else {
                panic!(
                    "Failed to parse TPS from pgbench output for {}:\n{}",
                    target, output
                );
            }
        }
        Err(e) => {
            panic!("pgbench failed for {}: {}", target, e);
        }
    }
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

/// Extract test suffix from target name (everything after pooler prefix)
/// Examples:
/// - "pg_doorman_c40" -> "c40"
/// - "pg_doorman_ssl_c40" -> "ssl_c40"
/// - "pg_doorman_extended_connect_c80" -> "extended_connect_c80"
fn extract_test_suffix(target: &str) -> Option<String> {
    let prefixes = ["postgresql_", "pg_doorman_", "odyssey_", "pgbouncer_"];

    for prefix in &prefixes {
        if target.starts_with(prefix) {
            return Some(target[prefix.len()..].to_string());
        }
    }
    None
}

/// Send normalized benchmark results to bencher.dev
/// Compares pg_doorman vs pgbouncer and pg_doorman vs odyssey
/// Value > 1.0 means pg_doorman is faster than competitor
#[then("I send normalized benchmark results to bencher.dev")]
pub async fn send_to_bencher(world: &mut DoormanWorld) {
    // Collect all unique test suffixes from pg_doorman results
    let mut test_suffixes: Vec<String> = world
        .bench_results
        .keys()
        .filter(|k| k.starts_with("pg_doorman_"))
        .filter_map(|k| extract_test_suffix(k))
        .collect();
    test_suffixes.sort();
    test_suffixes.dedup();

    // Print comparison results
    eprintln!("\n=== Benchmark Results (pg_doorman vs competitors) ===");
    eprintln!("Value > 1.0 means pg_doorman is faster\n");

    let mut metrics = serde_json::Map::new();

    for suffix in &test_suffixes {
        let doorman_key = format!("pg_doorman_{}", suffix);
        let pgbouncer_key = format!("pgbouncer_{}", suffix);
        let odyssey_key = format!("odyssey_{}", suffix);

        let doorman_tps = world
            .bench_results
            .get(&doorman_key)
            .copied()
            .unwrap_or(0.0);

        // Compare pg_doorman vs pgbouncer
        if let Some(&pgbouncer_tps) = world.bench_results.get(&pgbouncer_key) {
            if pgbouncer_tps > 0.0 && doorman_tps > 0.0 {
                let ratio = doorman_tps / pgbouncer_tps;
                let metric_name = format!("pg_doorman_vs_pgbouncer_{}", suffix);

                eprintln!(
                    "\x1b[1;36m{}: {:.2} / {:.2} = {:.4}\x1b[0m",
                    metric_name, doorman_tps, pgbouncer_tps, ratio
                );

                let mut metric = serde_json::Map::new();
                let mut throughput = serde_json::Map::new();
                throughput.insert("value".to_string(), serde_json::json!(ratio));
                metric.insert(
                    "throughput".to_string(),
                    serde_json::Value::Object(throughput),
                );
                metrics.insert(metric_name, serde_json::Value::Object(metric));
            } else if doorman_tps > 0.0 {
                // pgbouncer failed (0 tps), pg_doorman wins
                let metric_name = format!("pg_doorman_vs_pgbouncer_{}", suffix);
                eprintln!(
                    "\x1b[1;32m{}: pg_doorman={:.2}, pgbouncer=0 (pg_doorman wins)\x1b[0m",
                    metric_name, doorman_tps
                );
                // Use a high value to indicate pg_doorman is much better
                let mut metric = serde_json::Map::new();
                let mut throughput = serde_json::Map::new();
                throughput.insert("value".to_string(), serde_json::json!(10.0));
                metric.insert(
                    "throughput".to_string(),
                    serde_json::Value::Object(throughput),
                );
                metrics.insert(metric_name, serde_json::Value::Object(metric));
            }
        }

        // Compare pg_doorman vs odyssey
        if let Some(&odyssey_tps) = world.bench_results.get(&odyssey_key) {
            if odyssey_tps > 0.0 && doorman_tps > 0.0 {
                let ratio = doorman_tps / odyssey_tps;
                let metric_name = format!("pg_doorman_vs_odyssey_{}", suffix);

                eprintln!(
                    "\x1b[1;35m{}: {:.2} / {:.2} = {:.4}\x1b[0m",
                    metric_name, doorman_tps, odyssey_tps, ratio
                );

                let mut metric = serde_json::Map::new();
                let mut throughput = serde_json::Map::new();
                throughput.insert("value".to_string(), serde_json::json!(ratio));
                metric.insert(
                    "throughput".to_string(),
                    serde_json::Value::Object(throughput),
                );
                metrics.insert(metric_name, serde_json::Value::Object(metric));
            } else if doorman_tps > 0.0 {
                // odyssey failed (0 tps), pg_doorman wins
                let metric_name = format!("pg_doorman_vs_odyssey_{}", suffix);
                eprintln!(
                    "\x1b[1;32m{}: pg_doorman={:.2}, odyssey=0 (pg_doorman wins)\x1b[0m",
                    metric_name, doorman_tps
                );
                let mut metric = serde_json::Map::new();
                let mut throughput = serde_json::Map::new();
                throughput.insert("value".to_string(), serde_json::json!(10.0));
                metric.insert(
                    "throughput".to_string(),
                    serde_json::Value::Object(throughput),
                );
                metrics.insert(metric_name, serde_json::Value::Object(metric));
            }
        }
    }

    // Get API token from environment
    let api_token = match std::env::var("BENCHER_API_TOKEN") {
        Ok(token) if !token.trim().is_empty() => token.trim().to_string(),
        _ => {
            eprintln!("\nBENCHER_API_TOKEN not set, skipping bencher.dev upload");
            return;
        }
    };

    if metrics.is_empty() {
        eprintln!("No metrics to send to bencher.dev");
        return;
    }

    // Build the JSON payload for bencher.dev
    // See: https://bencher.dev/docs/api/projects/reports/
    // The "results" field must be an array of JSON strings in BMF (Bencher Metric Format)
    let metrics_json_str = serde_json::to_string(&metrics).expect("Failed to serialize metrics");

    // Get current time for end_time, and 30 minutes ago for start_time (approximate test duration)
    let now = chrono::Utc::now();
    let start_time = now - chrono::Duration::minutes(30);

    let payload = serde_json::json!({
        "branch": std::env::var("BENCHER_BRANCH").unwrap_or_else(|_| "main".to_string()),
        "testbed": std::env::var("BENCHER_TESTBED").unwrap_or_else(|_| "localhost".to_string()),
        "start_time": start_time.to_rfc3339(),
        "end_time": now.to_rfc3339(),
        "results": [metrics_json_str]
    });

    eprintln!(
        "Sending to bencher.dev: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    // Send to bencher.dev API using reqwest
    let project = std::env::var("BENCHER_PROJECT").unwrap_or_else(|_| "pg-doorman".to_string());
    let url = format!("https://api.bencher.dev/v0/projects/{}/reports", project);

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_token))
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            if status.is_success() {
                // Check if response contains an error message from API
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(message) = json.get("message") {
                        // API returned an error in JSON format
                        eprintln!(
                            "\x1b[1;31m✗ Failed to send results to bencher.dev: {}\x1b[0m",
                            message
                        );
                        eprintln!("Response: {}", body);
                    } else if json.get("uuid").is_some() || json.get("report").is_some() {
                        // Success - response contains expected fields
                        eprintln!("\x1b[1;32m✓ Successfully sent results to bencher.dev\x1b[0m");
                        eprintln!("Response: {}", body);
                    } else {
                        // Unknown response format
                        eprintln!("\x1b[1;33m⚠ Unexpected response from bencher.dev\x1b[0m");
                        eprintln!("Response: {}", body);
                    }
                } else {
                    // Could not parse response as JSON
                    eprintln!("\x1b[1;33m⚠ Could not parse bencher.dev response as JSON\x1b[0m");
                    eprintln!("Response: {}", body);
                }
            } else {
                eprintln!(
                    "\x1b[1;31m✗ Failed to send results to bencher.dev (HTTP {})\x1b[0m\nResponse: {}",
                    status, body
                );
            }
        }
        Err(e) => {
            eprintln!(
                "\x1b[1;31m✗ Failed to send request to bencher.dev: {}\x1b[0m",
                e
            );
        }
    }
}

/// Send benchmark results for a specific test step to bencher.dev
/// This should be called after each group of 4 tests (postgresql, pg_doorman, odyssey, pgbouncer)
/// Example: When I send benchmark results for "simple_c1" to bencher
#[when(expr = "I send benchmark results for {string} to bencher")]
pub async fn send_step_results_to_bencher(world: &mut DoormanWorld, test_suffix: String) {
    let doorman_key = format!("pg_doorman_{}", test_suffix);
    let pgbouncer_key = format!("pgbouncer_{}", test_suffix);
    let odyssey_key = format!("odyssey_{}", test_suffix);

    let doorman_tps = world
        .bench_results
        .get(&doorman_key)
        .copied()
        .unwrap_or(0.0);
    let pgbouncer_tps = world
        .bench_results
        .get(&pgbouncer_key)
        .copied()
        .unwrap_or(0.0);
    let odyssey_tps = world
        .bench_results
        .get(&odyssey_key)
        .copied()
        .unwrap_or(0.0);

    eprintln!("\n=== Sending benchmark results for {} ===", test_suffix);

    let mut metrics = serde_json::Map::new();

    // Compare pg_doorman vs pgbouncer
    if pgbouncer_tps > 0.0 && doorman_tps > 0.0 {
        let ratio = doorman_tps / pgbouncer_tps;
        let metric_name = format!("pg_doorman_vs_pgbouncer_{}", test_suffix);

        eprintln!(
            "\x1b[1;36m{}: {:.2} / {:.2} = {:.4}\x1b[0m",
            metric_name, doorman_tps, pgbouncer_tps, ratio
        );

        let mut metric = serde_json::Map::new();
        let mut throughput = serde_json::Map::new();
        throughput.insert("value".to_string(), serde_json::json!(ratio));
        metric.insert(
            "throughput".to_string(),
            serde_json::Value::Object(throughput),
        );
        metrics.insert(metric_name, serde_json::Value::Object(metric));
    } else if doorman_tps > 0.0 && pgbouncer_tps == 0.0 {
        let metric_name = format!("pg_doorman_vs_pgbouncer_{}", test_suffix);
        eprintln!(
            "\x1b[1;32m{}: pg_doorman={:.2}, pgbouncer=0 (pg_doorman wins)\x1b[0m",
            metric_name, doorman_tps
        );
        let mut metric = serde_json::Map::new();
        let mut throughput = serde_json::Map::new();
        throughput.insert("value".to_string(), serde_json::json!(10.0));
        metric.insert(
            "throughput".to_string(),
            serde_json::Value::Object(throughput),
        );
        metrics.insert(metric_name, serde_json::Value::Object(metric));
    }

    // Compare pg_doorman vs odyssey
    if odyssey_tps > 0.0 && doorman_tps > 0.0 {
        let ratio = doorman_tps / odyssey_tps;
        let metric_name = format!("pg_doorman_vs_odyssey_{}", test_suffix);

        eprintln!(
            "\x1b[1;35m{}: {:.2} / {:.2} = {:.4}\x1b[0m",
            metric_name, doorman_tps, odyssey_tps, ratio
        );

        let mut metric = serde_json::Map::new();
        let mut throughput = serde_json::Map::new();
        throughput.insert("value".to_string(), serde_json::json!(ratio));
        metric.insert(
            "throughput".to_string(),
            serde_json::Value::Object(throughput),
        );
        metrics.insert(metric_name, serde_json::Value::Object(metric));
    } else if doorman_tps > 0.0 && odyssey_tps == 0.0 {
        let metric_name = format!("pg_doorman_vs_odyssey_{}", test_suffix);
        eprintln!(
            "\x1b[1;32m{}: pg_doorman={:.2}, odyssey=0 (pg_doorman wins)\x1b[0m",
            metric_name, doorman_tps
        );
        let mut metric = serde_json::Map::new();
        let mut throughput = serde_json::Map::new();
        throughput.insert("value".to_string(), serde_json::json!(10.0));
        metric.insert(
            "throughput".to_string(),
            serde_json::Value::Object(throughput),
        );
        metrics.insert(metric_name, serde_json::Value::Object(metric));
    }

    // Get API token from environment
    let api_token = match std::env::var("BENCHER_API_TOKEN") {
        Ok(token) if !token.trim().is_empty() => token.trim().to_string(),
        _ => {
            eprintln!(
                "BENCHER_API_TOKEN not set, skipping bencher.dev upload for {}",
                test_suffix
            );
            return;
        }
    };

    if metrics.is_empty() {
        eprintln!("No metrics to send for {}", test_suffix);
        return;
    }

    // Build the JSON payload for bencher.dev
    // API requires start_time and end_time fields, and endpoint is /reports not /runs
    let metrics_json_str = serde_json::to_string(&metrics).expect("Failed to serialize metrics");

    // Get current time for end_time, and 30 minutes ago for start_time (approximate test duration)
    let now = chrono::Utc::now();
    let start_time = now - chrono::Duration::minutes(30);

    let payload = serde_json::json!({
        "branch": std::env::var("BENCHER_BRANCH").unwrap_or_else(|_| "main".to_string()),
        "testbed": std::env::var("BENCHER_TESTBED").unwrap_or_else(|_| "localhost".to_string()),
        "start_time": start_time.to_rfc3339(),
        "end_time": now.to_rfc3339(),
        "results": [metrics_json_str]
    });

    eprintln!(
        "Sending to bencher.dev: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    // Send to bencher.dev API using reqwest
    let project = std::env::var("BENCHER_PROJECT").unwrap_or_else(|_| "pg-doorman".to_string());
    let url = format!("https://api.bencher.dev/v0/projects/{}/reports", project);

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_token))
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            if status.is_success() {
                // Check if response contains an error message from API
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(message) = json.get("message") {
                        // API returned an error in JSON format
                        eprintln!(
                            "\x1b[1;31m✗ Failed to send {} results to bencher.dev: {}\x1b[0m",
                            test_suffix, message
                        );
                        eprintln!("Response: {}", body);
                    } else if json.get("uuid").is_some() || json.get("report").is_some() {
                        // Success - response contains expected fields
                        eprintln!(
                            "\x1b[1;32m✓ Successfully sent {} results to bencher.dev\x1b[0m",
                            test_suffix
                        );
                        eprintln!("Response: {}", body);
                    } else {
                        // Unknown response format
                        eprintln!(
                            "\x1b[1;33m⚠ Unexpected response from bencher.dev for {}\x1b[0m",
                            test_suffix
                        );
                        eprintln!("Response: {}", body);
                    }
                } else {
                    // Could not parse response as JSON
                    eprintln!(
                        "\x1b[1;33m⚠ Could not parse bencher.dev response as JSON for {}\x1b[0m",
                        test_suffix
                    );
                    eprintln!("Response: {}", body);
                }
            } else {
                eprintln!(
                    "\x1b[1;31m✗ Failed to send {} results to bencher.dev (HTTP {})\x1b[0m\nResponse: {}",
                    test_suffix, status, body
                );
            }
        }
        Err(e) => {
            eprintln!(
                "\x1b[1;31m✗ Failed to send request to bencher.dev for {}: {}\x1b[0m",
                test_suffix, e
            );
        }
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
                if let Some(baseline) = baseline_tps {
                    if baseline > 0.0 {
                        let normalized = tps / baseline;
                        let pooler_name = target
                            .strip_suffix(&format!("_{}", client_count))
                            .unwrap_or(target);
                        eprintln!(
                            "  {}: {:.2} tps (normalized: {:.4}x)",
                            pooler_name, tps, normalized
                        );
                    }
                } else {
                    eprintln!("  {}: {:.2} tps", target, tps);
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
        env_info.push("- **pgbench jobs**: variable (c1: 1, c40: 4, c120: 4, c500: 4, c10k: 4)".to_string());
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
    let markdown_content = format!(
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

### Legend

- **+N%**: pg_doorman is N% faster than competitor (e.g., +10% means pg_doorman is 10% faster)
- **-N%**: pg_doorman is N% slower than competitor (e.g., -10% means pg_doorman is 10% slower)
- **≈0%**: Equal performance (difference less than 3%)
- **∞**: Competitor failed (0 TPS), pg_doorman wins
- **N/A**: Test not supported by this pooler
- **-**: Test not executed
"#,
        now.format("%Y-%m-%d %H:%M UTC"),
        env_info.join("\n"),
        duration_info,
    );

    // Combine sections
    let full_markdown = format!(
        "{}\n---\n\n## Simple Protocol\n\n{}\n\n---\n\n## Extended Protocol\n\n{}\n\n---\n\n## Prepared Protocol\n\n{}\n\n---\n\n### Notes\n\n- Odyssey has poor support for extended query protocol in transaction pooling mode, resulting in significantly lower performance compared to pg_doorman and pgbouncer\n- **Important**: The values shown are **relative performance ratios**, not absolute TPS numbers. While absolute TPS values may vary depending on hardware and system load, the relative ratios between poolers should remain consistent when tests are run sequentially in a short timeframe (30 seconds each). This allows for fair comparison across different connection poolers under identical conditions\n",
        markdown_content,
        simple_table.join("\n"),
        extended_table.join("\n"),
        prepared_table.join("\n")
    );

    // Write to file
    let file_path = "documentation/docs/benchmarks.md";
    match std::fs::write(file_path, &full_markdown) {
        Ok(_) => {
            eprintln!(
                "\x1b[1;32m✓ Benchmark table written to {}\x1b[0m",
                file_path
            );
            eprintln!("\n{}", markdown_content);
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
mod tests {
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
}
