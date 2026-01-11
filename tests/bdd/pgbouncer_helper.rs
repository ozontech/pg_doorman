use crate::service_helper::{stop_process_immediate, wait_for_service_ready};
use crate::utils::{create_temp_file, get_stdio_config, is_root, set_file_permissions};
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given};
use portpicker::pick_unused_port;
use std::process::Command;

/// Create pgbouncer userlist file with inline content
#[given("pgbouncer userlist file:")]
pub async fn create_pgbouncer_userlist(world: &mut DoormanWorld, step: &Step) {
    let userlist_content = step
        .docstring
        .as_ref()
        .expect("userlist_content not found")
        .to_string();

    let userlist_file = create_temp_file(&userlist_content);
    world.pgbouncer_userlist_file = Some(userlist_file);
}

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

    // Set port temporarily for placeholder replacement
    world.pgbouncer_port = Some(pgbouncer_port);
    let config_content = world.replace_placeholders(&config_content);

    let config_file = create_temp_file(&config_content);
    let config_path = config_file.path().to_path_buf();

    // If running as root, make config file readable by postgres user
    if is_root() {
        set_file_permissions(&config_path, 0o644);

        // Also ensure SSL files are readable if they exist
        if let Some(ref ssl_key_file) = world.ssl_key_file {
            set_file_permissions(ssl_key_file.path(), 0o644);
        }
        if let Some(ref ssl_cert_file) = world.ssl_cert_file {
            set_file_permissions(ssl_cert_file.path(), 0o644);
        }
        // Also ensure userlist file is readable
        if let Some(ref userlist_file) = world.pgbouncer_userlist_file {
            set_file_permissions(userlist_file.path(), 0o644);
        }
    }

    world.pgbouncer_config_file = Some(config_file);

    let (stdout_cfg, stderr_cfg) = get_stdio_config();

    // PgBouncer refuses to run as root, so we need to run it as postgres user
    let child = if is_root() {
        Command::new("sudo")
            .arg("-u")
            .arg("postgres")
            .arg("pgbouncer")
            .arg(&config_path)
            .stdout(stdout_cfg)
            .stderr(stderr_cfg)
            .spawn()
            .expect("Failed to start pgbouncer as postgres user")
    } else {
        Command::new("pgbouncer")
            .arg(&config_path)
            .stdout(stdout_cfg)
            .stderr(stderr_cfg)
            .spawn()
            .expect("Failed to start pgbouncer")
    };

    world.pgbouncer_process = Some(child);

    wait_for_service_ready(
        "pgbouncer",
        pgbouncer_port,
        world.pgbouncer_process.as_mut().unwrap(),
    )
    .await;
}

pub fn stop_pgbouncer(child: &mut std::process::Child) {
    stop_process_immediate(child);
}
