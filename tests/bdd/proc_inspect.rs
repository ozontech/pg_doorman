//! External `/proc` inspection for BDD fd-leak checks.
//! The pooler must not read `/proc/self/fd` while the test is creating
//! fd pressure.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::BTreeMap;

/// One open fd of an inspected process, as seen from outside.
#[derive(Debug, Clone)]
pub struct FdRecord {
    pub fd: i32,
    /// Raw `readlink /proc/<pid>/fd/<fd>` value. Examples: `socket:[123]`,
    /// `pipe:[456]`, `anon_inode:[eventpoll]`, `/dev/null`, `/path/to/log`.
    pub target: String,
    /// For socket/pipe/anon_inode targets, the bracketed inode if it
    /// parsed cleanly. None for regular files.
    pub inode: Option<u64>,
    /// FD_CLOEXEC bit from `/proc/<pid>/fdinfo/<fd>` `flags:` line.
    /// `None` when fdinfo could not be read (race against fd close,
    /// permission denied, or non-Linux build).
    pub cloexec: Option<bool>,
}

impl FdRecord {
    pub fn is_socket(&self) -> bool {
        self.target.starts_with("socket:[")
    }

    pub fn kind(&self) -> &'static str {
        if self.target.starts_with("socket:[") {
            "socket"
        } else if self.target.starts_with("pipe:[") {
            "pipe"
        } else if self.target.starts_with("anon_inode:[") {
            "anon_inode"
        } else if self.target.starts_with("/dev/") {
            "device"
        } else if self.target.starts_with('/') {
            "file"
        } else {
            "other"
        }
    }
}

/// External fd snapshot plus listener inode set.
#[derive(Debug, Clone)]
pub struct FdInventory {
    pub pid: u32,
    pub fds: Vec<FdRecord>,
    /// Listener socket inodes for the inspected port.
    pub listener_inodes: std::collections::BTreeSet<u64>,
}

impl FdInventory {
    pub fn total_fds(&self) -> usize {
        self.fds.len()
    }

    pub fn socket_fd_count(&self) -> usize {
        self.fds.iter().filter(|f| f.is_socket()).count()
    }

    /// Socket fds whose inode is not in the listener set.
    pub fn non_listener_socket_fds(&self) -> Vec<&FdRecord> {
        self.fds
            .iter()
            .filter(|f| {
                if !f.is_socket() {
                    return false;
                }
                match f.inode {
                    Some(ino) => !self.listener_inodes.contains(&ino),
                    None => true,
                }
            })
            .collect()
    }

    /// Non-listener socket fds without proven FD_CLOEXEC.
    /// `None` is an offender; the BDD step retries before failing.
    pub fn non_listener_sockets_without_cloexec(&self) -> Vec<&FdRecord> {
        self.non_listener_socket_fds()
            .into_iter()
            .filter(|f| !matches!(f.cloexec, Some(true)))
            .collect()
    }

    /// Bounded offender table for panic messages.
    pub fn format_offender_lines(&self, offenders: &[&FdRecord]) -> String {
        let mut out = String::new();
        out.push_str("  fd      kind        inode      cloexec  target\n");
        for f in offenders.iter().take(40) {
            let ino = f.inode.map(|n| n.to_string()).unwrap_or_default();
            let cx = match f.cloexec {
                Some(true) => "yes",
                Some(false) => "no",
                None => "?",
            };
            out.push_str(&format!(
                "  {:<7} {:<11} {:<10} {:<8} {}\n",
                f.fd,
                f.kind(),
                ino,
                cx,
                f.target
            ));
        }
        if offenders.len() > 40 {
            out.push_str(&format!("  ... ({} more)\n", offenders.len() - 40));
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::FdRecord;
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    /// Parse bracketed inode from `socket:[12345]`-style targets.
    pub(super) fn parse_inode_in_brackets(s: &str) -> Option<u64> {
        let lb = s.find('[')?;
        let rb = s.find(']')?;
        if rb <= lb + 1 {
            return None;
        }
        s[lb + 1..rb].parse::<u64>().ok()
    }

    /// Read `O_CLOEXEC` from `/proc/<pid>/fdinfo/<fd>` flags.
    pub(super) fn read_cloexec(pid: u32, fd: i32) -> Option<bool> {
        let body = fs::read_to_string(format!("/proc/{pid}/fdinfo/{fd}")).ok()?;
        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("flags:") {
                let flags_str = rest.trim();
                let flags = u64::from_str_radix(flags_str, 8).ok()?;
                return Some(flags & 0o2_000_000 != 0);
            }
        }
        None
    }

    /// Parse `/proc/net/tcp` local address and return only the port.
    pub(super) fn parse_local_port_hex(local: &str) -> Option<u16> {
        let mut parts = local.split(':');
        let _ip = parts.next()?;
        let port_hex = parts.next()?;
        u16::from_str_radix(port_hex, 16).ok()
    }

    /// LISTEN socket inode set for `port` in the target process netns.
    pub(super) fn listening_inodes_for_port(pid: u32, port: u16) -> BTreeSet<u64> {
        let mut out = BTreeSet::new();
        for file in [
            format!("/proc/{pid}/net/tcp"),
            format!("/proc/{pid}/net/tcp6"),
        ] {
            let Ok(body) = fs::read_to_string(&file) else {
                continue;
            };
            for (i, line) in body.lines().enumerate() {
                if i == 0 {
                    continue; // header
                }
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() < 10 {
                    continue;
                }
                // 0=sl 1=local_address 2=remote_address 3=st 4=tx_queue ... 9=inode
                let st = fields[3];
                if st != "0A" {
                    continue; // not LISTEN
                }
                let Some(parsed_port) = parse_local_port_hex(fields[1]) else {
                    continue;
                };
                if parsed_port != port {
                    continue;
                }
                if let Ok(ino) = fields[9].parse::<u64>() {
                    out.insert(ino);
                }
            }
        }
        out
    }

    /// Snapshot `/proc/<pid>/fd`; skip entries that close mid-walk.
    pub(super) fn inventory_fds(pid: u32) -> Result<Vec<FdRecord>, String> {
        let dir = format!("/proc/{pid}/fd");
        let entries = fs::read_dir(&dir).map_err(|e| format!("read_dir {dir}: {e}"))?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let fd_str = entry.file_name();
            let Some(fd_str) = fd_str.to_str() else {
                continue;
            };
            let Ok(fd) = fd_str.parse::<i32>() else {
                continue;
            };
            let path = entry.path();
            let target = match fs::read_link(&path) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => continue,
            };
            let inode = parse_inode_in_brackets(&target);
            let cloexec = read_cloexec(pid, fd);
            out.push(FdRecord {
                fd,
                target,
                inode,
                cloexec,
            });
        }
        Ok(out)
    }

    /// Find the process that owns the listener on `port`.
    /// During upgrade overlap, prefer a process started with `--inherit-fd`.
    pub(super) fn find_pid_owning_listener(port: u16) -> Result<u32, String> {
        let proc_dir = fs::read_dir("/proc").map_err(|e| format!("read /proc: {e}"))?;
        let mut last_err: Option<String> = None;
        let mut candidates: Vec<u32> = Vec::new();
        for entry in proc_dir.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };

            let inodes = listening_inodes_for_port(pid, port);
            if inodes.is_empty() {
                continue;
            }

            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.uid() != current_uid() {
                // Same-netns tcp rows are visible across users, but fd
                // links are not. Keep the diagnostic if no candidate matches.
                last_err = Some(format!(
                    "pid {pid} owns LISTEN port {port} but its /proc/<pid> belongs to a different user"
                ));
                continue;
            }

            let fds = match inventory_fds(pid) {
                Ok(v) => v,
                Err(e) => {
                    last_err = Some(format!("inventory_fds({pid}): {e}"));
                    continue;
                }
            };
            let mut matched = false;
            for record in &fds {
                let Some(ino) = record.inode else { continue };
                if inodes.contains(&ino) {
                    matched = true;
                    break;
                }
            }
            if matched {
                candidates.push(pid);
            }
        }

        if candidates.is_empty() {
            return Err(last_err.unwrap_or_else(|| {
                format!("no process owns a LISTEN socket on port {port} in /proc")
            }));
        }

        let upgrade_child = candidates
            .iter()
            .copied()
            .find(|pid| cmdline_contains(*pid, "--inherit-fd"));
        if let Some(pid) = upgrade_child {
            return Ok(pid);
        }

        candidates.sort_unstable();
        Ok(*candidates.last().expect("non-empty after early return"))
    }

    /// Best-effort cmdline substring match; read failures mean "no match".
    pub(super) fn cmdline_contains(pid: u32, needle: &str) -> bool {
        let Ok(bytes) = fs::read(format!("/proc/{pid}/cmdline")) else {
            return false;
        };
        let joined: String = bytes
            .iter()
            .map(|&b| if b == 0 { ' ' } else { b as char })
            .collect();
        joined.contains(needle)
    }

    pub(super) fn current_uid() -> u32 {
        // SAFETY: getuid is async-signal-safe, takes no arguments,
        // returns the real uid of the calling process.
        unsafe { libc::getuid() }
    }
}

#[cfg(target_os = "linux")]
pub fn find_pid_owning_listener(port: u16) -> Result<u32, String> {
    linux::find_pid_owning_listener(port)
}

#[cfg(target_os = "linux")]
pub fn inventory(pid: u32, listener_port: u16) -> Result<FdInventory, String> {
    let fds = linux::inventory_fds(pid)?;
    let listener_inodes = linux::listening_inodes_for_port(pid, listener_port);
    Ok(FdInventory {
        pid,
        fds,
        listener_inodes,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn find_pid_owning_listener(_port: u16) -> Result<u32, String> {
    Err("proc_inspect requires Linux".to_string())
}

#[cfg(not(target_os = "linux"))]
pub fn inventory(_pid: u32, _listener_port: u16) -> Result<FdInventory, String> {
    Err("proc_inspect requires Linux".to_string())
}

/// One-line inventory summary plus per-kind counts.
pub fn summary(inv: &FdInventory) -> String {
    let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    for f in &inv.fds {
        *by_kind.entry(f.kind()).or_insert(0) += 1;
    }
    let breakdown = by_kind
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "pid={} total_fds={} listener_inodes={} | {}",
        inv.pid,
        inv.total_fds(),
        inv.listener_inodes.len(),
        breakdown
    )
}

// The BDD binary uses `harness = false`; parser unit tests need a separate
// test target if this helper grows more parsing logic.
