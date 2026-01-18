use cucumber::World;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use tempfile::{NamedTempFile, TempDir};

/// Result of a test command execution
#[derive(Default, Clone)]
pub struct TestCommandResult {
    /// Exit code of the command
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Whether the command succeeded (exit code 0)
    pub success: bool,
}

/// The World struct holds the state shared across all steps in a scenario.
#[derive(Default, World)]
pub struct DoormanWorld {
    /// Temporary directory for PostgreSQL data
    pub pg_tmp_dir: Option<TempDir>,
    /// PostgreSQL port
    pub pg_port: Option<u16>,
    /// PostgreSQL database path
    pub pg_db_path: Option<PathBuf>,
    /// pg_doorman process handle
    pub doorman_process: Option<Child>,
    /// pg_doorman port
    pub doorman_port: Option<u16>,
    /// Temporary config file for pg_doorman (kept alive while process runs)
    pub doorman_config_file: Option<NamedTempFile>,
    /// Temporary pg_hba file for pg_doorman (kept alive while process runs)
    pub doorman_hba_file: Option<NamedTempFile>,
    /// Temporary SSL private key file for pg_doorman
    pub ssl_key_file: Option<NamedTempFile>,
    /// Temporary SSL certificate file for pg_doorman
    pub ssl_cert_file: Option<NamedTempFile>,
    /// Path to daemon PID file (for cleanup after binary-upgrade)
    pub doorman_daemon_pid_file: Option<String>,
    /// Result of the last test command execution
    pub last_test_result: Option<TestCommandResult>,
    /// PostgreSQL connection
    pub pg_conn: Option<crate::pg_connection::PgConnection>,
    /// pg_doorman connection
    pub doorman_conn: Option<crate::pg_connection::PgConnection>,
    /// pgbouncer process handle
    pub pgbouncer_process: Option<Child>,
    /// pgbouncer port
    pub pgbouncer_port: Option<u16>,
    /// pgbouncer config file
    pub pgbouncer_config_file: Option<NamedTempFile>,
    /// pgbouncer userlist file (for authentication)
    pub pgbouncer_userlist_file: Option<NamedTempFile>,
    /// odyssey process handle
    pub odyssey_process: Option<Child>,
    /// odyssey port
    pub odyssey_port: Option<u16>,
    /// odyssey config file
    pub odyssey_config_file: Option<NamedTempFile>,
    /// Accumulated messages from PG
    pub pg_accumulated_messages: Vec<(char, Vec<u8>)>,
    /// Accumulated messages from Doorman
    pub doorman_accumulated_messages: Vec<(char, Vec<u8>)>,
    /// Named sessions (for multi-session tests)
    pub named_sessions: HashMap<String, crate::pg_connection::PgConnection>,
    /// Backend PIDs for named sessions
    pub session_backend_pids: HashMap<String, i32>,
    /// Secret keys for named sessions (from BackendKeyData, used for cancel requests)
    pub session_secret_keys: HashMap<String, i32>,
    /// Named backend PIDs (for storing multiple PIDs per session with custom keys)
    pub named_backend_pids: HashMap<(String, String), i32>,
    /// Messages from named sessions (for prepared statements cache tests)
    pub session_messages: HashMap<String, Vec<(char, Vec<u8>)>>,
    /// Benchmark results: target name -> tps (transactions per second)
    pub bench_results: HashMap<String, f64>,
    /// Temporary pgbench script file (created once, reused for all benchmarks)
    pub pgbench_script_file: Option<NamedTempFile>,
    /// Flag indicating if this is a benchmark scenario (affects log level)
    pub is_bench: bool,
    /// Benchmark start time (set when first pgbench runs)
    pub bench_start_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Benchmark end time (set when generating markdown table)
    pub bench_end_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Internal pool for direct Pool.get benchmarking
    pub internal_pool: Option<crate::pool_bench_helper::InternalPool>,
}

impl DoormanWorld {
    /// Replace all known placeholders in the given text
    pub fn replace_placeholders(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Replace port placeholders
        if let Some(port) = self.doorman_port {
            result = result.replace("${DOORMAN_PORT}", &port.to_string());
        }
        if let Some(port) = self.pg_port {
            result = result.replace("${PG_PORT}", &port.to_string());
        }
        if let Some(port) = self.pgbouncer_port {
            result = result.replace("${PGBOUNCER_PORT}", &port.to_string());
        }
        if let Some(port) = self.odyssey_port {
            result = result.replace("${ODYSSEY_PORT}", &port.to_string());
        }

        // Replace file path placeholders
        if let Some(ref hba_file) = self.doorman_hba_file {
            result = result.replace("${DOORMAN_HBA_FILE}", hba_file.path().to_str().unwrap());
        }
        if let Some(ref ssl_key_file) = self.ssl_key_file {
            result = result.replace("${DOORMAN_SSL_KEY}", ssl_key_file.path().to_str().unwrap());
        }
        if let Some(ref ssl_cert_file) = self.ssl_cert_file {
            result = result.replace(
                "${DOORMAN_SSL_CERT}",
                ssl_cert_file.path().to_str().unwrap(),
            );
        }
        if let Some(ref script_file) = self.pgbench_script_file {
            result = result.replace("${PGBENCH_FILE}", script_file.path().to_str().unwrap());
        }
        if let Some(ref userlist_file) = self.pgbouncer_userlist_file {
            result = result.replace(
                "${PGBOUNCER_USERLIST}",
                userlist_file.path().to_str().unwrap(),
            );
        }

        result
    }
}

impl std::fmt::Debug for DoormanWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoormanWorld")
            .field("pg_tmp_dir", &self.pg_tmp_dir)
            .field("pg_port", &self.pg_port)
            .field("pg_db_path", &self.pg_db_path)
            .field(
                "doorman_process",
                &self.doorman_process.as_ref().map(|p| p.id()),
            )
            .field("doorman_port", &self.doorman_port)
            .field(
                "doorman_config_file",
                &self.doorman_config_file.as_ref().map(|f| f.path()),
            )
            .field(
                "pgbouncer_process",
                &self.pgbouncer_process.as_ref().map(|p| p.id()),
            )
            .field("pgbouncer_port", &self.pgbouncer_port)
            .field(
                "odyssey_process",
                &self.odyssey_process.as_ref().map(|p| p.id()),
            )
            .field("odyssey_port", &self.odyssey_port)
            .finish()
    }
}
