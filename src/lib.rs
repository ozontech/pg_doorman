pub mod admin;
pub mod auth;
pub mod client;
pub mod cmd_args;
pub mod comments;
pub mod config;
pub mod core_affinity;
pub mod daemon;
pub mod errors;
pub mod generate;
pub mod logger;
pub mod messages;
pub mod pool;
pub mod prometheus;
pub mod rate_limit;
pub mod server;
pub mod stats;

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
