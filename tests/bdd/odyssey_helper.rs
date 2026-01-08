use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given};
use portpicker::pick_unused_port;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::sleep;

#[given("odyssey started with config:")]
pub async fn start_odyssey_with_config(world: &mut DoormanWorld, step: &Step) {
    if let Some(ref mut child) = world.odyssey_process {
        stop_odyssey(child);
    }
    world.odyssey_process = None;

    let config_content = step
        .docstring
        .as_ref()
        .expect("config_content not found")
        .to_string();
    
    let odyssey_port = pick_unused_port().expect("No free ports for odyssey");

    let mut config_content = config_content.replace("${ODYSSEY_PORT}", &odyssey_port.to_string());

    if let Some(pg_port) = world.pg_port {
        config_content = config_content.replace("${PG_PORT}", &pg_port.to_string());
    }

    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    config_file
        .write_all(config_content.as_bytes())
        .expect("Failed to write config content");
    config_file.flush().expect("Failed to flush config file");
    let config_path = config_file.path().to_path_buf();
    
    world.odyssey_config_file = Some(config_file);

    let debug_mode = std::env::var("DEBUG").is_ok();
    let (stdout_cfg, stderr_cfg) = if debug_mode {
        (Stdio::inherit(), Stdio::inherit())
    } else {
        (Stdio::null(), Stdio::null())
    };

    let child = Command::new("odyssey")
        .arg(&config_path)
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .spawn()
        .expect("Failed to start odyssey");

    world.odyssey_process = Some(child);
    world.odyssey_port = Some(odyssey_port);

    wait_for_odyssey_ready(odyssey_port, world.odyssey_process.as_mut().unwrap()).await;
}

pub fn stop_odyssey(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

async fn wait_for_odyssey_ready(port: u16, child: &mut Child) {
    let mut success = false;
    for _ in 0..20 {
        match child.try_wait() {
            Ok(Some(status)) => {
                panic!("odyssey exited with status: {:?}", status);
            }
            Ok(None) => {
                // Check if port is open
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    success = true;
                    break;
                }
            }
            Err(e) => panic!("Error checking odyssey process: {}", e),
        }
        sleep(Duration::from_millis(500)).await;
    }

    if !success {
        panic!("odyssey failed to start on port {}", port);
    }
}
