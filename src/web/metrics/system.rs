//! System metrics utilities for Prometheus exporter.

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

/// Returns the kernel page size in bytes.
///
/// `/proc/self/statm` reports memory in pages, and the page size is
/// architecture-specific: 4 KiB on x86_64, 16 KiB on ARM64 with a
/// non-default kernel build, configurable on PowerPC. Querying
/// `sysconf(_SC_PAGESIZE)` once at startup gives the correct value
/// instead of hard-coding 4096 — the latter under-reports RSS by 4×
/// on a 16 KiB-page kernel.
#[cfg(target_os = "linux")]
fn page_size_bytes() -> u64 {
    static PAGE_SIZE: OnceLock<u64> = OnceLock::new();
    *PAGE_SIZE.get_or_init(|| {
        // SAFETY: sysconf is async-signal-safe and `_SC_PAGESIZE` is
        // defined on every Linux ABI we ship to. A non-positive return
        // would indicate an out-of-spec kernel; fall back to 4096 in
        // that case to keep the metric meaningful instead of zeroed.
        let raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if raw > 0 {
            raw as u64
        } else {
            4096
        }
    })
}

/// Gets the current resident memory (RSS) of the process in bytes.
///
/// `/proc/self/statm` columns are documented in `man 5 proc`:
/// `size resident shared text lib data dt`. We want **resident** —
/// the number of pages backed by RAM right now (VmRSS). The first
/// field is `size` (VmSize, total virtual address space) and would
/// over-count by the heap arenas, mmaps, and library text pages
/// that the process has reserved but does not currently touch.
pub fn get_process_memory_usage() -> u64 {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/self/statm") {
            Ok(statm) => {
                let values: Vec<&str> = statm.split_whitespace().collect();
                if values.len() >= 2 {
                    if let Ok(pages) = values[1].parse::<u64>() {
                        return pages * page_size_bytes();
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
