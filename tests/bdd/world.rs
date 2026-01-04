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
    /// Result of the last test command execution
    pub last_test_result: Option<TestCommandResult>,
    /// PostgreSQL connection
    pub pg_conn: Option<crate::pg_connection::PgConnection>,
    /// pg_doorman connection
    pub doorman_conn: Option<crate::pg_connection::PgConnection>,
    /// Accumulated messages from PG
    pub pg_accumulated_messages: Vec<(char, Vec<u8>)>,
    /// Accumulated messages from Doorman
    pub doorman_accumulated_messages: Vec<(char, Vec<u8>)>,
    /// Named sessions (for multi-session tests)
    pub named_sessions: HashMap<String, crate::pg_connection::PgConnection>,
    /// Backend PIDs for named sessions
    pub session_backend_pids: HashMap<String, i32>,
    /// Named backend PIDs (for storing multiple PIDs per session with custom keys)
    pub named_backend_pids: HashMap<(String, String), i32>,
    /// Messages from named sessions (for prepared statements cache tests)
    pub session_messages: HashMap<String, Vec<(char, Vec<u8>)>>,
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
            .finish()
    }
}
