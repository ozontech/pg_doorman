use libc::{c_int, mode_t, stat};
#[cfg(debug_assertions)]
use log::debug;
use std::collections::HashSet;
use std::ffi::CStr;
use std::fmt::{Debug, Display, Formatter};
use std::fs::File;
use std::io::Read;
use std::mem::MaybeUninit;
#[cfg(debug_assertions)]
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use std::path::Path;
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
struct TcpStateCount {
    // https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/net/tcp_states.h
    established: u16,
    syn_sent: u16,
    syn_recv: u16,
    fin_wait1: u16,
    fin_wait2: u16,
    time_wait: u16,
    close: u16,
    close_wait: u16,
    last_ack: u16,
    listen: u16,
    closing: u16,
    new_syn_recv: u16,
    bound_inactive: u16,

    total_count: u32,
}

#[derive(Default)]
struct UnixStreamStateCount {
    // https://github.com/ecki/net-tools/blob/master/netstat.c#L121
    free: u16,          /* not allocated                */
    unconnected: u16,   /* unconnected to any socket    */
    connecting: u16,    /* in process of connecting     */
    connected: u16,     /* connected to socket          */
    disconnecting: u16, /* in process of disconnecting  */

    total_count: u32,
}

#[derive(Default)]
pub struct SocketStateCount {
    tcp: TcpStateCount,
    tcp6: TcpStateCount,
    unix_stream: UnixStreamStateCount,
    unix_dgram: u16,
    unix_seq_packet: u16,
    unknown: u16,
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

fn read_proc_file(path: &str) -> Result<Option<String>, SocketInfoErr> {
    match File::open(path) {
        Ok(mut file) => {
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            Ok(Some(content))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SocketInfoErr::Io(e)),
    }
}

pub fn get_socket_states_count(pid: u32) -> Result<SocketStateCount, SocketInfoErr> {
    let mut result: SocketStateCount = SocketStateCount {
        ..Default::default()
    };
    let mut inodes: HashSet<String> = HashSet::new();
    // run through /proc/<pid>/fd to find sockets with their inodes
    for entry in fs::read_dir(format!("/proc/{pid}/{FD_DIR}"))? {
        let path = &entry.unwrap().path();
        if !is_socket(path) {
            continue;
        }
        let target = fs::read_link(path)?;
        let socket_name = match target.to_str() {
            Some(socket_name) => socket_name,
            None => continue,
        };
        let inode: String = match get_inode(socket_name) {
            Some(inode) => String::from(inode),
            _ => continue,
        };
        inodes.insert(inode);
    }

    if let Some(content) = read_proc_file(&format!("/proc/{pid}/net/tcp"))? {
        fill_tcp(&content, &mut inodes, &mut result.tcp);
    }
    if let Some(content) = read_proc_file(&format!("/proc/{pid}/net/tcp6"))? {
        fill_tcp(&content, &mut inodes, &mut result.tcp6);
    }
    if let Some(content) = read_proc_file(&format!("/proc/{pid}/net/unix"))? {
        fill_unix(&content, &mut inodes, &mut result);
    }

    result.unknown += u16::try_from(inodes.len())?;
    Ok(result)
}

fn fill_tcp(content: &str, h_map: &mut HashSet<String>, counts: &mut TcpStateCount) {
    for row in content.split('\n') {
        // 39: A495FB0A:C566 2730FB0A:1920 01 00000000:00000000 02:00000418 00000000  5432        0 58864734 2 ff151d0987405780 20 4 30 94 -1
        //                                 ^^connection state                                        ^^inode
        let words: Vec<&str> = row.trim().split(' ').filter(|s| !s.is_empty()).collect();
        if words.len() != 17 {
            continue;
        }
        if h_map.contains(words[9]) {
            match u8::from_str_radix(words[3], 16) {
                Ok(conn_state) => counts.increase_count(conn_state),
                Err(_) => continue,
            };
            h_map.remove(words[9]);
            #[cfg(debug_assertions)]
            {
                let local_socket = match parse_addr(words[1]) {
                    Some(l) => l,
                    None => continue,
                };
                let remote_socket = match parse_addr(words[2]) {
                    Some(l) => l,
                    None => continue,
                };
                debug!("{} <-> {} as {}", local_socket, remote_socket, words[9]);
            }
        }
    }
}

fn fill_unix(content: &str, h_map: &mut HashSet<String>, counts: &mut SocketStateCount) {
    for row in content.split('\n') {
        // ffff9b5456bcb400: 00000003 00000000 00000000 0001 03 281629229 /optional/path
        //                                              ^type ^state ^inode
        let words: Vec<&str> = row.trim().split(' ').filter(|s| !s.is_empty()).collect();
        if words.len() < 7 {
            continue;
        }
        if h_map.contains(words[6]) {
            let sock_type = match u8::from_str_radix(words[4], 16) {
                Ok(sock_type) => sock_type,
                Err(_) => continue,
            };
            match sock_type {
                /*
                 For SOCK_STREAM sockets, this is
                 0001; for SOCK_DGRAM sockets, it is 0002; and for
                 SOCK_SEQPACKET sockets, it is 0005
                */
                1 => {
                    match u8::from_str_radix(words[5], 16) {
                        Ok(conn_state) => counts.unix_stream.increase_count(conn_state),
                        Err(_) => continue,
                    };
                }
                2 => counts.unix_dgram += 1,
                5 => counts.unix_seq_packet += 1,
                _ => continue,
            }
            h_map.remove(words[6]);
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
}
