extern crate log;

use log::{info, LevelFilter, Log, Metadata, Record};
use std::io::Write;
use std::process;
use syslog::{BasicLogger, Facility, Formatter3164};

use super::args::{Args, LogFormat};
use super::log_level::LogLevelController;
use crate::config::{Config, VERSION};

pub fn init_logging(args: &Args, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    init(args, config.general.syslog_prog_name.clone());
    info!("Welcome to PgDoorman! (Version {VERSION})");
    Ok(())
}

fn init(args: &Args, syslog_name: Option<String>) {
    let startup_level: LevelFilter = match args.log_level {
        tracing::Level::ERROR => LevelFilter::Error,
        tracing::Level::WARN => LevelFilter::Warn,
        tracing::Level::INFO => LevelFilter::Info,
        tracing::Level::DEBUG => LevelFilter::Debug,
        tracing::Level::TRACE => LevelFilter::Trace,
    };

    if let Some(syslog_name) = syslog_name {
        let formatter = Formatter3164 {
            facility: Facility::LOG_USER,
            hostname: None,
            process: syslog_name,
            pid: process::id(),
        };
        let syslog_logger = syslog::unix(formatter).unwrap();
        let inner = Box::new(BasicLogger::new(syslog_logger));
        LogLevelController::new(inner, startup_level).register();
    } else {
        let inner: Box<dyn Log> = match args.log_format {
            LogFormat::Structured => Box::new(JsonLogger::new()),
            _ => Box::new(TextLogger::new(!args.no_color)),
        };

        LogLevelController::new(inner, startup_level).register();
    }
}

/// Direct text logger — no tracing bridge.
/// Format: `2024-01-07T19:19:38.080Z  INFO pg_doorman::pool: message`
struct TextLogger {
    use_color: bool,
}

impl TextLogger {
    fn new(use_color: bool) -> Self {
        Self { use_color }
    }
}

impl Log for TextLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true // LogLevelController handles filtering
    }

    fn log(&self, record: &Record) {
        let now = chrono::Utc::now();
        let level = record.level();
        let file = record.file().unwrap_or("unknown");
        let line = record.line().unwrap_or(0);

        if self.use_color {
            let level_color = match level {
                log::Level::Error => "\x1b[31m", // red
                log::Level::Warn => "\x1b[33m",  // yellow
                log::Level::Info => "\x1b[32m",  // green
                log::Level::Debug => "\x1b[36m", // cyan
                log::Level::Trace => "\x1b[90m", // gray
            };
            let reset = "\x1b[0m";
            let _ = writeln!(
                std::io::stderr(),
                "{} {}{:>5}{} {}:{}: {}",
                now.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
                level_color,
                level,
                reset,
                file,
                line,
                record.args()
            );
        } else {
            let _ = writeln!(
                std::io::stderr(),
                "{} {:>5} {}:{}: {}",
                now.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
                level,
                file,
                line,
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Direct JSON logger — structured output without tracing overhead.
struct JsonLogger;

impl JsonLogger {
    fn new() -> Self {
        Self
    }
}

impl Log for JsonLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        let now = chrono::Utc::now();
        let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let level = match record.level() {
            log::Level::Error => "ERROR",
            log::Level::Warn => "WARN",
            log::Level::Info => "INFO",
            log::Level::Debug => "DEBUG",
            log::Level::Trace => "TRACE",
        };
        let file = record.file().unwrap_or("unknown");
        let line = record.line().unwrap_or(0);
        let msg = record.args();

        // Escape JSON string manually to avoid serde overhead.
        // Message is the only field that can contain arbitrary user data.
        let msg_str = msg.to_string();
        let escaped = msg_str
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");

        let _ = writeln!(
            std::io::stderr(),
            r#"{{"timestamp":"{}","level":"{}","file":"{}","line":{},"message":"{}"}}"#,
            timestamp,
            level,
            file,
            line,
            escaped,
        );
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}
