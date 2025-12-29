use crate::world::DoormanWorld;
use cucumber::{gherkin::Step, given, then};
use portpicker::pick_unused_port;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;

/// Check if the current process is running as root
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Build a PostgreSQL command, running as postgres user if we are root
fn pg_command_builder(cmd: &str, args: &[&str]) -> Command {
    if is_root() {
        let mut command = Command::new("sudo");
        command.arg("-u").arg("postgres").arg(cmd).args(args);
        command
    } else {
        let mut command = Command::new(cmd);
        command.args(args);
        command
    }
}

#[given("a temporary PostgreSQL database started")]
pub async fn start_postgres(world: &mut DoormanWorld) {
    let tmp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = tmp_dir.path().join("db");
    let port = pick_unused_port().expect("No free ports");

    if is_root() {
        // If we are root, we need to make sure the postgres user can access the temp dir
        Command::new("chown")
            .arg("-R")
            .arg("postgres:postgres")
            .arg(tmp_dir.path())
            .status()
            .expect("Failed to chown temp dir");
        // Also ensure it has proper permissions
        std::fs::set_permissions(tmp_dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("Failed to set permissions on temp dir");
    }

    // initdb (suppress output, show only on error)
    let output = pg_command_builder("initdb", &["-D", db_path.to_str().unwrap(), "--no-sync"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run initdb");
    if !output.status.success() {
        eprintln!("initdb stdout:\n{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("initdb stderr:\n{}", String::from_utf8_lossy(&output.stderr));
        panic!("initdb failed");
    }

    // Start and stop to initialize properly (suppress output)
    let _ = pg_command_builder(
        "pg_ctl",
        &[
            "-D",
            db_path.to_str().unwrap(),
            "-o",
            &format!("-p {} -F -k /tmp", port),
            "start",
        ],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
    let _ = pg_command_builder(
        "pg_ctl",
        &["-D", db_path.to_str().unwrap(), "stop", "-m", "immediate"],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();

    let log_path = tmp_dir.path().join("pg.log");
    // pg_ctl start (suppress output, logs go to pg.log)
    let _ = pg_command_builder(
        "pg_ctl",
        &[
            "-D",
            db_path.to_str().unwrap(),
            "-l",
            log_path.to_str().unwrap(),
            "-o",
            &format!("-p {} -F -k /tmp", port),
            "start",
        ],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .expect("Failed to start pg_ctl");

    // Wait for PG to be ready
    let mut success = false;
    for _ in 0..20 {
        let check = pg_command_builder(
            "pg_isready",
            &["-p", &port.to_string(), "-h", "127.0.0.1", "-t", "1"],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
        if let Ok(s) = check {
            if s.success() {
                success = true;
                break;
            }
        }
        sleep(Duration::from_millis(500)).await;
    }

    if !success {
        if let Ok(log_content) = std::fs::read_to_string(&log_path) {
            eprintln!("Postgres log:\n{}", log_content);
        }
        // Try one more time with psql just to be sure
        let check_psql = pg_command_builder(
            "psql",
            &[
                "-p",
                &port.to_string(),
                "-h",
                "127.0.0.1",
                "-c",
                "SELECT 1",
                "postgres",
            ],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
        if let Ok(s) = check_psql {
            if s.success() {
                success = true;
            }
        }
    }
    assert!(success, "Postgres failed to start");

    world.pg_tmp_dir = Some(tmp_dir);
    world.pg_port = Some(port);
    world.pg_db_path = Some(db_path);
}

#[given(expr = "fixtures from {string} applied")]
pub async fn apply_fixtures(world: &mut DoormanWorld, file_path: String) {
    let port = world.pg_port.expect("PG not started");
    let output = pg_command_builder(
        "psql",
        &[
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "-U",
            "postgres",
            "-d",
            "postgres",
            "-f",
            &file_path,
        ],
    )
    .output()
    .expect("Failed to run psql");
    if !output.status.success() {
        eprintln!("psql stdout:\n{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("psql stderr:\n{}", String::from_utf8_lossy(&output.stderr));
        panic!("Failed to apply fixtures from {}", file_path);
    }
}

/// Start PostgreSQL with inline pg_hba.conf content
#[given("PostgreSQL started with pg_hba.conf:")]
pub async fn start_postgres_with_hba(world: &mut DoormanWorld, step: &Step) {
    let hba_content = step
        .docstring
        .as_ref()
        .expect("hba_content not found")
        .to_string();
    let tmp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = tmp_dir.path().join("db");
    let port = pick_unused_port().expect("No free ports");

    if is_root() {
        // If we are root, we need to make sure the postgres user can access the temp dir
        Command::new("chown")
            .arg("-R")
            .arg("postgres:postgres")
            .arg(tmp_dir.path())
            .status()
            .expect("Failed to chown temp dir");
        // Also ensure it has proper permissions
        std::fs::set_permissions(tmp_dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("Failed to set permissions on temp dir");
    }

    // initdb with postgres user (suppress output, show only on error)
    let output = pg_command_builder("initdb", &["-D", db_path.to_str().unwrap(), "-U", "postgres", "--no-sync"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run initdb");
    if !output.status.success() {
        eprintln!("initdb stdout:\n{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("initdb stderr:\n{}", String::from_utf8_lossy(&output.stderr));
        panic!("initdb failed");
    }

    // Write custom pg_hba.conf
    let hba_path = db_path.join("pg_hba.conf");
    {
        let mut hba_file = std::fs::File::create(&hba_path).expect("Failed to create pg_hba.conf");
        hba_file
            .write_all(hba_content.as_bytes())
            .expect("Failed to write pg_hba.conf");
    }

    if is_root() {
        // Ensure postgres user owns the pg_hba.conf
        Command::new("chown")
            .arg("postgres:postgres")
            .arg(&hba_path)
            .status()
            .expect("Failed to chown pg_hba.conf");
    }

    let log_path = tmp_dir.path().join("pg.log");
    // pg_ctl start (suppress output, logs go to pg.log)
    let _ = pg_command_builder(
        "pg_ctl",
        &[
            "-D",
            db_path.to_str().unwrap(),
            "-l",
            log_path.to_str().unwrap(),
            "-o",
            &format!("-p {} -F -k /tmp", port),
            "start",
        ],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .expect("Failed to start pg_ctl");

    // Wait for PG to be ready
    let mut success = false;
    for _ in 0..20 {
        let check = pg_command_builder(
            "pg_isready",
            &["-p", &port.to_string(), "-h", "127.0.0.1", "-U", "postgres", "-t", "1"],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
        if let Ok(s) = check {
            if s.success() {
                success = true;
                break;
            }
        }
        sleep(Duration::from_millis(500)).await;
    }

    if !success {
        if let Ok(log_content) = std::fs::read_to_string(&log_path) {
            eprintln!("Postgres log:\n{}", log_content);
        }
        // Try one more time with psql just to be sure
        let check_psql = pg_command_builder(
            "psql",
            &[
                "-p",
                &port.to_string(),
                "-h",
                "127.0.0.1",
                "-U",
                "postgres",
                "-c",
                "SELECT 1",
                "postgres",
            ],
        )
        .status();
        if let Ok(s) = check_psql {
            if s.success() {
                success = true;
            }
        }
    }
    assert!(success, "Postgres failed to start");

    world.pg_tmp_dir = Some(tmp_dir);
    world.pg_port = Some(port);
    world.pg_db_path = Some(db_path);
}

/// Check that psql connection to pg_doorman succeeds
#[then(expr = "psql connection to pg_doorman as user {string} to database {string} with password {string} succeeds")]
pub async fn psql_connection_succeeds(world: &mut DoormanWorld, user: String, database: String, password: String) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    
    let status = Command::new("psql")
        .arg("-h")
        .arg("127.0.0.1")
        .arg("-p")
        .arg(doorman_port.to_string())
        .arg("-U")
        .arg(&user)
        .arg("-d")
        .arg(&database)
        .arg("-c")
        .arg("SELECT 1")
        .env("PGPASSWORD", &password)
        .status()
        .expect("Failed to run psql");
    
    assert!(status.success(), "psql connection to pg_doorman failed");
}

/// Stop PostgreSQL and pg_doorman when the world is dropped
impl Drop for DoormanWorld {
    fn drop(&mut self) {
        // Stop pg_doorman first
        if let Some(ref mut child) = self.doorman_process {
            crate::doorman_helper::stop_doorman(child);
        }
        // Then stop PostgreSQL (suppress output)
        if let Some(ref db_path) = self.pg_db_path {
            let _ = pg_command_builder(
                "pg_ctl",
                &["-D", db_path.to_str().unwrap(), "stop", "-m", "immediate"],
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        }
    }
}
