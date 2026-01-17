use clap::{Parser, Subcommand, ValueEnum};
use tracing::Level;

/// PgDoorman: Nextgen PostgreSQL Pooler (based on PgCat).
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(default_value_t = String::from("pg_doorman.toml"), env)]
    pub config_file: String,

    #[arg(short, long, default_value_t = tracing::Level::INFO, env)]
    pub log_level: Level,

    #[clap(short='F', long, value_enum, default_value_t=LogFormat::Text, env)]
    pub log_format: LogFormat,

    #[arg(
        short,
        long,
        default_value_t = false,
        env,
        help = "disable colors in the log output"
    )]
    pub no_color: bool,

    #[arg(short, long, default_value_t = false, env, help = "run as daemon")]
    pub daemon: bool,

    #[arg(
        long,
        env,
        help = "inherit listener file descriptor from parent process (for binary upgrade in foreground mode)"
    )]
    pub inherit_fd: Option<i32>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Generate configuration for pg_doorman by connecting to PostgreSQL and auto-detecting databases and users
    Generate {
        #[clap(flatten)]
        config: GenerateConfig,
    },
}

#[derive(Debug, Clone, Parser)]
pub struct GenerateConfig {
    /// PostgreSQL host to connect to.
    /// If not specified, uses localhost.
    /// Environment variable: PGHOST
    #[arg(long, env = "PGHOST")]
    pub(crate) host: Option<String>,
    /// PostgreSQL port to connect to.
    /// If not specified, uses 5432.
    /// Environment variable: PGPORT
    #[arg(short, long, env = "PGPORT", default_value_t = 5432)]
    pub(crate) port: u16,
    /// PostgreSQL user to connect as.
    /// Required superuser privileges to read pg_shadow.
    /// If not specified, uses the current user.
    /// Environment variable: PGUSER
    #[arg(short, long, env = "PGUSER")]
    pub(crate) user: Option<String>,
    /// PostgreSQL password to connect with.
    /// Environment variable: PGPASSWORD
    #[arg(long, env = "PGPASSWORD")]
    pub(crate) password: Option<String>,
    /// PostgreSQL database to connect to.
    /// If not specified, uses the same name as the user.
    /// Environment variable: PGDATABASE
    #[arg(short, long, env = "PGDATABASE")]
    pub(crate) database: Option<String>,
    /// PostgreSQL connection to server via tls.
    #[arg(long, default_value = "false")]
    pub(crate) ssl: bool,
    /// Pool size for the generated configuration.
    /// If not specified, uses 40.
    #[arg(long, default_value_t = 40)]
    pub(crate) pool_size: u32,
    /// Session pool mode for the generated configuration.
    /// If not specified, uses false.
    #[arg(short, long, default_value = "false")]
    pub(crate) session_pool_mode: bool,
    /// Output file for the generated configuration.
    /// If not specified, uses stdout.
    #[arg(short, long)]
    pub output: Option<String>,
    /// Override server_host in config
    /// If not specified, it uses the ` host ` parameter.
    #[arg(long)]
    pub(crate) server_host: Option<String>,
}

pub fn parse() -> Args {
    Args::parse()
}

#[derive(ValueEnum, Clone, Debug)]
pub enum LogFormat {
    Text,
    Structured,
    Debug,
}
