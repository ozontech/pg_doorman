extern crate log;

use log::{info, LevelFilter};
use std::process;
use syslog::{BasicLogger, Facility, Formatter3164};
use tracing_subscriber::EnvFilter;

use super::args::{Args, LogFormat};
use super::log_level::LogLevelController;
use crate::config::{Config, VERSION};

pub fn init_logging(args: &Args, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    init(args, config.general.syslog_prog_name.clone());
    info!("Welcome to PgDoorman! (Version {VERSION})");
    Ok(())
}

fn init(args: &Args, syslog_name: Option<String>) {
    if let Some(syslog_name) = syslog_name {
        // Syslog mode: wrap BasicLogger in LogLevelController
        let formatter = Formatter3164 {
            facility: Facility::LOG_USER,
            hostname: None,
            process: syslog_name,
            pid: process::id(),
        };
        let syslog_logger = syslog::unix(formatter).unwrap();
        let inner = Box::new(BasicLogger::new(syslog_logger));
        let startup_level = LevelFilter::Info;
        LogLevelController::new(inner, startup_level).register();
    } else {
        // Tracing mode: build subscriber, set as global, wrap LogTracer in controller
        let startup_level: LevelFilter = match args.log_level {
            tracing::Level::ERROR => LevelFilter::Error,
            tracing::Level::WARN => LevelFilter::Warn,
            tracing::Level::INFO => LevelFilter::Info,
            tracing::Level::DEBUG => LevelFilter::Debug,
            tracing::Level::TRACE => LevelFilter::Trace,
        };
        let filter = EnvFilter::from_default_env().add_directive(args.log_level.into());

        let trace_sub = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(!args.no_color);

        // Use .finish() instead of .init() so we control the log facade ourselves
        match args.log_format {
            LogFormat::Structured => {
                let subscriber = trace_sub.json().finish();
                tracing::subscriber::set_global_default(subscriber).unwrap();
            }
            LogFormat::Debug => {
                let subscriber = trace_sub.pretty().finish();
                tracing::subscriber::set_global_default(subscriber).unwrap();
            }
            _ => {
                let subscriber = trace_sub.finish();
                tracing::subscriber::set_global_default(subscriber).unwrap();
            }
        };

        // Bridge log → tracing, wrapped in our controller for runtime level changes
        let inner = Box::new(tracing_log::LogTracer::new());
        LogLevelController::new(inner, startup_level).register();
    }
}
