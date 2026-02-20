use crate::doorman_helper::{stop_doorman, wait_for_doorman_ready};
use crate::utils::{get_stdio_config, is_debug_mode};
use crate::world::{DoormanWorld, TestCommandResult};
use cucumber::{given, when};
use portpicker::pick_unused_port;
use std::io::Write;
use std::process::Command;
/// Run `pg_doorman generate` with given args and save the output to a temp file
/// with the appropriate extension (.toml or .yaml).
#[when(expr = "we generate pg_doorman config with args {string} to {string} format")]
pub async fn generate_config(world: &mut DoormanWorld, args: String, format: String) {
    let args = world.replace_placeholders(&args);

    let extension = match format.as_str() {
        "toml" => ".toml",
        "yaml" | "yml" => ".yaml",
        other => panic!("Unsupported format: {}", other),
    };

    // Create temp file with the right extension
    let output_file = tempfile::Builder::new()
        .suffix(extension)
        .tempfile()
        .expect("Failed to create temp file for generated config");

    let output_path = output_file.path().to_str().unwrap().to_string();

    let doorman_binary = env!("CARGO_BIN_EXE_pg_doorman");

    // Build the full command: pg_doorman generate {args} -o {output_path}
    let full_command = format!("{} generate {} -o {}", doorman_binary, args, output_path);

    let output = Command::new("sh")
        .arg("-c")
        .arg(&full_command)
        .output()
        .expect("Failed to run pg_doorman generate");

    let result = TestCommandResult {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        success: output.status.success(),
    };

    world.last_test_result = Some(result);
    world.generated_config_file = Some(output_file);
}

/// Start pg_doorman with a previously generated config file.
/// Patches the config to use a random free port and adds pg_hba trust rule.
#[given("pg_doorman started with generated config")]
pub async fn start_doorman_with_generated_config(world: &mut DoormanWorld) {
    // Stop any previously running pg_doorman
    if let Some(ref mut child) = world.doorman_process {
        stop_doorman(child);
    }
    world.doorman_process = None;

    let generated_file = world
        .generated_config_file
        .as_ref()
        .expect("No generated config file found. Run 'we generate pg_doorman config' step first.");

    let config_content =
        std::fs::read_to_string(generated_file.path()).expect("Failed to read generated config");

    let generated_path = generated_file.path().to_str().unwrap();
    let is_yaml = generated_path.ends_with(".yaml") || generated_path.ends_with(".yml");

    let doorman_port = pick_unused_port().expect("No free ports for pg_doorman");
    world.doorman_port = Some(doorman_port);

    let patched_content = if is_yaml {
        patch_yaml_config(&config_content, doorman_port)
    } else {
        patch_toml_config(&config_content, doorman_port)
    };

    // Create a new temp file with the patched config (preserving extension)
    let extension = if is_yaml { ".yaml" } else { ".toml" };
    let mut patched_file = tempfile::Builder::new()
        .suffix(extension)
        .tempfile()
        .expect("Failed to create temp file for patched config");
    patched_file
        .write_all(patched_content.as_bytes())
        .expect("Failed to write patched config");
    patched_file
        .flush()
        .expect("Failed to flush patched config");

    let config_path = patched_file.path().to_path_buf();

    let doorman_binary = env!("CARGO_BIN_EXE_pg_doorman");
    let log_level = if is_debug_mode() { "debug" } else { "info" };
    let (stdout_cfg, stderr_cfg) = get_stdio_config();

    let child = Command::new(doorman_binary)
        .arg(&config_path)
        .arg("-l")
        .arg(log_level)
        .stdout(stdout_cfg)
        .stderr(stderr_cfg)
        .spawn()
        .expect("Failed to start pg_doorman with generated config");

    world.doorman_process = Some(child);
    // Keep the patched config file alive
    world.doorman_config_file = Some(patched_file);

    wait_for_doorman_ready(doorman_port, world.doorman_process.as_mut().unwrap()).await;
}

/// Patch a TOML config: replace port and add pg_hba trust rule
fn patch_toml_config(content: &str, port: u16) -> String {
    let mut result = Vec::new();
    let mut port_replaced = false;

    for line in content.lines() {
        if !port_replaced && line.trim_start().starts_with("port") && line.contains("6432") {
            // Replace the default port with the random port
            let patched_line = line.replace("6432", &port.to_string());
            result.push(patched_line);
            port_replaced = true;
        } else {
            result.push(line.to_string());
        }
    }

    let mut output = result.join("\n");

    // Add pg_hba trust rule at the end of the [general] section
    // Find where to insert: after [general] block, before [pools]
    if let Some(pools_pos) = output.find("\n[pools]") {
        let insert = format!("\n[general.pg_hba]\ncontent = \"host all all 127.0.0.1/32 trust\"\n");
        output.insert_str(pools_pos, &insert);
    } else {
        // Fallback: append at the end
        output.push_str(&format!(
            "\n[general.pg_hba]\ncontent = \"host all all 127.0.0.1/32 trust\"\n"
        ));
    }

    output
}

/// Patch a YAML config: replace port and add pg_hba trust rule
fn patch_yaml_config(content: &str, port: u16) -> String {
    let mut result = Vec::new();
    let mut port_replaced = false;

    for line in content.lines() {
        if !port_replaced && line.contains("port:") && line.contains("6432") {
            let patched_line = line.replace("6432", &port.to_string());
            result.push(patched_line);
            // Add pg_hba right after port line, with proper indentation
            result.push("  pg_hba:".to_string());
            result.push("    content: \"host all all 127.0.0.1/32 trust\"".to_string());
            port_replaced = true;
        } else {
            result.push(line.to_string());
        }
    }

    result.join("\n")
}
