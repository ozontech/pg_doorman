use clap::{Parser, ValueEnum};
use tracing::Level;

/// PgDoorman: Nextgen PostgreSQL Pooler (based on PgCat).
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(default_value_t = String::from("pg_doorman.toml"), env)]
    pub config_file: String,

    #[arg(short, long, default_value_t = tracing::Level::INFO, env)]
    pub log_level: Level,

    #[clap(short = 'F', long, value_enum, default_value_t = LogFormat::Text, env)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_parses() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }
}
