//! System metrics utilities for Prometheus exporter.

/// Gets the current memory usage of the process in bytes
pub fn get_process_memory_usage() -> u64 {
    #[cfg(target_os = "linux")]
    {
        // On Linux, read from /proc/self/statm
        match std::fs::read_to_string("/proc/self/statm") {
            Ok(statm) => {
                let values: Vec<&str> = statm.split_whitespace().collect();
                if !values.is_empty() {
                    if let Ok(pages) = values[0].parse::<u64>() {
                        // Convert pages to bytes (page size is typically 4KB)
                        return pages * 4096;
                    }
                }
                0
            }
            Err(_) => 0,
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        // On macOS, use ps command
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output();

        match output {
            Ok(output) => {
                let rss = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u64>();
                match rss {
                    Ok(kb) => kb * 1024, // Convert KB to bytes
                    Err(_) => 0,
                }
            }
            Err(_) => 0,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Default implementation for other platforms
        0
    }
}
