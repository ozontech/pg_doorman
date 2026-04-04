pub mod clock;
pub mod core_affinity;
pub mod dashmap;
pub mod debug_messages;
pub mod rate_limit;

/// Format chrono::Duration in Go-style: `4m30s`, `1h30m`, `2d 4h30m`.
pub fn format_duration(duration: &chrono::Duration) -> String {
    let total_ms = duration.num_milliseconds();
    if total_ms <= 0 {
        return "0ms".to_string();
    }
    format_duration_ms(total_ms as u64)
}

/// Format std::time::Duration in Go-style. Accepts `Instant::elapsed()`, `Duration::from_*()`.
pub fn format_elapsed(d: std::time::Duration) -> String {
    format_duration_ms(d.as_millis() as u64)
}

/// Format milliseconds as Go-style duration: `123ms`, `5.123s`, `4m30s`, `1h30m`, `2d 4h30m`.
pub fn format_duration_ms(ms: u64) -> String {
    if ms == 0 {
        return "0ms".to_string();
    }
    if ms < 1_000 {
        return format!("{}ms", ms);
    }
    if ms < 60_000 {
        let secs = ms / 1_000;
        let frac = ms % 1_000;
        return if frac > 0 {
            format!("{}.{:03}s", secs, frac)
        } else {
            format!("{}s", secs)
        };
    }

    let total_secs = ms / 1_000;
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3_600;
    let mins = (total_secs % 3_600) / 60;
    let secs = total_secs % 60;

    let mut result = String::new();
    if days > 0 {
        result.push_str(&format!("{}d", days));
    }

    let mut sub_day = String::new();
    if hours > 0 {
        sub_day.push_str(&format!("{}h", hours));
    }
    if mins > 0 {
        sub_day.push_str(&format!("{}m", mins));
    }
    // Drop seconds when days are present (too much precision)
    if secs > 0 && days == 0 {
        sub_day.push_str(&format!("{}s", secs));
    }

    if !sub_day.is_empty() {
        if days > 0 {
            result.push(' ');
        }
        result.push_str(&sub_day);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_ms_zero() {
        assert_eq!(format_duration_ms(0), "0ms");
    }

    #[test]
    fn test_format_duration_ms_under_one_second() {
        assert_eq!(format_duration_ms(1), "1ms");
        assert_eq!(format_duration_ms(500), "500ms");
        assert_eq!(format_duration_ms(999), "999ms");
    }

    #[test]
    fn test_format_duration_ms_seconds() {
        assert_eq!(format_duration_ms(1_000), "1s");
        assert_eq!(format_duration_ms(5_123), "5.123s");
        assert_eq!(format_duration_ms(30_000), "30s");
        assert_eq!(format_duration_ms(59_999), "59.999s");
    }

    #[test]
    fn test_format_duration_ms_minutes() {
        assert_eq!(format_duration_ms(60_000), "1m");
        assert_eq!(format_duration_ms(90_000), "1m30s");
        assert_eq!(format_duration_ms(270_134), "4m30s");
        assert_eq!(format_duration_ms(300_000), "5m");
    }

    #[test]
    fn test_format_duration_ms_hours() {
        assert_eq!(format_duration_ms(3_600_000), "1h");
        assert_eq!(format_duration_ms(5_400_000), "1h30m");
        assert_eq!(format_duration_ms(7_261_000), "2h1m1s");
    }

    #[test]
    fn test_format_duration_ms_days() {
        assert_eq!(format_duration_ms(86_400_000), "1d");
        assert_eq!(format_duration_ms(90_000_000), "1d 1h");
        assert_eq!(format_duration_ms(176_400_000), "2d 1h");
    }

    #[test]
    fn test_format_duration_ms_days_drops_seconds() {
        // When days > 0, seconds are omitted for readability
        assert_eq!(format_duration_ms(86_401_000), "1d");
        assert_eq!(format_duration_ms(86_460_000), "1d 1m");
    }

    #[test]
    fn test_format_duration_chrono_negative() {
        let d = chrono::Duration::milliseconds(-100);
        assert_eq!(format_duration(&d), "0ms");
    }

    #[test]
    fn test_format_duration_chrono_positive() {
        let d = chrono::Duration::milliseconds(270_134);
        assert_eq!(format_duration(&d), "4m30s");
    }
}
