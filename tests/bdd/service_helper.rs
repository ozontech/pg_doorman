use std::process::Child;
use std::time::Duration;
use tokio::time::sleep;

/// Stop a process gracefully (SIGTERM first, then SIGKILL)
/// Also closes stdout/stderr pipes to prevent hanging
pub fn stop_process(child: &mut Child) {
    // Send SIGTERM first for graceful shutdown
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }

    // Wait a bit for graceful shutdown
    std::thread::sleep(Duration::from_millis(100));

    // Force kill if still running
    let _ = child.kill();

    // Close stdout/stderr pipes BEFORE wait() to prevent hanging
    drop(child.stdout.take());
    drop(child.stderr.take());

    let _ = child.wait();
}

/// Stop a process immediately (just kill + wait)
pub fn stop_process_immediate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Wait for a service to be ready by checking TCP connection
/// Returns Ok(()) if service is ready, Err with message if timeout or process exited
pub async fn wait_for_tcp_ready(
    service_name: &str,
    port: u16,
    child: &mut Child,
    max_attempts: u32,
    interval_ms: u64,
) -> Result<(), String> {
    for _ in 0..max_attempts {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!("{service_name} exited with status: {status:?}"));
            }
            Ok(None) => {
                // Process still running, try to connect
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    return Ok(());
                }
            }
            Err(e) => {
                return Err(format!("Error checking {service_name} process: {e}"));
            }
        }
        sleep(Duration::from_millis(interval_ms)).await;
    }

    Err(format!(
        "{service_name} failed to start on port {port} (timeout after {max_attempts} attempts)"
    ))
}

/// Wait for a service to be ready, panic on failure
/// This is a convenience wrapper around wait_for_tcp_ready
pub async fn wait_for_service_ready(service_name: &str, port: u16, child: &mut Child) {
    // Default: 20 attempts with 500ms interval = 10 seconds timeout
    if let Err(e) = wait_for_tcp_ready(service_name, port, child, 20, 500).await {
        panic!("{}", e);
    }
}
