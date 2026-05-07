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
use std::sync::LazyLock;

use crate::app::server::STARTED_AT;
use crate::web::metrics::system::get_process_memory_usage;
use crate::web::routes::dto::{
    CgroupMemoryDto, JemallocStatsDto, MemoryBreakdownDto, MemoryCategoryDto, ProcessDto,
    ProcessThreadDto,
};

use super::now_unix_ms;

#[cfg(target_os = "linux")]
const CLK_TCK_HZ: u64 = 100;

/// Process-lifetime constants — read them once on first request. The
/// alternative (read on every poll) costs an extra `/proc/self/limits`
/// read + libc::sysconf call + hostname syscall per /api/process tick;
/// at 1 Hz UI polling that is ~3 file reads/sec for values that cannot
/// change without re-exec.
static HOSTNAME: LazyLock<String> = LazyLock::new(read_hostname_uncached);
static CPU_CORES: LazyLock<u32> = LazyLock::new(|| num_cpus::get() as u32);
static PID: LazyLock<u32> = LazyLock::new(std::process::id);
#[cfg(target_os = "linux")]
static FD_LIMIT: LazyLock<u64> = LazyLock::new(read_linux_fd_limit_uncached);
static STARTED_AT_MS: LazyLock<u64> = LazyLock::new(|| {
    STARTED_AT
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
});

pub(crate) fn collect_process() -> ProcessDto {
    let pid = *PID;
    let hostname = HOSTNAME.clone();
    let uptime_seconds = STARTED_AT.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    let started_at_ms = *STARTED_AT_MS;

    let cpu_cores = *CPU_CORES;

    #[cfg(target_os = "linux")]
    let (cpu_user_us, cpu_system_us, threads, threads_breakdown) = read_linux_cpu();
    #[cfg(target_os = "linux")]
    let (vm_size_bytes, fd_open, fd_limit) =
        (read_linux_vm_size_bytes(), read_linux_fd_open(), *FD_LIMIT);

    #[cfg(not(target_os = "linux"))]
    let (cpu_user_us, cpu_system_us, threads, threads_breakdown): (
        u64,
        u64,
        u64,
        Vec<ProcessThreadDto>,
    ) = (0, 0, 0, Vec::new());
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

/// Build `/api/process/memory`. Reads `/proc/self/status`, the cgroup files,
/// jemalloc stats, and the in-process interner totals; assembles a
/// breakdown the operator can read top-down to find a leak. PSS-based
/// figures (`/proc/self/smaps_rollup`) are deliberately not collected
/// here — the kernel walks every VMA, ~100 µs+ per request, and the
/// 5 s panel cadence does not need that resolution.
pub(crate) fn collect_memory_breakdown() -> MemoryBreakdownDto {
    let rss_bytes = get_process_memory_usage();

    #[cfg(target_os = "linux")]
    let status = read_linux_status_block();
    #[cfg(not(target_os = "linux"))]
    let status = StatusBlock::default();

    #[cfg(target_os = "linux")]
    let cgroup = read_cgroup_memory();
    #[cfg(not(target_os = "linux"))]
    let cgroup = None;

    let jemalloc = read_jemalloc_stats();

    // Interner totals come from the global cache. The collector at
    // `crate::stats::interner` provides a snapshot of named/anonymous
    // bytes; we duplicate the path the /api/interner endpoint uses so
    // the two views always agree.
    let (interner_named_bytes, interner_anonymous_bytes) = {
        let int = crate::web::routes::collect::collect_interner();
        (int.named.bytes, int.anonymous.bytes)
    };

    let categories = build_categories(
        rss_bytes,
        status.rss_anon_bytes,
        status.rss_file_bytes,
        status.vm_stack_bytes,
        status.vm_pte_bytes,
        status.vm_swap_bytes,
        interner_named_bytes,
        interner_anonymous_bytes,
        jemalloc.as_ref(),
    );

    MemoryBreakdownDto {
        ts: now_unix_ms(),
        rss_bytes,
        vm_peak_bytes: status.vm_peak_bytes,
        vm_hwm_bytes: status.vm_hwm_bytes,
        vm_data_bytes: status.vm_data_bytes,
        vm_stack_bytes: status.vm_stack_bytes,
        vm_exe_bytes: status.vm_exe_bytes,
        vm_lib_bytes: status.vm_lib_bytes,
        vm_pte_bytes: status.vm_pte_bytes,
        vm_swap_bytes: status.vm_swap_bytes,
        rss_anon_bytes: status.rss_anon_bytes,
        rss_file_bytes: status.rss_file_bytes,
        rss_shmem_bytes: status.rss_shmem_bytes,
        jemalloc,
        cgroup,
        interner_named_bytes,
        interner_anonymous_bytes,
        categories,
    }
}

#[derive(Default)]
struct StatusBlock {
    vm_peak_bytes: Option<u64>,
    vm_hwm_bytes: Option<u64>,
    vm_data_bytes: Option<u64>,
    vm_stack_bytes: Option<u64>,
    vm_exe_bytes: Option<u64>,
    vm_lib_bytes: Option<u64>,
    vm_pte_bytes: Option<u64>,
    vm_swap_bytes: Option<u64>,
    rss_anon_bytes: Option<u64>,
    rss_file_bytes: Option<u64>,
    rss_shmem_bytes: Option<u64>,
}

#[cfg(target_os = "linux")]
fn read_linux_status_block() -> StatusBlock {
    let mut sb = StatusBlock::default();
    let Ok(raw) = std::fs::read_to_string("/proc/self/status") else {
        return sb;
    };
    let kib_after = |prefix: &str, line: &str| -> Option<u64> {
        line.strip_prefix(prefix)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    for line in raw.lines() {
        if let Some(b) = kib_after("VmPeak:", line) {
            sb.vm_peak_bytes = Some(b);
        } else if let Some(b) = kib_after("VmHWM:", line) {
            sb.vm_hwm_bytes = Some(b);
        } else if let Some(b) = kib_after("VmData:", line) {
            sb.vm_data_bytes = Some(b);
        } else if let Some(b) = kib_after("VmStk:", line) {
            sb.vm_stack_bytes = Some(b);
        } else if let Some(b) = kib_after("VmExe:", line) {
            sb.vm_exe_bytes = Some(b);
        } else if let Some(b) = kib_after("VmLib:", line) {
            sb.vm_lib_bytes = Some(b);
        } else if let Some(b) = kib_after("VmPTE:", line) {
            sb.vm_pte_bytes = Some(b);
        } else if let Some(b) = kib_after("VmSwap:", line) {
            sb.vm_swap_bytes = Some(b);
        } else if let Some(b) = kib_after("RssAnon:", line) {
            sb.rss_anon_bytes = Some(b);
        } else if let Some(b) = kib_after("RssFile:", line) {
            sb.rss_file_bytes = Some(b);
        } else if let Some(b) = kib_after("RssShmem:", line) {
            sb.rss_shmem_bytes = Some(b);
        }
    }
    sb
}

/// Detect cgroup v2 first, fall back to v1. Container deployments mount
/// the namespace at `/sys/fs/cgroup/...` directly so the path lookup is
/// usually trivial; the host case (operator running pg_doorman bare)
/// reads `/proc/self/cgroup` to find the right subdirectory.
#[cfg(target_os = "linux")]
fn read_cgroup_memory() -> Option<CgroupMemoryDto> {
    let proc_cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let first = proc_cgroup.lines().next()?;
    if first.starts_with("0::") {
        // cgroup v2 unified.
        let suffix = first.trim_start_matches("0::").trim_start_matches('/');
        let base = if suffix.is_empty() {
            "/sys/fs/cgroup".to_string()
        } else {
            format!("/sys/fs/cgroup/{suffix}")
        };
        let current = read_first_u64(&format!("{base}/memory.current"))?;
        let max = read_first_u64_or_max(&format!("{base}/memory.max"));
        let high = read_first_u64_or_max(&format!("{base}/memory.high"));
        let peak = read_first_u64(&format!("{base}/memory.peak"));
        return Some(CgroupMemoryDto {
            version: 2,
            current_bytes: current,
            peak_bytes: peak,
            max_bytes: max,
            high_bytes: high,
        });
    }
    // cgroup v1 — find the `memory` controller line.
    for line in proc_cgroup.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() != 3 {
            continue;
        }
        let controllers = parts[1];
        if !controllers.split(',').any(|c| c == "memory") {
            continue;
        }
        let suffix = parts[2].trim_start_matches('/');
        let base = if suffix.is_empty() {
            "/sys/fs/cgroup/memory".to_string()
        } else {
            format!("/sys/fs/cgroup/memory/{suffix}")
        };
        let current = read_first_u64(&format!("{base}/memory.usage_in_bytes"))?;
        let max =
            read_first_u64(&format!("{base}/memory.limit_in_bytes")).filter(|&n| n < u64::MAX / 2);
        return Some(CgroupMemoryDto {
            version: 1,
            current_bytes: current,
            peak_bytes: read_first_u64(&format!("{base}/memory.max_usage_in_bytes")),
            max_bytes: max,
            high_bytes: None,
        });
    }
    None
}

#[cfg(target_os = "linux")]
fn read_first_u64(path: &str) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(target_os = "linux")]
fn read_first_u64_or_max(path: &str) -> Option<u64> {
    let s = std::fs::read_to_string(path).ok()?;
    let s = s.trim();
    if s == "max" {
        return None;
    }
    s.parse::<u64>().ok()
}

/// jemalloc accounting. The crate is linked unconditionally (see Cargo.toml),
/// so on every supported build target this returns `Some`. The `Option`
/// guards against `epoch::advance` failing on a future jemalloc release that
/// changes the mib layout — preserves a graceful fallback to "no jemalloc
/// data" without panicking.
fn read_jemalloc_stats() -> Option<JemallocStatsDto> {
    use tikv_jemalloc_ctl::{epoch, stats};
    // Advance the epoch so per-arena counters merge into the read paths
    // below. Without this `stats.allocated` lags by up to 10 seconds
    // under low traffic.
    epoch::advance().ok()?;
    let allocated = stats::allocated::read().ok()? as u64;
    let active = stats::active::read().ok()? as u64;
    let resident = stats::resident::read().ok()? as u64;
    let mapped = stats::mapped::read().ok()? as u64;
    let retained = stats::retained::read().ok()? as u64;
    let metadata = stats::metadata::read().ok()? as u64;
    Some(JemallocStatsDto {
        allocated_bytes: allocated,
        active_bytes: active,
        resident_bytes: resident,
        mapped_bytes: mapped,
        retained_bytes: retained,
        metadata_bytes: metadata,
        fragmentation_bytes: resident.saturating_sub(allocated),
    })
}

#[allow(clippy::too_many_arguments)]
fn build_categories(
    rss_bytes: u64,
    rss_anon_bytes: Option<u64>,
    rss_file_bytes: Option<u64>,
    vm_stack_bytes: Option<u64>,
    vm_pte_bytes: Option<u64>,
    vm_swap_bytes: Option<u64>,
    interner_named_bytes: u64,
    interner_anonymous_bytes: u64,
    jemalloc: Option<&JemallocStatsDto>,
) -> Vec<MemoryCategoryDto> {
    let app_caches = interner_named_bytes + interner_anonymous_bytes;
    let mut cats: Vec<MemoryCategoryDto> = Vec::new();

    cats.push(MemoryCategoryDto {
        key: "app_caches",
        label: "Internal caches",
        bytes: app_caches,
        explain: "SQL interner (named + anonymous) — pg_doorman-side state we own.",
    });

    if let Some(j) = jemalloc {
        // Live = jemalloc.allocated minus the chunk we already attribute
        // to internal caches. Floor at zero — under high churn `allocated`
        // can briefly read below the cache estimate.
        let live = j.allocated_bytes.saturating_sub(app_caches);
        cats.push(MemoryCategoryDto {
            key: "jemalloc_live",
            label: "Live allocations",
            bytes: live,
            explain: "jemalloc.allocated minus tracked caches — Rust heap not in our maps yet.",
        });
        cats.push(MemoryCategoryDto {
            key: "jemalloc_fragmentation",
            label: "Allocator fragmentation",
            bytes: j.fragmentation_bytes,
            explain:
                "Pages jemalloc holds but is not currently using; reclaimable via arena.purge.",
        });
    }

    if let Some(rf) = rss_file_bytes {
        cats.push(MemoryCategoryDto {
            key: "code_and_libs",
            label: "Code + shared libs",
            bytes: rf,
            explain: "Resident pages backing the binary and shared objects. Static; growth = dlopen leak.",
        });
    }

    let stack_pte = vm_stack_bytes.unwrap_or(0) + vm_pte_bytes.unwrap_or(0);
    if stack_pte > 0 {
        cats.push(MemoryCategoryDto {
            key: "stacks_and_pagetables",
            label: "Stacks + page tables",
            bytes: stack_pte,
            explain: "Per-thread stacks plus kernel page-table overhead. Grows with thread count.",
        });
    }

    if let Some(sw) = vm_swap_bytes {
        if sw > 0 {
            cats.push(MemoryCategoryDto {
                key: "swap",
                label: "Swapped out",
                bytes: sw,
                explain: "Pages swapped to disk. Non-zero on a pooler is a red flag.",
            });
        }
    }

    // Anything in RSS that we did not attribute. Defends against operators
    // expecting the bar to add up to RSS exactly.
    let attributed: u64 = cats.iter().map(|c| c.bytes).sum();
    let remainder = rss_bytes.saturating_sub(attributed.min(rss_bytes));
    if remainder > 0 && rss_anon_bytes.is_some() {
        cats.push(MemoryCategoryDto {
            key: "other",
            label: "Other (anonymous)",
            bytes: remainder,
            explain: "Anonymous pages not yet attributed to a known bucket.",
        });
    }

    cats
}

fn read_hostname_uncached() -> String {
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
            let Some(parts) = parse_proc_stat(&s) else {
                continue;
            };
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
fn read_linux_fd_limit_uncached() -> u64 {
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

    fn jemalloc_with(allocated: u64, fragmentation: u64) -> JemallocStatsDto {
        JemallocStatsDto {
            allocated_bytes: allocated,
            active_bytes: 0,
            resident_bytes: allocated + fragmentation,
            mapped_bytes: 0,
            retained_bytes: 0,
            metadata_bytes: 0,
            fragmentation_bytes: fragmentation,
        }
    }

    fn cat<'a>(cats: &'a [MemoryCategoryDto], key: &str) -> Option<&'a MemoryCategoryDto> {
        cats.iter().find(|c| c.key == key)
    }

    #[test]
    fn build_categories_empty_inputs_yields_only_caches_zero() {
        let cats = build_categories(0, None, None, None, None, None, 0, 0, None);
        // app_caches is unconditional, even at zero — it anchors the bar.
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].key, "app_caches");
        assert_eq!(cats[0].bytes, 0);
    }

    #[test]
    fn build_categories_jemalloc_live_floors_at_zero_when_caches_exceed_allocated() {
        // Cache estimate larger than jemalloc.allocated can briefly happen
        // under churn (counters update at different epochs). Live must not
        // underflow.
        let j = jemalloc_with(100, 50);
        let cats = build_categories(0, None, None, None, None, None, 1_000, 0, Some(&j));
        assert_eq!(cat(&cats, "jemalloc_live").unwrap().bytes, 0);
        // Fragmentation is still surfaced as-is.
        assert_eq!(cat(&cats, "jemalloc_fragmentation").unwrap().bytes, 50);
    }

    #[test]
    fn build_categories_attributed_over_rss_does_not_underflow_or_emit_other() {
        // RSS smaller than the sum of attributed buckets — `other` must be
        // suppressed and no panic.
        let j = jemalloc_with(10_000, 2_000);
        let cats = build_categories(
            1_000,       // rss < attributed
            Some(500),   // rss_anon set, but remainder will be 0
            Some(5_000), // code_and_libs
            Some(1_024), // stacks
            Some(1_024), // pte
            None,        // no swap
            500,         // named cache
            500,         // anon cache
            Some(&j),
        );
        assert!(cat(&cats, "other").is_none());
    }

    #[test]
    fn build_categories_swap_zero_is_hidden() {
        let cats = build_categories(0, None, None, None, None, Some(0), 0, 0, None);
        assert!(cat(&cats, "swap").is_none());
    }

    #[test]
    fn build_categories_swap_none_is_hidden() {
        let cats = build_categories(0, None, None, None, None, None, 0, 0, None);
        assert!(cat(&cats, "swap").is_none());
    }

    #[test]
    fn build_categories_stacks_pte_both_none_omits_row() {
        let cats = build_categories(0, None, None, None, None, None, 0, 0, None);
        assert!(cat(&cats, "stacks_and_pagetables").is_none());
    }

    #[test]
    fn build_categories_other_only_appears_when_rss_anon_is_known() {
        // Remainder is positive (RSS=1 GiB, nothing attributed) but rss_anon
        // is None → cannot honestly attribute the slack → suppress `other`.
        let cats_no_anon = build_categories(
            1_073_741_824, // 1 GiB
            None,
            None,
            None,
            None,
            None,
            0,
            0,
            None,
        );
        assert!(cat(&cats_no_anon, "other").is_none());

        // Same RSS, rss_anon known → `other` shows the slack.
        let cats_with_anon = build_categories(
            1_073_741_824,
            Some(500_000_000),
            None,
            None,
            None,
            None,
            0,
            0,
            None,
        );
        let other = cat(&cats_with_anon, "other").expect("other expected");
        assert_eq!(other.bytes, 1_073_741_824);
    }

    #[test]
    fn build_categories_swap_positive_is_surfaced() {
        let cats = build_categories(0, None, None, None, None, Some(1_024 * 1_024), 0, 0, None);
        assert_eq!(cat(&cats, "swap").unwrap().bytes, 1_024 * 1_024);
    }
}
