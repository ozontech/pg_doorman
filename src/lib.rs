pub mod admin;
pub mod app;
pub mod auth;
pub mod client;
pub mod config;
pub mod daemon;
pub mod errors {
    pub use crate::app::errors::*;
}
pub mod logger {
    pub use crate::app::logger::*;
}
pub mod messages;
pub mod pool;
pub mod prometheus;
pub mod server;
pub mod stats;
pub mod utils;

// Backward-compatible module path (was `src/cmd_args.rs`).
pub mod cmd_args {
    pub use crate::app::args::*;

    pub fn parse() -> Args {
        crate::app::args::parse()
    }
}

// Backward-compatible re-exports (old module paths).
pub use utils::{comments, core_affinity, rate_limit};

pub use config::tls;

/// Format chrono::Duration to be more human-friendly.
///
/// # Arguments
///
/// * `duration` - A duration of time
pub fn format_duration(duration: &chrono::Duration) -> String {
    let milliseconds = format!("{:0>3}", duration.num_milliseconds() % 1000);

    let seconds = format!("{:0>2}", duration.num_seconds() % 60);

    let minutes = format!("{:0>2}", duration.num_minutes() % 60);

    let hours = format!("{:0>2}", duration.num_hours() % 24);

    let days = duration.num_days().to_string();

    format!("{days}d {hours}:{minutes}:{seconds}.{milliseconds}")
}
