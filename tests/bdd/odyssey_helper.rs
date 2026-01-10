use crate::service_helper::{stop_process_immediate, wait_for_service_ready};
use crate::utils::{create_temp_file, get_stdio_config};
use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given};
use portpicker::pick_unused_port;
use std::process::Command;

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

    // Set port temporarily for placeholder replacement
    world.odyssey_port = Some(odyssey_port);
    let config_content = world.replace_placeholders(&config_content);

    let config_file = create_temp_file(&config_content);
    let config_path = config_file.path().to_path_buf();
    world.odyssey_config_file = Some(config_file);

    let (stdout_cfg, stderr_cfg) = get_stdio_config();

    let child = Command::new("odyssey")
        .arg("--log_to_stdout")
        .arg("--console")
        .arg(&config_path)
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .spawn()
        .expect("Failed to start odyssey");

    world.odyssey_process = Some(child);

    wait_for_service_ready(
        "odyssey",
        odyssey_port,
        world.odyssey_process.as_mut().unwrap(),
    )
    .await;
}

pub fn stop_odyssey(child: &mut std::process::Child) {
    stop_process_immediate(child);
}
