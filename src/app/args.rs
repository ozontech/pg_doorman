use clap::{Parser, Subcommand, ValueEnum};
use std::fmt;
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

    #[arg(
        short = 't',
        long = "test-config",
        default_value_t = false,
        help = "test configuration file and exit"
    )]
    pub test_config: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Generate configuration for pg_doorman by connecting to PostgreSQL and auto-detecting databases and users
    Generate {
        #[clap(flatten)]
        config: GenerateConfig,
    },
    /// Generate reference documentation (Markdown) for all configuration parameters
    GenerateDocs {
        /// Output directory for generated documentation files.
        /// If not specified, prints to stdout.
        #[arg(short, long)]
        output_dir: Option<String>,
        /// Generate docs for all languages (EN + RU).
        #[arg(long, default_value = "false")]
        all_languages: bool,
        /// Generate docs in Russian only (by default generates English).
        #[arg(long, alias = "ru", default_value = "false")]
        russian: bool,
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
    /// Disable comments in generated config (by default, comments are included).
    #[arg(long, default_value = "false")]
    pub(crate) no_comments: bool,
    /// Generate reference config without PG connection (uses example values).
    #[arg(long, default_value = "false")]
    pub(crate) reference: bool,
    /// Generate comments in Russian for quick start guide.
    #[arg(long, alias = "ru", default_value = "false")]
    pub(crate) russian_comments: bool,
    /// Output format: yaml (default) or toml.
    /// If --output is specified, format is auto-detected from file extension.
    /// This flag overrides the auto-detected format.
    #[arg(short, long, value_enum)]
    pub(crate) format: Option<OutputFormat>,
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

#[derive(ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    Yaml,
    Toml,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Yaml => write!(f, "yaml"),
            OutputFormat::Toml => write!(f, "toml"),
        }
    }
}
