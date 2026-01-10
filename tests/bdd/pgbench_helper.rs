use crate::utils::create_temp_file;
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given, then, when};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Default timeout for pgbench commands (10 minutes)
const PGBENCH_TIMEOUT_SECS: u64 = 600;

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
                eprintln!(
                    "\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m",
                    target, tps
                );
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
    let options = world.replace_placeholders(&options);

    // Build the full pgbench command
    let command = format!("pgbench {}", options);

    eprintln!("Running pgbench for {}: {}", target, command);

    match run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS)) {
        Ok(output) => {
            if let Some(tps) = parse_tps(&output) {
                // Print TPS with bright colors for visibility
                eprintln!(
                    "\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m",
                    target, tps
                );
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

/// Run pgbench with inline command and explicit environment variables
/// Example: When I run pgbench for "target" with "-h 127.0.0.1 -p 5432 ..." and env "PGSSLMODE=require"
#[when(expr = "I run pgbench for {string} with {string} and env {string}")]
pub async fn run_pgbench_for_target_with_env(
    world: &mut DoormanWorld,
    target: String,
    options: String,
    env_vars: String,
) {
    let options = world.replace_placeholders(&options);

    // Build the full pgbench command with env prefix
    let command = format!("{} pgbench {}", env_vars, options);

    eprintln!("Running pgbench for {}: {}", target, command);

    match run_pgbench(&command, &target, Duration::from_secs(PGBENCH_TIMEOUT_SECS)) {
        Ok(output) => {
            if let Some(tps) = parse_tps(&output) {
                // Print TPS with bright colors for visibility
                eprintln!(
                    "\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m",
                    target, tps
                );
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
                eprintln!(
                    "\x1b[1;32m✓ TPS for {}: {:.2}\x1b[0m",
                    target, tps
                );
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

/// Extract client count suffix from target name (e.g., "pg_doorman_c10" -> "c10", "pg_doorman_ssl_c10" -> "c10")
fn extract_client_suffix(target: &str) -> Option<&str> {
    // Look for patterns like _c1, _c10, _c50, _c100, _c200
    for suffix in &["_c1", "_c10", "_c50", "_c100", "_c200"] {
        if target.ends_with(suffix) {
            return Some(&suffix[1..]); // Return without leading underscore
        }
    }
    None
}

/// Get the baseline key for a given target (e.g., "pg_doorman_c10" -> "postgresql_c10")
fn get_baseline_key(target: &str) -> Option<String> {
    extract_client_suffix(target).map(|suffix| format!("postgresql_{}", suffix))
}

/// Send normalized benchmark results to bencher.dev
#[then("I send normalized benchmark results to bencher.dev")]
pub async fn send_to_bencher(world: &mut DoormanWorld) {
    // Get API token from environment
    let api_token = match std::env::var("BENCHER_API_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        _ => {
            eprintln!("BENCHER_API_TOKEN not set, skipping bencher.dev upload");
            // Print results locally instead
            eprintln!(
                "\n=== Benchmark Results (normalized to postgresql baseline per client count) ==="
            );
            for (target, tps) in &world.bench_results {
                if target.starts_with("postgresql_") {
                    eprintln!("{}: {:.2} tps (baseline)", target, tps);
                } else if let Some(baseline_key) = get_baseline_key(target) {
                    if let Some(baseline_tps) = world.bench_results.get(&baseline_key) {
                        if *baseline_tps > 0.0 {
                            let normalized = tps / baseline_tps;
                            eprintln!(
                                "{}: {:.2} tps (normalized: {:.4} vs {})",
                                target, tps, normalized, baseline_key
                            );
                        }
                    }
                }
            }
            return;
        }
    };

    // Prepare normalized results for each target (except postgresql baselines)
    let mut metrics = serde_json::Map::new();

    for (target, tps) in &world.bench_results {
        // Skip postgresql baselines
        if target.starts_with("postgresql_") {
            continue;
        }

        // Find the corresponding baseline for this client count
        let baseline_key = match get_baseline_key(target) {
            Some(key) => key,
            None => {
                eprintln!(
                    "Warning: Cannot determine baseline for {}, skipping",
                    target
                );
                continue;
            }
        };

        let baseline_tps = match world.bench_results.get(&baseline_key) {
            Some(tps) if *tps > 0.0 => *tps,
            _ => {
                eprintln!(
                    "Warning: Baseline {} not found or invalid for {}, skipping",
                    baseline_key, target
                );
                continue;
            }
        };

        let normalized = tps / baseline_tps;
        eprintln!(
            "Normalized result for {}: {:.2} tps / {:.2} tps ({}) = {:.4}",
            target, tps, baseline_tps, baseline_key, normalized
        );

        // Create metric entry for this target
        let mut metric = serde_json::Map::new();
        let mut throughput = serde_json::Map::new();
        throughput.insert("value".to_string(), serde_json::json!(normalized));
        metric.insert(
            "throughput".to_string(),
            serde_json::Value::Object(throughput),
        );
        metrics.insert(target.clone(), serde_json::Value::Object(metric));
    }

    // Build the JSON payload for bencher.dev
    // See: https://bencher.dev/docs/api/projects/runs/
    let payload = serde_json::json!({
        "branch": std::env::var("BENCHER_BRANCH").unwrap_or_else(|_| "main".to_string()),
        "testbed": std::env::var("BENCHER_TESTBED").unwrap_or_else(|_| "localhost".to_string()),
        "results": [metrics]
    });

    eprintln!(
        "Sending to bencher.dev: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    // Send to bencher.dev API
    let project = std::env::var("BENCHER_PROJECT").unwrap_or_else(|_| "pg-doorman".to_string());
    let url = format!("https://api.bencher.dev/v0/projects/{}/runs", project);

    // Use curl to send the request (simpler than adding reqwest dependency)
    let payload_str = serde_json::to_string(&payload).expect("Failed to serialize payload");

    let output = Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            &url,
            "-H",
            "Content-Type: application/json",
            "-H",
            &format!("Authorization: Bearer {}", api_token),
            "-d",
            &payload_str,
        ])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                eprintln!("Successfully sent results to bencher.dev");
                eprintln!("Response: {}", stdout);
            } else {
                eprintln!(
                    "Warning: Failed to send results to bencher.dev\nstdout: {}\nstderr: {}",
                    stdout, stderr
                );
            }
        }
        Err(e) => {
            eprintln!("Warning: Failed to execute curl: {}", e);
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
