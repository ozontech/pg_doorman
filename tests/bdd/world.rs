use cucumber::World;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use tempfile::{NamedTempFile, TempDir};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

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
    /// Background query task handle
    pub background_query_handle: Option<JoinHandle<()>>,
    /// Background query client (wrapped in Arc<Mutex> for cancellation)
    pub background_query_client: Option<Arc<Mutex<Option<tokio_postgres::Client>>>>,
    /// Backend PID from PostgreSQL (server connection pid)
    pub backend_pid: Option<i32>,
    /// Result of the last test command execution
    pub last_test_result: Option<TestCommandResult>,
}

impl std::fmt::Debug for DoormanWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoormanWorld")
            .field("pg_tmp_dir", &self.pg_tmp_dir)
            .field("pg_port", &self.pg_port)
            .field("pg_db_path", &self.pg_db_path)
            .field("doorman_process", &self.doorman_process.as_ref().map(|p| p.id()))
            .field("doorman_port", &self.doorman_port)
            .field("doorman_config_file", &self.doorman_config_file.as_ref().map(|f| f.path()))
            .finish()
    }
}
