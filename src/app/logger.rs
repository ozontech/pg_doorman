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
    let startup_level: LevelFilter = (&args.log_level).into();

    if let Some(syslog_name) = syslog_name {
        let formatter = Formatter3164 {
            facility: Facility::LOG_USER,
            hostname: None,
            process: syslog_name,
            pid: process::id(),
        };
        let syslog_logger = match syslog::unix(formatter) {
            Ok(logger) => logger,
            Err(err) => {
                eprintln!("fatal: failed to open syslog socket (/dev/log): {err}");
                eprintln!(
                    "hint: disable syslog_prog_name in config or ensure /dev/log is available"
                );
                std::process::exit(1);
            }
        };
        let inner = Box::new(BasicLogger::new(syslog_logger));
        LogLevelController::new(inner, startup_level).register();
    } else {
        let inner: Box<dyn Log> = match args.log_format {
            LogFormat::Structured => Box::new(JsonLogger::new()),
            _ => Box::new(TextLogger::new(should_use_color(args.no_color))),
        };

        LogLevelController::new(inner, startup_level).register();
    }
}

/// Decide whether the text logger should emit ANSI colour escapes.
///
/// The runtime answer threads three inputs through [`resolve_color`]:
/// the explicit `--no-color` flag, the standard `NO_COLOR` env var
/// (<https://no-color.org/>: any non-empty value disables colour), and
/// whether stderr is a TTY. Under systemd the unit's stderr is a pipe
/// to journald, not a terminal, so colour escapes used to land in the
/// journal as `[NNN blob data]` placeholders; auto-disabling for a
/// non-TTY stderr fixes that without requiring every operator to pass
/// `--no-color`.
fn should_use_color(no_color_flag: bool) -> bool {
    use std::io::IsTerminal;
    let env_no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    resolve_color(no_color_flag, env_no_color, std::io::stderr().is_terminal())
}

/// Pure decision used by [`should_use_color`]: colour is on only when
/// every gate is open. Split out so the matrix of inputs can be unit
/// tested without touching real env vars or a real terminal handle.
fn resolve_color(no_color_flag: bool, env_no_color: bool, stderr_is_tty: bool) -> bool {
    !no_color_flag && !env_no_color && stderr_is_tty
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
        let mut escaped = String::with_capacity(msg_str.len());
        for ch in msg_str.chars() {
            match ch {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                c if c.is_control() => {
                    escaped.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => escaped.push(c),
            }
        }

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

#[cfg(test)]
mod tests {
    use super::resolve_color;

    #[test]
    fn color_on_only_when_every_gate_is_open() {
        assert!(resolve_color(
            /*no_color_flag=*/ false, /*env_no_color=*/ false,
            /*stderr_is_tty=*/ true,
        ));
    }

    #[test]
    fn color_off_when_no_color_flag_set() {
        assert!(!resolve_color(true, false, true));
    }

    #[test]
    fn color_off_when_no_color_env_set() {
        // `NO_COLOR` is the open standard from https://no-color.org/:
        // any non-empty value disables colour. The pure decision sees
        // the boolean after the env lookup.
        assert!(!resolve_color(false, true, true));
    }

    #[test]
    fn color_off_when_stderr_is_not_a_tty() {
        // The fix this guard exists for: under systemd the journal
        // pipe is not a terminal, so default colour-on used to leak
        // ANSI escapes into journalctl as `[NNN blob data]`.
        assert!(!resolve_color(false, false, false));
    }
}
