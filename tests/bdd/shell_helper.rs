use crate::world::{DoormanWorld, TestCommandResult};
use cucumber::{gherkin::Step, then, when};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Default timeout for shell commands (10 minutes)
const COMMAND_TIMEOUT_SECS: u64 = 600;

/// Helper function to run a shell command and capture the result with timeout
fn run_command(command: &str, working_dir: Option<&str>) -> TestCommandResult {
    run_command_with_timeout(command, working_dir, Duration::from_secs(COMMAND_TIMEOUT_SECS))
}

/// Helper function to run a shell command with a specific timeout
fn run_command_with_timeout(command: &str, working_dir: Option<&str>, timeout: Duration) -> TestCommandResult {
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

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    // Set environment variables for pg_doorman connection
    if let Ok(doorman_port) = std::env::var("DOORMAN_PORT") {
        cmd.env("DOORMAN_PORT", doorman_port);
    }
    if let Ok(pg_port) = std::env::var("PG_PORT") {
        cmd.env("PG_PORT", pg_port);
    }

    match cmd.spawn() {
        Ok(mut child) => {
            // Wait for the command with timeout
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Process finished, capture output
                        let stdout = child.stdout.take()
                            .map(|mut s| {
                                use std::io::Read;
                                let mut buf = String::new();
                                let _ = s.read_to_string(&mut buf);
                                buf
                            })
                            .unwrap_or_default();
                        let stderr = child.stderr.take()
                            .map(|mut s| {
                                use std::io::Read;
                                let mut buf = String::new();
                                let _ = s.read_to_string(&mut buf);
                                buf
                            })
                            .unwrap_or_default();
                        
                        return TestCommandResult {
                            exit_code: status.code(),
                            stdout,
                            stderr,
                            success: status.success(),
                        };
                    }
                    Ok(None) => {
                        // Process still running, check timeout
                        if start.elapsed() > timeout {
                            // Timeout reached, kill the process
                            let _ = child.kill();
                            let _ = child.wait();
                            
                            return TestCommandResult {
                                exit_code: None,
                                stdout: String::new(),
                                stderr: format!(
                                    "Command timed out after {} seconds and was killed",
                                    timeout.as_secs()
                                ),
                                success: false,
                            };
                        }
                        // Sleep a bit before checking again
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => {
                        return TestCommandResult {
                            exit_code: None,
                            stdout: String::new(),
                            stderr: format!("Error waiting for command: {}", e),
                            success: false,
                        };
                    }
                }
            }
        }
        Err(e) => TestCommandResult {
            exit_code: None,
            stdout: String::new(),
            stderr: format!("Failed to execute command: {}", e),
            success: false,
        },
    }
}

/// Helper function to replace placeholders in commands
fn replace_placeholders(world: &DoormanWorld, command: &str) -> String {
    let mut result = command.to_string();

    // Replace ${DOORMAN_PORT} with actual doorman port
    if let Some(port) = world.doorman_port {
        result = result.replace("${DOORMAN_PORT}", &port.to_string());
    }

    // Replace ${PG_PORT} with actual postgres port
    if let Some(port) = world.pg_port {
        result = result.replace("${PG_PORT}", &port.to_string());
    }

    result
}

/// Run a shell command with inline multi-line support (docstring)
#[when("I run shell command:")]
pub async fn run_shell_command_multiline(world: &mut DoormanWorld, step: &Step) {
    let command = step
        .docstring
        .as_ref()
        .expect("Shell command not found in docstring")
        .to_string();

    let command = replace_placeholders(world, &command);

    let result = run_command(&command, None);
    world.last_test_result = Some(result);
}

/// Run a shell command with a single-line string argument
#[when(expr = "I run shell command {string}")]
pub async fn run_shell_command_string(world: &mut DoormanWorld, command: String) {
    let command = replace_placeholders(world, &command);

    let result = run_command(&command, None);
    world.last_test_result = Some(result);
}

/// Helper function to capture pg_doorman logs from stdout and stderr
fn capture_doorman_logs(world: &mut DoormanWorld) -> String {
    if let Some(ref mut child) = world.doorman_process {
        use std::io::Read;
        let mut result = String::new();
        
        // Capture stdout
        if let Some(ref mut stdout) = child.stdout.take() {
            let mut stdout_logs = String::new();
            let _ = stdout.read_to_string(&mut stdout_logs);
            if !stdout_logs.is_empty() {
                result.push_str(&format!("\n=== pg_doorman stdout ===\n{}\n", stdout_logs));
            }
        }
        
        // Capture stderr
        if let Some(ref mut stderr) = child.stderr.take() {
            let mut stderr_logs = String::new();
            let _ = stderr.read_to_string(&mut stderr_logs);
            if !stderr_logs.is_empty() {
                result.push_str(&format!("\n=== pg_doorman stderr ===\n{}\n", stderr_logs));
            }
        }
        
        if !result.is_empty() {
            return result;
        }
    }
    String::new()
}

/// Assert that the last command succeeded
#[then("the command should succeed")]
pub async fn command_should_succeed(world: &mut DoormanWorld) {
    let result = world
        .last_test_result
        .clone()
        .expect("No command has been run");

    if !result.success {
        let doorman_logs = capture_doorman_logs(world);
        
        // IMPORTANT: Stop pg_doorman BEFORE panic to prevent hanging
        // The after hook may not be called properly when panic occurs
        if let Some(ref mut child) = world.doorman_process {
            crate::doorman_helper::stop_doorman(child);
        }
        world.doorman_process = None;
        
        panic!(
            "Command failed with exit code {:?}\nstdout:\n{}\nstderr:\n{}{}",
            result.exit_code, result.stdout, result.stderr, doorman_logs
        );
    }
}

/// Assert that the last command failed
#[then("the command should fail")]
pub async fn command_should_fail(world: &mut DoormanWorld) {
    let result = world
        .last_test_result
        .clone()
        .expect("No command has been run");

    if result.success {
        // IMPORTANT: Stop pg_doorman BEFORE panic to prevent hanging
        if let Some(ref mut child) = world.doorman_process {
            crate::doorman_helper::stop_doorman(child);
        }
        world.doorman_process = None;
        
        panic!(
            "Command succeeded but was expected to fail\nstdout:\n{}\nstderr:\n{}",
            result.stdout, result.stderr
        );
    }
}

/// Assert that the command output contains the specified text
#[then(expr = "the command output should contain {string}")]
pub async fn command_output_should_contain(world: &mut DoormanWorld, text: String) {
    let result = world
        .last_test_result
        .clone()
        .expect("No command has been run");

    let combined_output = format!("{}{}", result.stdout, result.stderr);

    if !combined_output.contains(&text) {
        // IMPORTANT: Stop pg_doorman BEFORE panic to prevent hanging
        if let Some(ref mut child) = world.doorman_process {
            crate::doorman_helper::stop_doorman(child);
        }
        world.doorman_process = None;
        
        panic!(
            "Command output does not contain '{}'\nstdout:\n{}\nstderr:\n{}",
            text, result.stdout, result.stderr
        );
    }
}

/// Assert that the command output does not contain the specified text
#[then(expr = "the command output should not contain {string}")]
pub async fn command_output_should_not_contain(world: &mut DoormanWorld, text: String) {
    let result = world
        .last_test_result
        .clone()
        .expect("No command has been run");

    let combined_output = format!("{}{}", result.stdout, result.stderr);

    if combined_output.contains(&text) {
        // IMPORTANT: Stop pg_doorman BEFORE panic to prevent hanging
        if let Some(ref mut child) = world.doorman_process {
            crate::doorman_helper::stop_doorman(child);
        }
        world.doorman_process = None;
        
        panic!(
            "Command output contains '{}' but should not\nstdout:\n{}\nstderr:\n{}",
            text, result.stdout, result.stderr
        );
    }
}
