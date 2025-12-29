use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given};
use portpicker::pick_unused_port;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::sleep;

/// Generate self-signed SSL certificates for pg_doorman TLS configuration
#[given("self-signed SSL certificates are generated")]
pub async fn generate_ssl_certificates(world: &mut DoormanWorld) {
    // Create temporary files for key and certificate
    let key_file = NamedTempFile::new().expect("Failed to create temp key file");
    let cert_file = NamedTempFile::new().expect("Failed to create temp cert file");
    
    let key_path = key_file.path().to_str().unwrap().to_string();
    let cert_path = cert_file.path().to_str().unwrap().to_string();
    
    // Generate private key using openssl
    let key_output = Command::new("openssl")
        .args([
            "genrsa",
            "-out", &key_path,
            "2048"
        ])
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
            "-key", &key_path,
            "-out", &cert_path,
            "-days", "1",
            "-subj", "/CN=localhost"
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
    
    // Create a temporary hba file
    let mut hba_file = NamedTempFile::new().expect("Failed to create temp hba file");
    hba_file
        .write_all(hba_content.as_bytes())
        .expect("Failed to write hba content");
    hba_file.flush().expect("Failed to flush hba file");
    world.doorman_hba_file = Some(hba_file);
}

/// Start pg_doorman with config content
#[given("pg_doorman started with config:")]
pub async fn start_doorman_with_config(world: &mut DoormanWorld, step: &Step) {
    // IMPORTANT: Stop any previously running pg_doorman before starting a new one
    // This prevents zombie processes and hanging tests
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
    
    // Replace placeholder for doorman port if present
    let config_content = config_content.replace("${DOORMAN_PORT}", &doorman_port.to_string());
    
    // Replace placeholder for postgres port if present
    let config_content = if let Some(pg_port) = world.pg_port {
        config_content.replace("${PG_PORT}", &pg_port.to_string())
    } else {
        config_content
    };
    
    // Replace placeholder for pg_hba file path (use temp file from "pg_doorman hba file contains:" step)
    let config_content = if let Some(ref hba_file) = world.doorman_hba_file {
        config_content.replace("${DOORMAN_HBA_FILE}", hba_file.path().to_str().unwrap())
    } else {
        config_content
    };
    
    // Replace placeholder for SSL private key file path (use temp file from "self-signed SSL certificates are generated" step)
    let config_content = if let Some(ref ssl_key_file) = world.ssl_key_file {
        config_content.replace("${DOORMAN_SSL_KEY}", ssl_key_file.path().to_str().unwrap())
    } else {
        config_content
    };
    
    // Replace placeholder for SSL certificate file path (use temp file from "self-signed SSL certificates are generated" step)
    let config_content = if let Some(ref ssl_cert_file) = world.ssl_cert_file {
        config_content.replace("${DOORMAN_SSL_CERT}", ssl_cert_file.path().to_str().unwrap())
    } else {
        config_content
    };
    
    // Create a temporary config file
    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    config_file
        .write_all(config_content.as_bytes())
        .expect("Failed to write config content");
    config_file.flush().expect("Failed to flush config file");
    let config_path = config_file.path().to_path_buf();
    world.doorman_config_file = Some(config_file);
    
    // Use CARGO_BIN_EXE_pg_doorman which is automatically set by cargo test
    let doorman_binary = env!("CARGO_BIN_EXE_pg_doorman");
    
    // Use null for stdout/stderr to prevent hanging on pipe reads
    // When tests fail, the pipes would block indefinitely waiting for EOF
    // Log files can be used for debugging if needed
    let child = Command::new(&doorman_binary)
        .arg(&config_path)
        .arg("-l")
        .arg("debug")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start pg_doorman");
    
    world.doorman_process = Some(child);
    world.doorman_port = Some(doorman_port);
    
    // Wait for pg_doorman to be ready
    wait_for_doorman_ready(doorman_port, world.doorman_process.as_mut().unwrap()).await;
}

/// Helper function to wait for pg_doorman to be ready (max 5 seconds)
async fn wait_for_doorman_ready(port: u16, child: &mut Child) {
    use std::io::Read;
    
    let mut success = false;
    for _ in 0..20 {
        // Check if process is still running
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
                    "pg_doorman exited with status: {:?}\n\n=== pg_doorman stdout ===\n{}\n=== pg_doorman stderr ===\n{}",
                    status, stdout_output, stderr_output
                );
            }
            Ok(None) => {
                // Process still running, try to connect
                if let Ok(_) = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)) {
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
        // Timeout reached, kill process and capture logs
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
            "pg_doorman failed to start on port {} (timeout 5s)\n\n=== pg_doorman stdout ===\n{}\n=== pg_doorman stderr ===\n{}",
            port, stdout_output, stderr_output
        );
    }
}

/// Stop pg_doorman process
pub fn stop_doorman(child: &mut Child) {
    // Send SIGTERM first for graceful shutdown
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    
    // Wait a bit for graceful shutdown
    std::thread::sleep(Duration::from_millis(100));
    
    // Force kill if still running
    let _ = child.kill();
    
    // IMPORTANT: Close stdout/stderr pipes BEFORE wait() to prevent hanging
    // If we don't close these, the parent process will block waiting for EOF
    // on the pipes even after the child is killed
    drop(child.stdout.take());
    drop(child.stderr.take());
    
    let _ = child.wait();
}

