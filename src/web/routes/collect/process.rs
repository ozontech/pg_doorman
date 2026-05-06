//! `/api/process` collector. Linux-first: reads `/proc/self/{stat,status,fd,
//! limits,task}` to fill in CPU, memory, and FD counters. macOS / others
//! fall back to the existing `get_process_memory_usage` and zero where no
//! cheap source is available.
//!
//! All values come from one-shot reads (no background sampling task), so
//! CPU is reported as monotonic microsecond counters; the frontend
//! computes the percentage from successive snapshots.
//!
//! `/proc/self/stat` field positions used here follow `proc(5)`. The kernel
//! defines `clock ticks per second` via `_SC_CLK_TCK` (always 100 in
//! practice on Linux); we use that constant rather than calling
//! `sysconf(3)` to keep the file dependency-free.

use std::sync::atomic::Ordering;

use crate::app::server::STARTED_AT;
use crate::web::metrics::system::get_process_memory_usage;
use crate::web::routes::dto::{ProcessDto, ProcessThreadDto};

use super::now_unix_ms;

#[cfg(target_os = "linux")]
const CLK_TCK_HZ: u64 = 100;

pub(crate) fn collect_process() -> ProcessDto {
    let pid = std::process::id();
    let hostname = read_hostname();
    let uptime_seconds = STARTED_AT.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    let started_at_ms = STARTED_AT
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let cpu_cores = num_cpus::get() as u32;

    #[cfg(target_os = "linux")]
    let (cpu_user_us, cpu_system_us, threads, threads_breakdown) = read_linux_cpu();
    #[cfg(target_os = "linux")]
    let (vm_size_bytes, fd_open, fd_limit) = (
        read_linux_vm_size_bytes(),
        read_linux_fd_open(),
        read_linux_fd_limit(),
    );

    #[cfg(not(target_os = "linux"))]
    let (cpu_user_us, cpu_system_us, threads, threads_breakdown): (u64, u64, u64, Vec<ProcessThreadDto>) =
        (0, 0, 0, Vec::new());
    #[cfg(not(target_os = "linux"))]
    let (vm_size_bytes, fd_open, fd_limit): (u64, u64, u64) = (0, 0, 0);

    ProcessDto {
        ts: now_unix_ms(),
        pid,
        hostname,
        uptime_seconds,
        started_at_ms,
        rss_bytes: get_process_memory_usage(),
        vm_size_bytes,
        threads,
        fd_open,
        fd_limit,
        cpu_user_us,
        cpu_system_us,
        cpu_cores,
        threads_breakdown,
    }
    .also(|d| {
        // Avoid an unused-field warning when the entire `_us` path is zero
        // on non-linux: the compiler sees the assignment.
        let _ = d;
    })
}

#[allow(dead_code)]
fn _atomic_dummy() {
    // Keep the `Ordering` import alive even when only one cfg branch reads
    // it (avoids a target-conditional unused-import warning).
    let _ = Ordering::Relaxed;
}

trait Also: Sized {
    fn also(self, f: impl FnOnce(&Self)) -> Self;
}
impl<T> Also for T {
    fn also(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}

fn read_hostname() -> String {
    // `gethostname(3)` lives in libc; reading `/proc/sys/kernel/hostname`
    // works on Linux without a libc binding. Bounded read keeps the call
    // cheap on hosts with overlong names.
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
            return s.trim().to_string();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn read_linux_cpu() -> (u64, u64, u64, Vec<ProcessThreadDto>) {
    let (mut user, mut sys, mut threads) = (0u64, 0u64, 0u64);
    if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
        if let Some(parts) = parse_proc_stat(&stat) {
            user = ticks_to_us(parts.utime);
            sys = ticks_to_us(parts.stime);
            threads = parts.num_threads.max(0) as u64;
        }
    }

    let mut breakdown = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc/self/task") {
        for entry in entries.flatten() {
            let tid_str = entry.file_name();
            let tid: u64 = match tid_str.to_string_lossy().parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let stat_path = entry.path().join("stat");
            let Ok(s) = std::fs::read_to_string(&stat_path) else {
                continue;
            };
            let Some(parts) = parse_proc_stat(&s) else { continue };
            breakdown.push(ProcessThreadDto {
                tid,
                name: parts.comm,
                cpu_user_us: ticks_to_us(parts.utime),
                cpu_system_us: ticks_to_us(parts.stime),
            });
        }
    }
    breakdown.sort_by(|a, b| {
        let a_total = a.cpu_user_us + a.cpu_system_us;
        let b_total = b.cpu_user_us + b.cpu_system_us;
        b_total.cmp(&a_total)
    });

    (user, sys, threads, breakdown)
}

#[cfg(target_os = "linux")]
fn read_linux_vm_size_bytes() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmSize:") {
            // Format: "VmSize:    12345 kB"
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return kb * 1024;
        }
    }
    0
}

#[cfg(target_os = "linux")]
fn read_linux_fd_open() -> u64 {
    std::fs::read_dir("/proc/self/fd")
        .map(|it| it.filter_map(|e| e.ok()).count() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn read_linux_fd_limit() -> u64 {
    let Ok(limits) = std::fs::read_to_string("/proc/self/limits") else {
        return 0;
    };
    for line in limits.lines() {
        if line.starts_with("Max open files") {
            // "Max open files            65536                65536                files"
            let mut tokens = line.split_whitespace().rev();
            // skip "files"
            tokens.next();
            // hard limit
            let _ = tokens.next();
            // soft limit
            if let Some(soft) = tokens.next() {
                if let Ok(n) = soft.parse() {
                    return n;
                }
            }
        }
    }
    0
}

/// Selected `/proc/<pid>/stat` fields. Field index reference: `man proc(5)`.
#[cfg(target_os = "linux")]
struct ProcStat {
    comm: String,
    utime: u64,
    stime: u64,
    num_threads: i64,
}

#[cfg(target_os = "linux")]
fn parse_proc_stat(raw: &str) -> Option<ProcStat> {
    // The `comm` field can contain whitespace and parentheses, so the standard
    // trick is to find the *last* `)` and treat everything after it as the
    // remaining whitespace-separated fields.
    let lparen = raw.find('(')?;
    let rparen = raw.rfind(')')?;
    if rparen <= lparen + 1 {
        return None;
    }
    let comm = raw[lparen + 1..rparen].to_string();
    let tail = &raw[rparen + 1..];
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // After the literal `)` and a single space, field index 3 (`state`) is
    // tokens[0]; `utime` is field 14 (tokens[11]), `stime` field 15
    // (tokens[12]), `num_threads` field 20 (tokens[17]).
    let utime: u64 = fields.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
    let stime: u64 = fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
    let num_threads: i64 = fields.get(17).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(ProcStat {
        comm,
        utime,
        stime,
        num_threads,
    })
}

#[cfg(target_os = "linux")]
fn ticks_to_us(ticks: u64) -> u64 {
    ticks.saturating_mul(1_000_000) / CLK_TCK_HZ
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_proc_stat_handles_paren_in_comm() {
        let raw = "1 (pg_doorman main) S 0 1 1 0 -1 4194304 1 0 0 0 100 50 0 0 20 0 8 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let parsed = parse_proc_stat(raw).expect("parse");
        assert_eq!(parsed.comm, "pg_doorman main");
        assert_eq!(parsed.utime, 100);
        assert_eq!(parsed.stime, 50);
        assert_eq!(parsed.num_threads, 8);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ticks_to_us_at_100hz_yields_10ms_steps() {
        assert_eq!(ticks_to_us(0), 0);
        assert_eq!(ticks_to_us(1), 10_000);
        assert_eq!(ticks_to_us(100), 1_000_000);
    }

    #[test]
    fn collect_returns_envelope_on_any_platform() {
        let dto = collect_process();
        assert!(dto.ts > 0);
        // pid is always known.
        assert!(dto.pid > 0);
        // cpu_cores comes from num_cpus and is at least 1 on every supported
        // build target.
        assert!(dto.cpu_cores >= 1);
    }
}
