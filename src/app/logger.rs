extern crate log;

use log::LevelFilter;
use std::process;
use syslog::{BasicLogger, Facility, Formatter3164};
use tracing_subscriber;
use tracing_subscriber::EnvFilter;

use super::args::{Args, LogFormat};
use crate::config::{Config, VERSION};

pub fn init_logging(args: &Args, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    use log::info;

    init(args, config.general.syslog_prog_name.clone());
    info!("Welcome to PgDoorman! (Version {VERSION})");
    Ok(())
}

fn init(args: &Args, syslog_name: Option<String>) {
    if let Some(syslog_name) = syslog_name {
        let formatter = Formatter3164 {
            facility: Facility::LOG_USER,
            hostname: None,
            process: syslog_name,
            pid: process::id(),
        };
        let syslog_logger = syslog::unix(formatter).unwrap();
        // max level in syslog mode is INFO (performance penalty for DEBUG).
        log::set_boxed_logger(Box::new(BasicLogger::new(syslog_logger)))
            .map(|()| log::set_max_level(LevelFilter::Info))
            .unwrap();
    } else {
        // Iniitalize a default filter, and then override the builtin default "warning" with our
        // commandline, (default: "info")
        let filter = EnvFilter::from_default_env().add_directive(args.log_level.into());

        let trace_sub = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(!args.no_color);

        match args.log_format {
            LogFormat::Structured => trace_sub.json().init(),
            LogFormat::Debug => trace_sub.pretty().init(),
            _ => trace_sub.init(),
        };
    }
}
