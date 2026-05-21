//! BDD steps for the binary-upgrade fd-leak coverage.
//!
//! Three operations:
//! 1. Discover the current pg_doorman PID from outside the process
//!    (parses `/proc/net/tcp` for the LISTEN inode on `doorman_port`,
//!    then maps that inode to a process via `/proc/<pid>/fd`).
//! 2. Capture an `FdInventory` snapshot for a stored PID and stash the
//!    socket / total / non-listener-socket counters in `world.vars`
//!    under a caller-named key. Two of those snapshots form the input
//!    to the delta assertion that catches the FD_CLOEXEC migration
//!    leak across chained SIGUSR2 upgrades.
//! 3. Universal FD_CLOEXEC assertion across every non-listener socket
//!    fd of a stored PID.
//!
//! All inspection runs from the test process. `pg_doorman` is never
//! asked to introspect its own fd table — the leak we are catching is
//! exactly the kind that masks `/proc/self/fd` reads under EMFILE.

use crate::proc_inspect::{find_pid_owning_listener, inventory, summary, FdInventory};
use crate::world::DoormanWorld;
use cucumber::{then, when};
use log::info;

/// Key used in `world.named_backend_pids` for PIDs discovered via the
/// external `/proc` walker. We piggy-back on the existing PID hashmap
/// instead of introducing a new field so existing PID-comparison steps
/// (`foreground pg_doorman PID should be different from stored ...`)
/// keep working unchanged against names we store here.
fn pid_slot(name: &str) -> (String, String) {
    (name.to_string(), "foreground_pid".to_string())
}

#[when(regex = r#"^we discover the current pg_doorman PID externally and store as "([^"]+)"$"#)]
pub async fn discover_pid_externally(world: &mut DoormanWorld, name: String) {
    let port = world
        .doorman_port
        .expect("doorman_port not set — pg_doorman must be started first");
    let pid = find_pid_owning_listener(port)
        .unwrap_or_else(|e| panic!("discover pid for port {port}: {e}"));
    info!(
        "[binary-upgrade-fd] discovered pid={pid} for listener on port {port} (storing as '{name}')"
    );
    world.named_backend_pids.insert(pid_slot(&name), pid as i32);
}

fn captured_pid(world: &DoormanWorld, name: &str) -> u32 {
    *world
        .named_backend_pids
        .get(&pid_slot(name))
        .unwrap_or_else(|| panic!("no PID stored under name '{name}' — call `discover the current pg_doorman PID externally and store as \"{name}\"` first"))
        as u32
}

fn store_inventory_counters(world: &mut DoormanWorld, key: &str, inv: &FdInventory) {
    let summary_line = summary(inv);
    info!("[binary-upgrade-fd] capture '{}': {}", key, summary_line);
    world
        .vars
        .insert(format!("{key}_summary"), summary_line.clone());
    world
        .vars
        .insert(format!("{key}_total"), inv.total_fds().to_string());
    world
        .vars
        .insert(format!("{key}_sockets"), inv.socket_fd_count().to_string());
    world.vars.insert(
        format!("{key}_non_listener_sockets"),
        inv.non_listener_socket_fds().len().to_string(),
    );
}

fn read_counter(world: &DoormanWorld, key: &str) -> usize {
    let raw = world.vars.get(key).unwrap_or_else(|| {
        panic!("no captured counter '{key}' in world.vars — capture an inventory first")
    });
    raw.parse::<usize>()
        .unwrap_or_else(|e| panic!("counter '{key}' = {raw:?} is not numeric: {e}"))
}

#[when(regex = r#"^we capture the fd inventory for stored PID "([^"]+)" as "([^"]+)"$"#)]
pub async fn capture_fd_inventory(world: &mut DoormanWorld, pid_name: String, key: String) {
    let pid = captured_pid(world, &pid_name);
    let port = world.doorman_port.expect("doorman_port not set");
    let inv =
        inventory(pid, port).unwrap_or_else(|e| panic!("inventory pid={pid} port={port}: {e}"));
    store_inventory_counters(world, &key, &inv);
}

#[then(
    regex = r#"^the non-listener socket fd count delta from "([^"]+)" to "([^"]+)" should be at most (\d+)$"#
)]
pub async fn non_listener_socket_delta_at_most(
    world: &mut DoormanWorld,
    before: String,
    after: String,
    slack: usize,
) {
    let before_n = read_counter(world, &format!("{before}_non_listener_sockets"));
    let after_n = read_counter(world, &format!("{after}_non_listener_sockets"));
    let summary_before = world
        .vars
        .get(&format!("{before}_summary"))
        .cloned()
        .unwrap_or_default();
    let summary_after = world
        .vars
        .get(&format!("{after}_summary"))
        .cloned()
        .unwrap_or_default();
    let delta = after_n as isize - before_n as isize;
    assert!(
        delta <= slack as isize,
        "non-listener socket fd count grew by {delta} (from {before_n} to {after_n}); allowed slack {slack}\n  before: {summary_before}\n  after:  {summary_after}"
    );
}

#[then(regex = r#"^the socket fd count for stored PID "([^"]+)" should be at most (\d+)$"#)]
pub async fn socket_count_at_most(world: &mut DoormanWorld, pid_name: String, max: usize) {
    let pid = captured_pid(world, &pid_name);
    let port = world.doorman_port.expect("doorman_port not set");
    let inv = inventory(pid, port).unwrap_or_else(|e| panic!("inventory pid={pid}: {e}"));
    let n = inv.socket_fd_count();
    assert!(
        n <= max,
        "pid={pid} has {n} socket fd(s); allowed at most {max}\n  {}",
        summary(&inv)
    );
}

/// Counts fds whose `readlink` target starts with `pipe:[`. Used by
/// the polluted-parent scenario: the test seeds the parent with N
/// pairs of pipe fds before exec, then asserts the count went down
/// across the binary-upgrade boundary because the child's cleanup
/// pass should have closed them.
fn pipe_fd_count(inv: &crate::proc_inspect::FdInventory) -> usize {
    inv.fds.iter().filter(|f| f.kind() == "pipe").count()
}

#[then(regex = r#"^the pipe fd count for stored PID "([^"]+)" should be at least (\d+)$"#)]
pub async fn pipe_count_at_least(world: &mut DoormanWorld, pid_name: String, min: usize) {
    let pid = captured_pid(world, &pid_name);
    let port = world.doorman_port.expect("doorman_port not set");
    let inv = inventory(pid, port).unwrap_or_else(|e| panic!("inventory pid={pid}: {e}"));
    let n = pipe_fd_count(&inv);
    assert!(
        n >= min,
        "pid={pid} has {n} pipe fd(s); expected at least {min}\n  {}",
        summary(&inv)
    );
}

#[then(regex = r#"^the pipe fd count for stored PID "([^"]+)" should be at most (\d+)$"#)]
pub async fn pipe_count_at_most(world: &mut DoormanWorld, pid_name: String, max: usize) {
    let pid = captured_pid(world, &pid_name);
    let port = world.doorman_port.expect("doorman_port not set");
    let inv = inventory(pid, port).unwrap_or_else(|e| panic!("inventory pid={pid}: {e}"));
    let n = pipe_fd_count(&inv);
    if n > max {
        let log_excerpt = world
            .doorman_log_path
            .as_ref()
            .map(|p| {
                std::fs::read_to_string(p)
                    .map(|s| {
                        let lines: Vec<&str> = s.lines().collect();
                        let tail: Vec<&str> = lines.iter().rev().take(60).rev().copied().collect();
                        tail.join("\n")
                    })
                    .unwrap_or_else(|e| format!("(failed to read log: {e})"))
            })
            .unwrap_or_else(|| "(no log capture path)".to_string());
        let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
            .map(|s| s.replace('\0', " ").trim().to_string())
            .unwrap_or_else(|e| format!("(no cmdline: {e})"));
        panic!(
            "pid={pid} has {n} pipe fd(s); allowed at most {max}\n  {}\n  cmdline: {cmdline}\n  -- doorman log tail (60 lines) --\n{log_excerpt}\n  -- end log --",
            summary(&inv)
        );
    }
}

#[then(regex = r#"^every non-listener socket fd of stored PID "([^"]+)" has FD_CLOEXEC set$"#)]
pub async fn every_non_listener_socket_is_cloexec(world: &mut DoormanWorld, pid_name: String) {
    let pid = captured_pid(world, &pid_name);
    let port = world.doorman_port.expect("doorman_port not set");
    let inv = inventory(pid, port).unwrap_or_else(|e| panic!("inventory pid={pid}: {e}"));
    let offenders = inv.non_listener_sockets_without_cloexec();
    assert!(
        offenders.is_empty(),
        "pid={pid}: {} non-listener socket fd(s) missing FD_CLOEXEC out of {} checked\n  {}\n{}",
        offenders.len(),
        inv.non_listener_socket_fds().len(),
        summary(&inv),
        inv.format_offender_lines(&offenders)
    );
}

#[then(regex = r#"^stored PID "([^"]+)" should be different from stored PID "([^"]+)"$"#)]
pub async fn stored_pids_differ(world: &mut DoormanWorld, a: String, b: String) {
    let pa = captured_pid(world, &a);
    let pb = captured_pid(world, &b);
    assert_ne!(
        pa, pb,
        "expected different PIDs but '{a}' and '{b}' both = {pa}"
    );
}

/// Targets the process at a name previously captured by
/// `we discover the current pg_doorman PID externally and store as "<NAME>"`.
/// Needed for chained binary upgrades: after the first `SIGUSR2`, the
/// original `world.doorman_process` handle no longer points at the
/// process that owns the listener, so the default
/// `we send SIGUSR2 to foreground pg_doorman` step (which uses that
/// handle) would deliver the signal to the dead parent.
#[when(regex = r#"^we send SIGUSR2 to pg_doorman process at stored PID "([^"]+)"$"#)]
pub async fn send_sigusr2_to_stored_pid(world: &mut DoormanWorld, name: String) {
    let pid = captured_pid(world, &name);
    info!(
        "[binary-upgrade-fd] sending SIGUSR2 to stored PID '{}' = {}",
        name, pid
    );
    // SAFETY: kill(2) with a non-zero pid signals that process by id.
    // We just looked the PID up from /proc and verified it owns the
    // doorman listener inode, so it cannot be a stale dead PID.
    let rc = unsafe { libc::kill(pid as i32, libc::SIGUSR2) };
    assert_eq!(
        rc,
        0,
        "kill(pid={pid}, SIGUSR2) failed: {}",
        std::io::Error::last_os_error()
    );
}
