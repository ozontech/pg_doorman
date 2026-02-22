use crate::port_allocator::allocate_port;
use crate::utils::{create_temp_file, get_stdio_config, is_debug_mode};
use crate::world::PatroniProxyWorld;
use cucumber::{gherkin::Step, given, when};
use std::fs;
use std::process::{Child, Command};
use std::time::Duration;
use tokio::time::sleep;

/// Start patroni_proxy with config content
#[given("patroni_proxy started with config:")]
pub async fn start_proxy_with_config(world: &mut PatroniProxyWorld, step: &Step) {
    // Stop any previously running patroni_proxy before starting a new one
    if let Some(ref mut child) = world.proxy_process {
        stop_proxy(child);
    }
    world.proxy_process = None;

    let config_content = step
        .docstring
        .as_ref()
        .expect("config_content not found")
        .to_string();

    // Use centralized placeholder replacement
    let config_content = world.replace_placeholders(&config_content);

    let config_file = create_temp_file(&config_content);
    let config_path = config_file.path().to_path_buf();
    world.proxy_config_file = Some(config_file);

    // Use CARGO_BIN_EXE_patroni_proxy which is automatically set by cargo test
    let proxy_binary = env!("CARGO_BIN_EXE_patroni_proxy");
    let log_level = if is_debug_mode() { "debug" } else { "info" };
    let (stdout_cfg, stderr_cfg) = get_stdio_config();

    let child = Command::new(proxy_binary)
        .arg(&config_path)
        .env("RUST_LOG", log_level)
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .spawn()
        .expect("Failed to start patroni_proxy");

    world.proxy_process = Some(child);

    // Wait for patroni_proxy to be ready
    if let Some(ref api_addr) = world.api_listen_address {
        let port = api_addr
            .split(':')
            .next_back()
            .and_then(|p| p.parse::<u16>().ok())
            .expect("Invalid API listen address");
        wait_for_proxy_ready(port, world.proxy_process.as_mut().unwrap()).await;
    } else {
        // If no API address, just wait a bit
        sleep(Duration::from_millis(500)).await;
    }
}

/// Helper function to wait for patroni_proxy to be ready (max 10 seconds)
async fn wait_for_proxy_ready(port: u16, child: &mut Child) {
    use std::io::Read;

    let mut success = false;
    for _ in 0..40 {
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
                    "patroni_proxy exited with status: {status:?}\n\n=== stdout ===\n{stdout_output}\n=== stderr ===\n{stderr_output}"
                );
            }
            Ok(None) => {
                if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
                    success = true;
                    break;
                }
            }
            Err(e) => {
                panic!("Error checking patroni_proxy process: {e:?}");
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
            "patroni_proxy failed to start on port {port} (timeout 10s)\n\n=== stdout ===\n{stdout_output}\n=== stderr ===\n{stderr_output}"
        );
    }
}

/// Stop patroni_proxy process
pub fn stop_proxy(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Define API listen address before starting
#[given("API listen address is allocated")]
pub async fn define_api_listen_address(world: &mut PatroniProxyWorld) {
    let port = allocate_port();
    let listen_addr = format!("127.0.0.1:{port}");
    world.api_listen_address = Some(listen_addr);
}

/// Allocate a single proxy port by name
#[given(regex = r"^proxy port '(.+)' is allocated$")]
pub async fn allocate_proxy_port(world: &mut PatroniProxyWorld, port_name: String) {
    let port = allocate_port();
    let listen_addr = format!("127.0.0.1:{port}");
    world.proxy_listen_addresses.insert(port_name, listen_addr);
}

/// Modify patroni_proxy configuration file
#[when("patroni_proxy config is modified:")]
pub async fn modify_proxy_config(world: &mut PatroniProxyWorld, step: &Step) {
    let config_content = step
        .docstring
        .as_ref()
        .expect("config_content not found")
        .to_string();

    // Use centralized placeholder replacement
    let config_content = world.replace_placeholders(&config_content);

    // Get the path of the existing config file
    let config_path = world
        .proxy_config_file
        .as_ref()
        .expect("No config file exists")
        .path()
        .to_path_buf();

    // Write new content to the config file
    fs::write(&config_path, config_content).expect("Failed to write modified config");
}

/// Send SIGHUP signal to patroni_proxy process
#[when("patroni_proxy receives SIGHUP signal")]
pub async fn send_sighup_to_proxy(world: &mut PatroniProxyWorld) {
    let child = world
        .proxy_process
        .as_ref()
        .expect("No patroni_proxy process running");

    let pid = child.id();

    // Send SIGHUP signal using nix crate
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    kill(Pid::from_raw(pid as i32), Signal::SIGHUP).expect("Failed to send SIGHUP");
}

/// Wait for specified number of seconds
#[when(regex = r"^wait for (\d+) seconds?$")]
pub async fn wait_for_seconds(_world: &mut PatroniProxyWorld, seconds: u64) {
    sleep(Duration::from_secs(seconds)).await;
}
