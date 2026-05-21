use arc_swap::ArcSwapOption;
use libc::{c_int, mode_t, stat};
#[cfg(debug_assertions)]
use log::debug;
use log::warn;
use std::collections::HashSet;
use std::ffi::CStr;
use std::fmt::{Debug, Display, Formatter};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::mem::MaybeUninit;
#[cfg(debug_assertions)]
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::{fs, mem, ptr, slice};

#[derive(Debug)]
pub enum SocketInfoErr {
    Io(std::io::Error),
    Nix(nix::errno::Errno),
    Convert(std::num::TryFromIntError),
}

const FD_DIR: &str = "fd";
const INODE_STR: &str = "socket:[";
// /proc/<pid>/fd/<fd_num> - <pid> and <fd_num> max size is 20, total should be 20 + 20 + 10 < 64
const PATH_BUF_SIZE: usize = 64;

#[cfg(debug_assertions)]
enum SocketAddr {
    V4(SocketAddrV4),
    V6(SocketAddrV6),
}

#[derive(Default)]
pub struct TcpStateCount {
    // https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/net/tcp_states.h
    pub established: u16,
    pub syn_sent: u16,
    pub syn_recv: u16,
    pub fin_wait1: u16,
    pub fin_wait2: u16,
    pub time_wait: u16,
    pub close: u16,
    pub close_wait: u16,
    pub last_ack: u16,
    pub listen: u16,
    pub closing: u16,
    pub new_syn_recv: u16,
    pub bound_inactive: u16,

    pub total_count: u32,
}

#[derive(Default)]
pub struct UnixStreamStateCount {
    // https://github.com/ecki/net-tools/blob/master/netstat.c#L121
    pub free: u16,          /* not allocated                */
    pub unconnected: u16,   /* unconnected to any socket    */
    pub connecting: u16,    /* in process of connecting     */
    pub connected: u16,     /* connected to socket          */
    pub disconnecting: u16, /* in process of disconnecting  */

    pub total_count: u32,
}

#[derive(Default)]
pub struct SocketStateCount {
    pub tcp: TcpStateCount,
    pub tcp6: TcpStateCount,
    pub unix_stream: UnixStreamStateCount,
    pub unix_dgram: u16,
    pub unix_seq_packet: u16,
    pub unknown: u16,
}

impl SocketStateCount {
    pub fn to_vector(&self) -> Vec<String> {
        let mut res = self.tcp.to_vector();
        res.extend(self.tcp6.to_vector());
        res.extend(self.unix_stream.to_vector());
        res.extend(vec![
            self.unix_dgram.to_string(),
            self.unix_seq_packet.to_string(),
            self.unknown.to_string(),
        ]);
        res
    }
    pub fn get_tcp(&self) -> u32 {
        self.tcp.get_total()
    }
    pub fn get_tcp6(&self) -> u32 {
        self.tcp6.get_total()
    }
    pub fn get_unix(&self) -> u32 {
        self.unix_stream.get_total()
    }

    pub fn get_unknown(&self) -> u32 {
        self.unknown as u32
    }
}

impl TcpStateCount {
    fn get_total(&self) -> u32 {
        self.total_count
    }
    fn increase_count(&mut self, conn_type: u8) {
        match conn_type {
            1 => self.established += 1,
            2 => self.syn_sent += 1,
            3 => self.syn_recv += 1,
            4 => self.fin_wait1 += 1,
            5 => self.fin_wait2 += 1,
            6 => self.time_wait += 1,
            7 => self.close += 1,
            8 => self.close_wait += 1,
            9 => self.last_ack += 1,
            10 => self.listen += 1,
            11 => self.closing += 1,
            12 => self.new_syn_recv += 1,
            13 => self.bound_inactive += 1,
            _ => return,
        }
        self.total_count += 1
    }
    pub fn to_vector(&self) -> Vec<String> {
        vec![
            self.established.to_string(),
            self.syn_sent.to_string(),
            self.syn_recv.to_string(),
            self.fin_wait1.to_string(),
            self.fin_wait2.to_string(),
            self.time_wait.to_string(),
            self.close.to_string(),
            self.close_wait.to_string(),
            self.last_ack.to_string(),
            self.listen.to_string(),
            self.closing.to_string(),
            self.new_syn_recv.to_string(),
            self.bound_inactive.to_string(),
        ]
    }
}

impl UnixStreamStateCount {
    fn get_total(&self) -> u32 {
        self.total_count
    }
    fn increase_count(&mut self, conn_type: u8) {
        match conn_type {
            1 => self.unconnected += 1,
            2 => self.connecting += 1,
            3 => self.connected += 1,
            4 => self.disconnecting += 1,
            _ => self.free += 1,
        }
        self.total_count += 1
    }
    pub fn to_vector(&self) -> Vec<String> {
        vec![
            self.free.to_string(),
            self.unconnected.to_string(),
            self.connecting.to_string(),
            self.connected.to_string(),
            self.disconnecting.to_string(),
        ]
    }
}
#[cfg(debug_assertions)]
impl Display for SocketAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketAddr::V4(socket) => {
                f.write_fmt(format_args!("{}:{}", socket.ip(), socket.port()))
            }
            SocketAddr::V6(socket) => {
                f.write_fmt(format_args!("{}:{}", socket.ip(), socket.port()))
            }
        }
    }
}

impl Display for SocketInfoErr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketInfoErr::Io(io_error) => write!(f, "{io_error}"),
            SocketInfoErr::Nix(n_error) => write!(f, "{n_error}"),
            SocketInfoErr::Convert(int_error) => write!(f, "{int_error}"),
        }
    }
}

/// TCP state breakdown with IPv4 and IPv6 counters merged per state.
///
/// Kernel-level limits like `net.ipv4.tcp_max_tw_buckets` are process-wide, so
/// a DBA reacting to a TIME_WAIT storm or a CLOSE_WAIT leak wants a single
/// number per state rather than one per address family. The per-family totals
/// still appear separately in the log line so a dual-stack misconfig stays
/// visible — see `Display for SocketStateCount`.
#[derive(Debug, Default, PartialEq, Eq)]
struct MergedTcpBreakdown {
    established: u32,
    listen: u32,
    time_wait: u32,
    fin_wait1: u32,
    fin_wait2: u32,
    close_wait: u32,
    last_ack: u32,
    syn_sent: u32,
    syn_recv: u32,
    new_syn_recv: u32,
    closing: u32,
    close: u32,
    bound_inactive: u32,
}

impl SocketStateCount {
    /// Sum each TCP state across the `tcp` (IPv4) and `tcp6` counters.
    /// Widening to `u32` avoids any risk of overflow on a process with more
    /// than 65_535 sockets in a single state.
    fn merged_tcp_breakdown(&self) -> MergedTcpBreakdown {
        let t4 = &self.tcp;
        let t6 = &self.tcp6;
        MergedTcpBreakdown {
            established: t4.established as u32 + t6.established as u32,
            listen: t4.listen as u32 + t6.listen as u32,
            time_wait: t4.time_wait as u32 + t6.time_wait as u32,
            fin_wait1: t4.fin_wait1 as u32 + t6.fin_wait1 as u32,
            fin_wait2: t4.fin_wait2 as u32 + t6.fin_wait2 as u32,
            close_wait: t4.close_wait as u32 + t6.close_wait as u32,
            last_ack: t4.last_ack as u32 + t6.last_ack as u32,
            syn_sent: t4.syn_sent as u32 + t6.syn_sent as u32,
            syn_recv: t4.syn_recv as u32 + t6.syn_recv as u32,
            new_syn_recv: t4.new_syn_recv as u32 + t6.new_syn_recv as u32,
            closing: t4.closing as u32 + t6.closing as u32,
            close: t4.close as u32 + t6.close as u32,
            bound_inactive: t4.bound_inactive as u32 + t6.bound_inactive as u32,
        }
    }
}

impl Display for SocketStateCount {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Layout follows the pool-stats line convention: `[prefix] key=value ...
        // | group2 ... | group3 ...`. Zero values are always printed so that
        // field positions stay stable for `awk`/Loki parsing — a listen socket
        // dropping to zero must be observable as `tcp_lstn=0`, not as an
        // absent field.
        let tcp4 = self.tcp.total_count;
        let tcp6 = self.tcp6.total_count;
        let tcp_total = tcp4 + tcp6;
        let tcp = self.merged_tcp_breakdown();
        let u = &self.unix_stream;

        write!(f, "[sockets] tcp={tcp_total} tcp4={tcp4} tcp6={tcp6}")?;
        write!(
            f,
            " | tcp_est={est} tcp_lstn={lstn} tcp_tw={tw} tcp_fw1={fw1} tcp_fw2={fw2} \
             tcp_cw={cw} tcp_la={la} tcp_syns={syns} tcp_synr={synr} tcp_nsr={nsr} \
             tcp_clsg={clsg} tcp_cls={cls} tcp_bnd={bnd}",
            est = tcp.established,
            lstn = tcp.listen,
            tw = tcp.time_wait,
            fw1 = tcp.fin_wait1,
            fw2 = tcp.fin_wait2,
            cw = tcp.close_wait,
            la = tcp.last_ack,
            syns = tcp.syn_sent,
            synr = tcp.syn_recv,
            nsr = tcp.new_syn_recv,
            clsg = tcp.closing,
            cls = tcp.close,
            bnd = tcp.bound_inactive,
        )?;
        write!(
            f,
            " | unix={unix} unix_conn={conn} unix_uncn={uncn} unix_cng={cng} \
             unix_dcn={dcn} unix_free={free}",
            unix = u.total_count,
            conn = u.connected,
            uncn = u.unconnected,
            cng = u.connecting,
            dcn = u.disconnecting,
            free = u.free,
        )?;
        write!(
            f,
            " | dgram={dgram} seqpkt={seqpkt} unknown={unknown}",
            dgram = self.unix_dgram,
            seqpkt = self.unix_seq_packet,
            unknown = self.unknown,
        )
    }
}

impl From<nix::errno::Errno> for SocketInfoErr {
    fn from(err: nix::errno::Errno) -> Self {
        SocketInfoErr::Nix(err)
    }
}
impl From<std::io::Error> for SocketInfoErr {
    fn from(err: std::io::Error) -> Self {
        SocketInfoErr::Io(err)
    }
}

impl From<std::num::TryFromIntError> for SocketInfoErr {
    fn from(err: std::num::TryFromIntError) -> Self {
        SocketInfoErr::Convert(err)
    }
}

/// Open one of the `/proc/<pid>/net/*` files for line-oriented streaming.
/// Returns `Ok(None)` when the kernel did not expose the file (e.g. no IPv6
/// in this namespace) so callers can skip it without surfacing the error.
fn open_proc_file(path: &str) -> Result<Option<BufReader<File>>, SocketInfoErr> {
    match File::open(path) {
        Ok(file) => Ok(Some(BufReader::new(file))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SocketInfoErr::Io(e)),
    }
}

pub fn get_socket_states_count(pid: u32) -> Result<SocketStateCount, SocketInfoErr> {
    let mut result: SocketStateCount = SocketStateCount::default();
    // Inodes are decimal integers both in `/proc/<pid>/fd/*` ("socket:[12345]")
    // and in the `/proc/<pid>/net/*` tables (column 9 for tcp/tcp6, column 6 for
    // unix). Storing them as u64 lets the matching step go through integer
    // hashing instead of allocating a `String` per fd and per matched row —
    // on a pooler with thousands of client connections this is the bulk of
    // the heap traffic the walk used to produce.
    let mut inodes: HashSet<u64> = HashSet::with_capacity(64);

    // Step 1: enumerate the socket fds the process holds.
    for entry in fs::read_dir(format!("/proc/{pid}/{FD_DIR}"))? {
        let path = entry?.path();
        if !is_socket(&path) {
            continue;
        }
        let target = fs::read_link(&path)?;
        let socket_name = match target.to_str() {
            Some(s) => s,
            None => continue,
        };
        if let Some(inode) = get_inode_u64(socket_name) {
            inodes.insert(inode);
        }
    }

    // Step 2: classify each by walking the matching kernel tables. Each
    // table is streamed line-by-line — `/proc/net/tcp` on a busy host is
    // routinely megabytes, and loading the whole file into a `String`
    // before parsing wastes both memory and a copy.
    if let Some(reader) = open_proc_file(&format!("/proc/{pid}/net/tcp"))? {
        fill_tcp(reader, &mut inodes, &mut result.tcp);
    }
    if let Some(reader) = open_proc_file(&format!("/proc/{pid}/net/tcp6"))? {
        fill_tcp(reader, &mut inodes, &mut result.tcp6);
    }
    if let Some(reader) = open_proc_file(&format!("/proc/{pid}/net/unix"))? {
        fill_unix(reader, &mut inodes, &mut result);
    }

    result.unknown += u16::try_from(inodes.len())?;
    Ok(result)
}

/// Shared snapshot of the most recent `get_socket_states_count` result.
/// Refreshed by `spawn_socket_states_refresh` on its own cadence so
/// consumers that don't need real-time freshness (Prometheus exporter,
/// periodic stats logger, admin `SHOW SOCKETS`) read it lock-free.
///
/// The Web UI's `/api/sockets` is the only consumer that needs real-time
/// freshness — it passes `fresh=true` and pays for a direct walk.
static SOCKET_STATES_CACHE: ArcSwapOption<SocketStateCount> = ArcSwapOption::const_empty();

/// Returns the process's socket-state breakdown. One entrypoint for both
/// real-time (Web UI) and cached (Prometheus, admin, logger) consumers so
/// the walk implementation is never duplicated.
///
/// - `fresh = true`: synchronously walks `/proc/<pid>/fd` plus the kernel
///   socket tables. Side effect: the freshly-walked snapshot is stored
///   into the shared cache so any cached reader that arrives moments
///   later sees the up-to-date value.
/// - `fresh = false`: returns whatever the background refresher last
///   produced, or an empty snapshot when the daemon has just started and
///   the refresher hasn't ticked yet. Never blocks, never walks `/proc`.
///
/// The reason cached readers never trigger a walk on cold-start: doing
/// so reintroduces the per-scrape syscall storm this module exists to
/// remove. One empty scrape immediately after restart is a better trade.
pub fn cached_socket_states_count(fresh: bool) -> Result<Arc<SocketStateCount>, SocketInfoErr> {
    if fresh {
        let pid = std::process::id();
        let states = Arc::new(get_socket_states_count(pid)?);
        SOCKET_STATES_CACHE.store(Some(states.clone()));
        return Ok(states);
    }
    Ok(SOCKET_STATES_CACHE
        .load_full()
        .unwrap_or_else(|| Arc::new(SocketStateCount::default())))
}

/// Spawn the background refresher. Must be called once, after a tokio
/// runtime is available. Each tick runs the synchronous walk on
/// `spawn_blocking` so the daemon's network worker threads never carry
/// the syscall cost. Skipped ticks (when a previous walk overruns the
/// interval) are dropped rather than coalesced into a burst, matching
/// the convention used by other periodic tasks in this codebase.
pub fn spawn_socket_states_refresh(refresh_interval: Duration) {
    let pid = std::process::id();
    tokio::task::spawn(async move {
        let mut interval = tokio::time::interval(refresh_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // `tokio::time::interval` returns immediately on the first
            // tick, so the cache is populated as soon as the runtime
            // schedules this task — readers only see the empty fallback
            // for the few hundred microseconds until then.
            interval.tick().await;
            // Move the walk off the runtime worker — kernel-side `/proc/net/tcp`
            // and per-fd readlink syscalls are blocking, and the whole point of
            // this module is to keep them away from threads that serve clients.
            let result = tokio::task::spawn_blocking(move || get_socket_states_count(pid)).await;
            match result {
                Ok(Ok(states)) => {
                    SOCKET_STATES_CACHE.store(Some(Arc::new(states)));
                }
                Ok(Err(e)) => {
                    warn!("socket-states refresh failed: {e}");
                }
                Err(join_err) => {
                    warn!("socket-states refresh task panicked: {join_err}");
                }
            }
        }
    });
}

const TCP_ROW_COLUMNS: usize = 17;
const UNIX_ROW_MIN_COLUMNS: usize = 7;

fn fill_tcp<R: BufRead>(mut reader: R, h_map: &mut HashSet<u64>, counts: &mut TcpStateCount) {
    // Reuse one row buffer across iterations; the parser strictly borrows
    // against it for the lifetime of the iteration body.
    let mut line = String::with_capacity(256);
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        // 39: A495FB0A:C566 2730FB0A:1920 01 00000000:00000000 02:00000418 00000000  5432        0 58864734 2 ff151d0987405780 20 4 30 94 -1
        //                                 ^^connection state                                        ^^inode
        let tokens: Vec<&str> = line.split_ascii_whitespace().collect();
        if tokens.len() != TCP_ROW_COLUMNS {
            continue;
        }
        let inode: u64 = match tokens[9].parse() {
            Ok(i) => i,
            Err(_) => continue,
        };
        if !h_map.contains(&inode) {
            continue;
        }
        // Match the master ordering: only remove from h_map once we have a
        // valid connection state. A row with a parseable inode but a
        // malformed state column keeps the inode in h_map so the eventual
        // tally lands in `result.unknown` — that bucket is intentional
        // signal for operators, not a quirk to optimise away.
        if let Ok(conn_state) = u8::from_str_radix(tokens[3], 16) {
            counts.increase_count(conn_state);
            h_map.remove(&inode);
        }
        #[cfg(debug_assertions)]
        {
            if let (Some(local), Some(remote)) = (parse_addr(tokens[1]), parse_addr(tokens[2])) {
                debug!("{local} <-> {remote} as {inode}");
            }
        }
    }
}

fn fill_unix<R: BufRead>(mut reader: R, h_map: &mut HashSet<u64>, counts: &mut SocketStateCount) {
    let mut line = String::with_capacity(256);
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        // ffff9b5456bcb400: 00000003 00000000 00000000 0001 03 281629229 /optional/path
        //                                              ^type ^state ^inode
        let tokens: Vec<&str> = line.split_ascii_whitespace().collect();
        if tokens.len() < UNIX_ROW_MIN_COLUMNS {
            continue;
        }
        let inode: u64 = match tokens[6].parse() {
            Ok(i) => i,
            Err(_) => continue,
        };
        if !h_map.contains(&inode) {
            continue;
        }
        let sock_type = match u8::from_str_radix(tokens[4], 16) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Same ordering rule as `fill_tcp`: a row reaches `unknown` if any
        // field after the inode fails to parse.
        match sock_type {
            // SOCK_STREAM=0001, SOCK_DGRAM=0002, SOCK_SEQPACKET=0005.
            1 => {
                if let Ok(conn_state) = u8::from_str_radix(tokens[5], 16) {
                    counts.unix_stream.increase_count(conn_state);
                    h_map.remove(&inode);
                }
            }
            2 => {
                counts.unix_dgram += 1;
                h_map.remove(&inode);
            }
            5 => {
                counts.unix_seq_packet += 1;
                h_map.remove(&inode);
            }
            _ => {}
        }
    }
}

fn is_socket(path: &Path) -> bool {
    let path_bytes = path.as_os_str().as_encoded_bytes();
    let mut buf_res: MaybeUninit<stat> = mem::MaybeUninit::uninit();
    let mut buf = MaybeUninit::<[u8; PATH_BUF_SIZE]>::uninit();
    let buf_ptr = buf.as_mut_ptr() as *mut u8;
    unsafe {
        ptr::copy_nonoverlapping(path_bytes.as_ptr(), buf_ptr, path_bytes.len());
        buf_ptr.add(path_bytes.len()).write(0);
    }
    match CStr::from_bytes_with_nul(unsafe { slice::from_raw_parts(buf_ptr, path_bytes.len() + 1) })
    {
        Ok(s) => {
            unsafe {
                libc::fstatat(
                    libc::AT_FDCWD,
                    s.as_ptr(),
                    buf_res.as_mut_ptr(),
                    c_int::default(),
                )
            };
            let mut result: mode_t;
            unsafe {
                result = buf_res.assume_init().st_mode;
            }
            // prune permission bits
            result = result >> 9 << 9;
            if result == libc::S_IFSOCK {
                return true;
            }
            false
        }
        Err(_) => false,
    }
}

fn get_inode(content: &str) -> Option<&str> {
    // 'socket:[1956357]'
    let s_index = match content.find(INODE_STR) {
        Some(s) => s + INODE_STR.len(),
        None => return None,
    };
    let e_index = match content[s_index..].find(']') {
        Some(e) => e + s_index,
        None => return None,
    };
    Some(&content[s_index..e_index])
}

/// Same logic as `get_inode` but returns the parsed integer directly, so the
/// hot walk does not have to materialise a `String` for every fd.
fn get_inode_u64(content: &str) -> Option<u64> {
    get_inode(content).and_then(|s| s.parse().ok())
}

#[cfg(debug_assertions)]
fn parse_addr(raw: &str) -> Option<SocketAddr> {
    // 0100007F:1920 -> 127.0.0.1:6432
    let words: Vec<&str> = raw.split(':').collect();
    if words.len() != 2 {
        return None;
    }
    // parse port
    let port: u16 = match u16::from_str_radix(words[1], 16) {
        Ok(port) => port,
        Err(_) => return None,
    };
    match words[0].len() {
        8 => {
            // ipv4
            let mut buf: [u8; 4] = [0; 4];
            for i in (0..words[0].len()).step_by(2).rev() {
                match u8::from_str_radix(&words[0][i..i + 2], 16) {
                    Ok(val) => buf[3 - i / 2] = val,
                    Err(_) => return None,
                };
            }
            Some(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(buf), port)))
        }
        32 => {
            // ipv6
            let mut buf: [u8; 16] = [0; 16];
            for i in (0..words[0].len()).step_by(2).rev() {
                match u8::from_str_radix(&words[0][i..i + 2], 16) {
                    Ok(val) => buf[15 - i / 2] = val,
                    Err(_) => return None,
                };
            }
            Some(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(buf),
                port,
                0,
                0,
            )))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! Format tests for the `[sockets] ...` log line and the v4+v6 merge
    //! helper. The format is consumed by grep/awk/Loki pipelines, so the key
    //! set, the ` | ` group boundaries, and the zero-value visibility all
    //! belong in pinned tests — any intentional change must update these.

    use super::*;

    fn sample_full() -> SocketStateCount {
        SocketStateCount {
            tcp: TcpStateCount {
                established: 600,
                listen: 1,
                time_wait: 3,
                fin_wait1: 0,
                fin_wait2: 2,
                close_wait: 5,
                last_ack: 0,
                syn_sent: 0,
                syn_recv: 0,
                new_syn_recv: 0,
                closing: 0,
                close: 0,
                bound_inactive: 0,
                total_count: 611,
            },
            tcp6: TcpStateCount {
                established: 10,
                listen: 1,
                time_wait: 0,
                fin_wait1: 0,
                fin_wait2: 0,
                close_wait: 1,
                last_ack: 0,
                syn_sent: 0,
                syn_recv: 0,
                new_syn_recv: 0,
                closing: 0,
                close: 0,
                bound_inactive: 0,
                total_count: 12,
            },
            unix_stream: UnixStreamStateCount {
                free: 0,
                unconnected: 0,
                connecting: 0,
                connected: 12,
                disconnecting: 0,
                total_count: 12,
            },
            unix_dgram: 1,
            unix_seq_packet: 0,
            unknown: 0,
        }
    }

    #[test]
    fn merged_tcp_breakdown_sums_v4_and_v6_per_state() {
        let merged = sample_full().merged_tcp_breakdown();
        assert_eq!(merged.established, 610);
        assert_eq!(merged.listen, 2);
        assert_eq!(merged.time_wait, 3);
        assert_eq!(merged.close_wait, 6);
        assert_eq!(merged.fin_wait2, 2);
        // States that are zero on both families stay zero.
        assert_eq!(merged.syn_sent, 0);
        assert_eq!(merged.closing, 0);
    }

    #[test]
    fn merged_tcp_breakdown_widens_to_u32_without_overflow() {
        // 50_000 + 50_000 overflows u16 but must fit in u32.
        let mut sc = SocketStateCount::default();
        sc.tcp.established = 50_000;
        sc.tcp6.established = 50_000;
        let merged = sc.merged_tcp_breakdown();
        assert_eq!(merged.established, 100_000);
    }

    #[test]
    fn display_emits_full_sockets_line_with_every_key() {
        let line = format!("{}", sample_full());
        let expected = "[sockets] tcp=623 tcp4=611 tcp6=12 \
            | tcp_est=610 tcp_lstn=2 tcp_tw=3 tcp_fw1=0 tcp_fw2=2 tcp_cw=6 tcp_la=0 \
            tcp_syns=0 tcp_synr=0 tcp_nsr=0 tcp_clsg=0 tcp_cls=0 tcp_bnd=0 \
            | unix=12 unix_conn=12 unix_uncn=0 unix_cng=0 unix_dcn=0 unix_free=0 \
            | dgram=1 seqpkt=0 unknown=0";
        assert_eq!(line, expected);
    }

    #[test]
    fn display_prints_zeros_on_an_empty_count() {
        // A DBA alert on `tcp_lstn=0` must still fire when nothing is listening,
        // so every field has to be present even when every counter is zero.
        let line = format!("{}", SocketStateCount::default());
        assert_eq!(
            line,
            "[sockets] tcp=0 tcp4=0 tcp6=0 \
             | tcp_est=0 tcp_lstn=0 tcp_tw=0 tcp_fw1=0 tcp_fw2=0 tcp_cw=0 tcp_la=0 \
             tcp_syns=0 tcp_synr=0 tcp_nsr=0 tcp_clsg=0 tcp_cls=0 tcp_bnd=0 \
             | unix=0 unix_conn=0 unix_uncn=0 unix_cng=0 unix_dcn=0 unix_free=0 \
             | dgram=0 seqpkt=0 unknown=0"
        );
    }

    #[test]
    fn display_uses_exactly_three_group_separators() {
        // Four groups → three ` | ` separators. Adding or removing a group is
        // a contract break for awk parsers that index into split-by-pipe fields.
        let line = format!("{}", sample_full());
        assert_eq!(line.matches(" | ").count(), 3);
    }

    #[test]
    fn display_starts_with_fixed_sockets_tag() {
        // The `[sockets]` prefix is how ops pipelines disambiguate this line
        // from per-pool lines, which start with `[user@pool]`.
        let line = format!("{}", sample_full());
        assert!(line.starts_with("[sockets] "));
        // And it must not accidentally look like a pool line.
        assert!(!line.starts_with("[sockets@"));
    }

    #[test]
    fn display_key_set_is_stable() {
        // Every key that grep/awk pipelines downstream can pattern-match on.
        // If you need to rename or add one, update this list *and* the
        // changelog/docs at the same time.
        let line = format!("{}", sample_full());
        for key in [
            "tcp=",
            "tcp4=",
            "tcp6=",
            "tcp_est=",
            "tcp_lstn=",
            "tcp_tw=",
            "tcp_fw1=",
            "tcp_fw2=",
            "tcp_cw=",
            "tcp_la=",
            "tcp_syns=",
            "tcp_synr=",
            "tcp_nsr=",
            "tcp_clsg=",
            "tcp_cls=",
            "tcp_bnd=",
            "unix=",
            "unix_conn=",
            "unix_uncn=",
            "unix_cng=",
            "unix_dcn=",
            "unix_free=",
            "dgram=",
            "seqpkt=",
            "unknown=",
        ] {
            assert!(line.contains(key), "missing key {key:?} in {line}");
        }
    }

    #[test]
    fn display_no_key_collides_with_pool_stats_keys() {
        // Pool stats already use `active`, `idle`, `wait`, `clients`,
        // `servers`, `qps`, `tps`, `query_ms`, `xact_ms`, `avg_wait`. None of
        // those must appear as a key in the sockets line, or a naive
        // `key=` grep over the whole stats block would double-count.
        let line = format!("{}", sample_full());
        for collision in [
            " active=",
            " idle=",
            " wait=",
            " clients=",
            " servers=",
            " qps=",
            " tps=",
            " query_ms ",
            " xact_ms ",
            " avg_wait=",
        ] {
            assert!(
                !line.contains(collision),
                "sockets line collided with pool-stats key {collision:?}"
            );
        }
    }

    // ----------------------------------------------------------------------
    // Pure parsing helpers — must keep working when /proc/net/tcp rows are
    // truncated, padded, or have a malformed state column. The `unknown`
    // bucket exists for the malformed case and is intentional signal for
    // operators; tests pin that behaviour.
    // ----------------------------------------------------------------------

    #[test]
    fn get_inode_u64_parses_socket_link() {
        assert_eq!(get_inode_u64("socket:[1956357]"), Some(1956357));
    }

    #[test]
    fn get_inode_u64_rejects_non_socket_link() {
        assert_eq!(get_inode_u64("/dev/null"), None);
    }

    #[test]
    fn get_inode_u64_rejects_malformed_inode() {
        assert_eq!(get_inode_u64("socket:[]"), None);
        assert_eq!(get_inode_u64("socket:[abc]"), None);
    }

    fn tcp_row(state_hex: &str, inode: u64) -> String {
        // 17 whitespace-separated columns; only the state at idx 3 and
        // inode at idx 9 are inspected. Spacing matches the actual layout.
        format!(
            "  0: 0100007F:1920 00000000:0000 {state_hex} \
             00000000:00000000 00:00000000 00000000  5432        0 \
             {inode} 1 0000000000000000 100 0 0 10 0"
        )
    }

    #[test]
    fn fill_tcp_counts_known_state() {
        let mut h_map: HashSet<u64> = [1001u64].into_iter().collect();
        let mut counts = TcpStateCount::default();
        // 01 == TCP_ESTABLISHED.
        let body = tcp_row("01", 1001);
        fill_tcp(body.as_bytes(), &mut h_map, &mut counts);
        assert_eq!(counts.established, 1);
        // Inode consumed: the caller's leftover set will not double-count.
        assert!(!h_map.contains(&1001));
    }

    #[test]
    fn fill_tcp_leaves_inode_for_unknown_bucket_on_bad_state() {
        // Malformed hex in the state column: the inode must NOT be removed
        // from h_map, because the caller tallies what is left over into
        // `result.unknown`. DBAs rely on that bucket to spot rows that
        // surprise the parser; if we silently dropped them, the line
        // would lie about how many sockets the process actually has.
        let mut h_map: HashSet<u64> = [2002u64].into_iter().collect();
        let mut counts = TcpStateCount::default();
        let body = tcp_row("zz", 2002);
        fill_tcp(body.as_bytes(), &mut h_map, &mut counts);
        assert_eq!(counts.total_count, 0);
        assert!(h_map.contains(&2002));
    }

    #[test]
    fn fill_tcp_ignores_rows_for_other_pids() {
        // h_map carries this process's inodes; rows that name an inode we
        // do not own must be skipped without touching the counters or the
        // set.
        let mut h_map: HashSet<u64> = [3003u64].into_iter().collect();
        let mut counts = TcpStateCount::default();
        let body = tcp_row("01", 9999);
        fill_tcp(body.as_bytes(), &mut h_map, &mut counts);
        assert_eq!(counts.established, 0);
        assert!(h_map.contains(&3003));
    }

    fn unix_row(type_hex: &str, state_hex: &str, inode: u64) -> String {
        // 7 columns minimum; the path at idx 7 is allowed to be absent.
        format!("ffff0000: 00000003 00000000 00000000 {type_hex} {state_hex} {inode}")
    }

    #[test]
    fn fill_unix_counts_stream_socket() {
        let mut h_map: HashSet<u64> = [4004u64].into_iter().collect();
        let mut counts = SocketStateCount::default();
        // 0001 = SOCK_STREAM, state 03 = connected.
        let body = unix_row("0001", "03", 4004);
        fill_unix(body.as_bytes(), &mut h_map, &mut counts);
        assert_eq!(counts.unix_stream.connected, 1);
        assert!(!h_map.contains(&4004));
    }

    #[test]
    fn fill_unix_counts_dgram_and_seqpacket() {
        let mut h_map: HashSet<u64> = [5005u64, 6006u64].into_iter().collect();
        let mut counts = SocketStateCount::default();
        let dgram = unix_row("0002", "00", 5005);
        let seq = unix_row("0005", "00", 6006);
        fill_unix(dgram.as_bytes(), &mut h_map, &mut counts);
        fill_unix(seq.as_bytes(), &mut h_map, &mut counts);
        assert_eq!(counts.unix_dgram, 1);
        assert_eq!(counts.unix_seq_packet, 1);
    }

    #[test]
    fn fill_unix_leaves_inode_for_unknown_bucket_on_bad_type() {
        let mut h_map: HashSet<u64> = [7007u64].into_iter().collect();
        let mut counts = SocketStateCount::default();
        // Type 0099 is not in {1, 2, 5} — leave inode for `unknown`.
        let body = unix_row("0099", "00", 7007);
        fill_unix(body.as_bytes(), &mut h_map, &mut counts);
        assert_eq!(counts.unix_dgram, 0);
        assert_eq!(counts.unix_seq_packet, 0);
        assert_eq!(counts.unix_stream.total_count, 0);
        assert!(h_map.contains(&7007));
    }

    #[test]
    fn fill_unix_leaves_inode_for_unknown_bucket_on_bad_state() {
        let mut h_map: HashSet<u64> = [8008u64].into_iter().collect();
        let mut counts = SocketStateCount::default();
        // SOCK_STREAM with garbage state: state parse fails, inode stays.
        let body = unix_row("0001", "zz", 8008);
        fill_unix(body.as_bytes(), &mut h_map, &mut counts);
        assert_eq!(counts.unix_stream.total_count, 0);
        assert!(h_map.contains(&8008));
    }
}
