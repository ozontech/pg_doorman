use crate::service_helper::stop_process;
use crate::utils::{create_temp_file, get_stdio_config, is_debug_mode};
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given};
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

    let config_file = create_temp_file(&config_content);
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
async fn wait_for_doorman_ready(port: u16, child: &mut Child) {
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

/// Stop pg_doorman process
pub fn stop_doorman(child: &mut Child) {
    stop_process(child);
}
