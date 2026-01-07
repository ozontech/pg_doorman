use log::info;

use pg_doorman::cmd_args::Args;
use pg_doorman::config::{Config, VERSION};
use pg_doorman::logger;

pub fn init_logging(args: &Args, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    logger::init(args, config.general.syslog_prog_name.clone());
    info!("Welcome to PgDoorman! (Version {VERSION})");
    Ok(())
}
