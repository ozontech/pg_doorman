use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given};
use portpicker::pick_unused_port;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::sleep;

#[given("pgbouncer started with config:")]
pub async fn start_pgbouncer_with_config(world: &mut DoormanWorld, step: &Step) {
    if let Some(ref mut child) = world.pgbouncer_process {
        stop_pgbouncer(child);
    }
    world.pgbouncer_process = None;

    let config_content = step
        .docstring
        .as_ref()
        .expect("config_content not found")
        .to_string();
    
    let pgbouncer_port = pick_unused_port().expect("No free ports for pgbouncer");

    let mut config_content = config_content.replace("${PGBOUNCER_PORT}", &pgbouncer_port.to_string());

    if let Some(pg_port) = world.pg_port {
        config_content = config_content.replace("${PG_PORT}", &pg_port.to_string());
    }

    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    config_file
        .write_all(config_content.as_bytes())
        .expect("Failed to write config content");
    config_file.flush().expect("Failed to flush config file");
    let config_path = config_file.path().to_path_buf();
    
    world.pgbouncer_config_file = Some(config_file);

    let debug_mode = std::env::var("DEBUG").is_ok();
    let (stdout_cfg, stderr_cfg) = if debug_mode {
        (Stdio::inherit(), Stdio::inherit())
    } else {
        (Stdio::null(), Stdio::null())
    };

    let child = Command::new("pgbouncer")
        .arg(&config_path)
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .spawn()
        .expect("Failed to start pgbouncer");

    world.pgbouncer_process = Some(child);
    world.pgbouncer_port = Some(pgbouncer_port);

    wait_for_pgbouncer_ready(pgbouncer_port, world.pgbouncer_process.as_mut().unwrap()).await;
}

pub fn stop_pgbouncer(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

async fn wait_for_pgbouncer_ready(port: u16, child: &mut Child) {
    let mut success = false;
    for _ in 0..20 {
        match child.try_wait() {
            Ok(Some(status)) => {
                panic!("pgbouncer exited with status: {:?}", status);
            }
            Ok(None) => {
                // Check if port is open
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    success = true;
                    break;
                }
            }
            Err(e) => panic!("Error checking pgbouncer process: {}", e),
        }
        sleep(Duration::from_millis(500)).await;
    }

    if !success {
        panic!("pgbouncer failed to start on port {}", port);
    }
}
