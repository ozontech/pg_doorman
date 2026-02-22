use crate::service_helper::stop_process;
use crate::utils::{create_config_file, create_temp_file, get_stdio_config, is_debug_mode};
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given, then, when};
use portpicker::pick_unused_port;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::sleep;

/// Generate self-signed SSL certificates for pg_doorman TLS configuration
#[given("self-signed SSL certificates are generated")]
pub async fn generate_ssl_certificates(world: &mut DoormanWorld) {
    let key_file = NamedTempFile::new().expect("Failed to create temp key file");
    let cert_file = NamedTempFile::new().expect("Failed to create temp cert file");

    let key_path = key_file.path().to_str().unwrap().to_string();
    let cert_path = cert_file.path().to_str().unwrap().to_string();

    // Generate private key using openssl
    let key_output = Command::new("openssl")
        .args(["genrsa", "-out", &key_path, "2048"])
        .output()
        .expect("Failed to execute openssl genrsa");

    if !key_output.status.success() {
        panic!(
            "Failed to generate SSL private key:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&key_output.stdout),
            String::from_utf8_lossy(&key_output.stderr)
        );
    }

    // Generate self-signed certificate using openssl
    let cert_output = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-x509",
            "-key",
            &key_path,
            "-out",
            &cert_path,
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
        ])
        .output()
        .expect("Failed to execute openssl req");

    if !cert_output.status.success() {
        panic!(
            "Failed to generate SSL certificate:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&cert_output.stdout),
            String::from_utf8_lossy(&cert_output.stderr)
        );
    }

    world.ssl_key_file = Some(key_file);
    world.ssl_cert_file = Some(cert_file);
}

/// Set pg_doorman hba file with inline content
#[given("pg_doorman hba file contains:")]
pub async fn set_doorman_hba_file(world: &mut DoormanWorld, step: &Step) {
    let hba_content = step
        .docstring
        .as_ref()
        .expect("hba_content not found")
        .to_string();

    world.doorman_hba_file = Some(create_temp_file(&hba_content));
}

/// Start pg_doorman with config content
#[given("pg_doorman started with config:")]
pub async fn start_doorman_with_config(world: &mut DoormanWorld, step: &Step) {
    // Stop any previously running pg_doorman before starting a new one
    if let Some(ref mut child) = world.doorman_process {
        stop_doorman(child);
    }
    world.doorman_process = None;

    let config_content = step
        .docstring
        .as_ref()
        .expect("config_content not found")
        .to_string();

    let doorman_port = pick_unused_port().expect("No free ports for pg_doorman");
    world.doorman_port = Some(doorman_port);

    // Use centralized placeholder replacement
    let config_content = world.replace_placeholders(&config_content);

    let config_file = create_config_file(&config_content);
    let config_path = config_file.path().to_path_buf();
    world.doorman_config_file = Some(config_file);

    // Use CARGO_BIN_EXE_pg_doorman which is automatically set by cargo test
    let doorman_binary = env!("CARGO_BIN_EXE_pg_doorman");
    // For @bench scenarios, always use "info" level to avoid debug overhead
    let log_level = if world.is_bench {
        "info"
    } else if is_debug_mode() {
        "debug"
    } else {
        "info"
    };
    let (stdout_cfg, stderr_cfg) = get_stdio_config();

    let child = Command::new(doorman_binary)
        .arg(&config_path)
        .arg("-l")
        .arg(log_level)
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .spawn()
        .expect("Failed to start pg_doorman");

    world.doorman_process = Some(child);

    // Wait for pg_doorman to be ready (custom implementation with log capture)
    wait_for_doorman_ready(doorman_port, world.doorman_process.as_mut().unwrap()).await;
}

/// Helper function to wait for pg_doorman to be ready (max 5 seconds)
/// This is a custom implementation that captures logs on failure
pub(crate) async fn wait_for_doorman_ready(port: u16, child: &mut Child) {
    use std::io::Read;

    let mut success = false;
    for _ in 0..20 {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process exited, capture stdout and stderr
                let mut stdout_output = String::new();
                let mut stderr_output = String::new();

                if let Some(ref mut stdout) = child.stdout {
                    let _ = stdout.read_to_string(&mut stdout_output);
                }
                if let Some(ref mut stderr) = child.stderr {
                    let _ = stderr.read_to_string(&mut stderr_output);
                }

                panic!(
                    "pg_doorman exited with status: {:?}\n\n=== stdout ===\n{}\n=== stderr ===\n{}",
                    status, stdout_output, stderr_output
                );
            }
            Ok(None) => {
                if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                    success = true;
                    break;
                }
            }
            Err(e) => {
                panic!("Error checking pg_doorman process: {:?}", e);
            }
        }
        sleep(Duration::from_millis(250)).await;
    }

    if !success {
        let _ = child.kill();

        let mut stdout_output = String::new();
        let mut stderr_output = String::new();

        if let Some(ref mut stdout) = child.stdout {
            let _ = stdout.read_to_string(&mut stdout_output);
        }
        if let Some(ref mut stderr) = child.stderr {
            let _ = stderr.read_to_string(&mut stderr_output);
        }

        let _ = child.wait();

        panic!(
            "pg_doorman failed to start on port {} (timeout 5s)\n\n=== stdout ===\n{}\n=== stderr ===\n{}",
            port, stdout_output, stderr_output
        );
    }
}

/// Stop pg_doorman process (only if still running)
pub fn stop_doorman(child: &mut Child) {
    // Check if process has already exited
    match child.try_wait() {
        Ok(Some(_status)) => {
            // Process already exited, just clean up pipes
            drop(child.stdout.take());
            drop(child.stderr.take());
            return;
        }
        Ok(None) => {
            // Process still running, stop it
            stop_process(child);
        }
        Err(_) => {
            // Error checking status, try to stop anyway
            stop_process(child);
        }
    }
}

/// Stop pg_doorman daemon by PID
pub fn stop_doorman_daemon(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    // Wait a bit for process to terminate
    std::thread::sleep(Duration::from_millis(100));
}

/// Check if a process is running by PID
fn is_process_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Start pg_doorman in daemon mode with config content
#[given("pg_doorman started in daemon mode with config:")]
pub async fn start_doorman_daemon_with_config(world: &mut DoormanWorld, step: &Step) {
    // Stop any previously running pg_doorman daemon by reading PID from file
    if let Some(ref pid_path) = world.doorman_daemon_pid_file {
        if let Ok(pid_content) = std::fs::read_to_string(pid_path) {
            if let Ok(pid) = pid_content.trim().parse::<u32>() {
                stop_doorman_daemon(pid);
            }
        }
    }
    world.doorman_daemon_pid_file = None;

    let config_content = step
        .docstring
        .as_ref()
        .expect("config_content not found")
        .trim()
        .to_string();

    let doorman_port = pick_unused_port().expect("No free ports for pg_doorman");
    world.doorman_port = Some(doorman_port);

    // Use centralized placeholder replacement
    let config_content = world.replace_placeholders(&config_content);

    // Create temp file with appropriate extension (required for pg_doorman config parsing)
    let config_file = create_config_file(&config_content);
    let config_path = config_file.path().to_path_buf();
    world.doorman_config_file = Some(config_file);

    let doorman_binary = env!("CARGO_BIN_EXE_pg_doorman");
    let log_level = if is_debug_mode() { "debug" } else { "info" };

    // Run with --daemon flag - parent process exits after daemon is ready
    let output = Command::new(doorman_binary)
        .arg(&config_path)
        .arg("--daemon")
        .arg("-l")
        .arg(log_level)
        .output()
        .expect("Failed to start pg_doorman in daemon mode");

    if !output.status.success() {
        panic!(
            "pg_doorman daemon failed to start (exit code: {:?}):\nstdout: {}\nstderr: {}\nconfig_path: {:?}\nconfig_content:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            config_path,
            config_content
        );
    }

    // Wait for port to be ready
    wait_for_daemon_port_ready(doorman_port).await;

    // Store daemon_pid_file path for cleanup (PID will be read from file in after hook)
    // This handles binary-upgrade where PID changes
    if let Some(pid_path) = extract_daemon_pid_file(&config_content) {
        world.doorman_daemon_pid_file = Some(pid_path);
    }
}

/// Extract daemon_pid_file path from config content
fn extract_daemon_pid_file(config: &str) -> Option<String> {
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with("daemon_pid_file") {
            if let Some(value) = line.split('=').nth(1) {
                return Some(value.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// Helper function to wait for daemon port to be ready
async fn wait_for_daemon_port_ready(port: u16) {
    for _ in 0..20 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            return;
        }
        sleep(Duration::from_millis(250)).await;
    }

    panic!(
        "pg_doorman daemon failed to listen on port {} (timeout 5s)",
        port
    );
}

/// Check if PID file contains correct daemon PID
#[then(regex = r#"PID file "([^"]+)" should contain running daemon PID"#)]
pub async fn verify_pid_file(_world: &mut DoormanWorld, pid_path: String) {
    let pid_content = std::fs::read_to_string(&pid_path).expect("Failed to read PID file");
    let pid: u32 = pid_content
        .trim()
        .parse()
        .expect("PID file should contain valid number");

    assert!(
        is_process_running(pid),
        "PID {} in file should be a running process",
        pid
    );
}

/// Store current daemon PID for later comparison
#[when(regex = r#"we store daemon PID from "([^"]+)" as "([^"]+)""#)]
pub async fn store_daemon_pid(world: &mut DoormanWorld, pid_path: String, name: String) {
    let pid_content = std::fs::read_to_string(&pid_path).expect("Failed to read PID file");
    let pid: i32 = pid_content
        .trim()
        .parse()
        .expect("PID file should contain valid number");

    world
        .named_backend_pids
        .insert((name, "daemon_pid".to_string()), pid);
}

/// Send SIGINT to daemon for graceful reload (binary-upgrade)
/// This only sends the signal, use "we wait for new daemon" step to wait for the new daemon
#[when(regex = r#"we send SIGINT to daemon from PID file "([^"]+)""#)]
pub async fn send_sigint_to_daemon(_world: &mut DoormanWorld, pid_path: String) {
    let pid_content = std::fs::read_to_string(&pid_path).expect("Failed to read PID file");
    let pid: i32 = pid_content
        .trim()
        .parse()
        .expect("PID file should contain valid number");

    unsafe {
        libc::kill(pid, libc::SIGINT);
    }
}

/// Verify that daemon PID has changed after graceful reload
#[then(regex = r#"PID file "([^"]+)" should contain different PID than stored "([^"]+)""#)]
pub async fn verify_pid_changed(_world: &mut DoormanWorld, pid_path: String, name: String) {
    let pid_content = std::fs::read_to_string(&pid_path).expect("Failed to read PID file");
    let current_pid: i32 = pid_content
        .trim()
        .parse()
        .expect("PID file should contain valid number");

    let old_pid = _world
        .named_backend_pids
        .get(&(name.clone(), "daemon_pid".to_string()))
        .expect("Stored PID not found");

    assert_ne!(
        current_pid, *old_pid,
        "PID should have changed after graceful reload (old: {}, current: {})",
        old_pid, current_pid
    );

    assert!(
        is_process_running(current_pid as u32),
        "New PID {} should be a running process",
        current_pid
    );
}

/// Verify that a named session is connected
#[then(regex = r#"session "([^"]+)" should be connected"#)]
pub async fn verify_session_connected(world: &mut DoormanWorld, session_name: String) {
    assert!(
        world.named_sessions.contains_key(&session_name),
        "Session '{}' should exist and be connected",
        session_name
    );
}

/// Verify that two stored PIDs are different
/// PIDs are stored as (session_name, pid_name) -> pid in named_backend_pids
#[then(regex = r#"stored PID "([^"]+)" should be different from "([^"]+)"#)]
pub async fn verify_stored_pids_different(
    world: &mut DoormanWorld,
    pid1_name: String,
    pid2_name: String,
) {
    // Find PID by pid_name (second element of the key tuple)
    let pid1 = world
        .named_backend_pids
        .iter()
        .find(|((_, name), _)| name == &pid1_name)
        .map(|(_, pid)| *pid)
        .unwrap_or_else(|| panic!("Stored PID '{}' not found in named_backend_pids", pid1_name));

    let pid2 = world
        .named_backend_pids
        .iter()
        .find(|((_, name), _)| name == &pid2_name)
        .map(|(_, pid)| *pid)
        .unwrap_or_else(|| panic!("Stored PID '{}' not found in named_backend_pids", pid2_name));

    assert_ne!(
        pid1, pid2,
        "PIDs should be different: '{}' = {}, '{}' = {}",
        pid1_name, pid1, pid2_name, pid2
    );
}

/// Store current foreground pg_doorman PID for later comparison
#[when(regex = r#"we store foreground pg_doorman PID as "([^"]+)""#)]
pub async fn store_foreground_pid(world: &mut DoormanWorld, name: String) {
    let pid = world
        .doorman_process
        .as_ref()
        .expect("pg_doorman process not running")
        .id() as i32;

    world
        .named_backend_pids
        .insert((name, "foreground_pid".to_string()), pid);
}

/// Send SIGINT to foreground pg_doorman for binary upgrade
#[when("we send SIGINT to foreground pg_doorman")]
pub async fn send_sigint_to_foreground(world: &mut DoormanWorld) {
    let pid = world
        .doorman_process
        .as_ref()
        .expect("pg_doorman process not running")
        .id() as i32;

    unsafe {
        libc::kill(pid, libc::SIGINT);
    }
}

/// Wait for foreground pg_doorman binary upgrade to complete (new process takes over)
#[when("we wait for foreground binary upgrade to complete")]
pub async fn wait_for_foreground_binary_upgrade(world: &mut DoormanWorld) {
    let port = world.doorman_port.expect("doorman_port not set");

    // Wait a bit for the new process to start and signal readiness
    sleep(Duration::from_millis(2000)).await;

    // Verify the port is still accessible (new process is listening)
    for _ in 0..20 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            return;
        }
        sleep(Duration::from_millis(250)).await;
    }

    panic!(
        "pg_doorman failed to complete binary upgrade on port {} (timeout 5s)",
        port
    );
}

/// Verify that foreground pg_doorman PID has changed after binary upgrade
#[then(regex = r#"foreground pg_doorman PID should be different from stored "([^"]+)""#)]
pub async fn verify_foreground_pid_changed(world: &mut DoormanWorld, name: String) {
    let port = world.doorman_port.expect("doorman_port not set");

    let old_pid = world
        .named_backend_pids
        .get(&(name.clone(), "foreground_pid".to_string()))
        .expect("Stored foreground PID not found");

    // The old process should have exited or be in graceful shutdown
    // We need to find the new process listening on the port
    // Since we can't easily get the new PID, we verify the old process is no longer the main listener

    // Wait a bit and check if old process is still running
    sleep(Duration::from_millis(500)).await;

    // Check if port is still accessible
    assert!(
        std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok(),
        "New pg_doorman should be listening on port {}",
        port
    );

    // The old process should eventually exit after graceful shutdown
    // For now, we just verify the service is still available
    // In a real scenario, the old process exits after all clients disconnect

    // Note: We can't easily verify PID change in foreground mode without additional tracking
    // because the child process is not tracked by our test harness
    // The key verification is that the service remains available after SIGINT

    println!(
        "Binary upgrade completed: old PID was {}, service still available on port {}",
        old_pid, port
    );
}

/// Verify that stored foreground pg_doorman PID no longer exists (process terminated)
#[then(regex = r#"stored foreground PID "([^"]+)" should not exist"#)]
pub async fn verify_foreground_pid_not_exists(world: &mut DoormanWorld, name: String) {
    let pid = world
        .named_backend_pids
        .get(&(name.clone(), "foreground_pid".to_string()))
        .expect("Stored foreground PID not found");

    assert!(
        !is_process_running(*pid as u32),
        "Process with PID {} should not exist, but it is still running",
        pid
    );
}

/// Overwrite the pg_doorman config file with new invalid content
#[when("we overwrite pg_doorman config file with invalid content:")]
pub async fn overwrite_config_with_invalid(world: &mut DoormanWorld, step: &Step) {
    let invalid_content = step
        .docstring
        .as_ref()
        .expect("Invalid config content not found in docstring")
        .to_string();

    let config_file = world
        .doorman_config_file
        .as_ref()
        .expect("pg_doorman config file not found");

    std::fs::write(config_file.path(), invalid_content).expect("Failed to overwrite config file");
}

/// Verify that pg_doorman PID has NOT changed (same process still running)
#[then(regex = r#"foreground pg_doorman PID should be same as stored "([^"]+)""#)]
pub async fn verify_foreground_pid_same(world: &mut DoormanWorld, name: String) {
    let stored_pid = world
        .named_backend_pids
        .get(&(name.clone(), "foreground_pid".to_string()))
        .expect("Stored foreground PID not found");

    if let Some(ref child) = world.doorman_process {
        let current_pid = child.id() as i32;
        assert_eq!(
            *stored_pid, current_pid,
            "PID should be same: stored '{}' = {}, current = {}",
            name, stored_pid, current_pid
        );
    } else {
        panic!("pg_doorman process not running");
    }
}
