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

impl Display for TcpStateCount {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut str_buf: Vec<String> = Vec::new();
        if self.established != 0 {
            str_buf.push(format!("ESTABLISHED: {}", self.established));
        }
        if self.syn_sent != 0 {
            str_buf.push(format!("SYN_SENT: {}", self.syn_sent));
        }
        if self.syn_recv != 0 {
            str_buf.push(format!("SYN_RECV: {}", self.syn_recv));
        }
        if self.fin_wait1 != 0 {
            str_buf.push(format!("FIN_WAIT1: {}", self.fin_wait1));
        }
        if self.fin_wait2 != 0 {
            str_buf.push(format!("FIN_WAIT2: {}", self.fin_wait2));
        }
        if self.time_wait != 0 {
            str_buf.push(format!("TIME_WAIT: {}", self.time_wait));
        }
        if self.close != 0 {
            str_buf.push(format!("CLOSE: {}", self.close));
        }
        if self.close_wait != 0 {
            str_buf.push(format!("CLOSE_WAIT: {}", self.close_wait));
        }
        if self.last_ack != 0 {
            str_buf.push(format!("LAST_ACK: {}", self.last_ack));
        }
        if self.listen != 0 {
            str_buf.push(format!("LISTEN: {}", self.listen));
        }
        if self.closing != 0 {
            str_buf.push(format!("CLOSING: {}", self.closing));
        }
        if self.new_syn_recv != 0 {
            str_buf.push(format!("NEW_SYN_RECV: {}", self.new_syn_recv));
        }
        if self.bound_inactive != 0 {
            str_buf.push(format!("BOUND_INACTIVE: {}", self.bound_inactive));
        }
        f.write_fmt(format_args!("[{}]", str_buf.join(", ")))?;
        Ok(())
    }
}

impl Display for UnixStreamStateCount {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut str_buf: Vec<String> = Vec::new();
        if self.unconnected != 0 {
            str_buf.push(format!("UNCONNECTED: {}", self.unconnected));
        }
        if self.connecting != 0 {
            str_buf.push(format!("CONNECTING: {}", self.connecting));
        }
        if self.connected != 0 {
            str_buf.push(format!("CONNECTED: {}", self.connected));
        }
        if self.disconnecting != 0 {
            str_buf.push(format!("DISCONNECTING: {}", self.disconnecting));
        }
        if self.free != 0 {
            str_buf.push(format!("FREE: {}", self.free));
        }
        f.write_fmt(format_args!("[{}]", str_buf.join(", ")))?;
        Ok(())
    }
}

impl Display for SocketStateCount {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut str_buf: Vec<String> = Vec::new();
        let tcp_total = self.tcp.get_total();
        if tcp_total != 0 {
            str_buf.push(format!("{} tcp sockets: {}", tcp_total, self.tcp));
        }
        let tcp6_total = self.tcp6.get_total();
        if tcp6_total != 0 {
            str_buf.push(format!("{} tcp6 sockets: {}", tcp6_total, self.tcp6));
        }
        let unix_total = self.unix_stream.get_total();
        if unix_total != 0 {
            str_buf.push(format!(
                "{} unix SOCK_STREAMs: {}",
                unix_total, self.unix_stream
            ));
        }
        if self.unix_dgram != 0 {
            str_buf.push(format!("SOCK_DGRAM: {}", self.unix_dgram));
        }
        if self.unix_seq_packet != 0 {
            str_buf.push(format!("SOCK_SEQPACKET: {}", self.unix_seq_packet));
        }
        if self.unknown != 0 {
            str_buf.push(format!("UNKNOWN={}", self.unknown));
        }
        write!(f, "{}", str_buf.join(", "))?;
        Ok(())
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

    // match inodes with tcp connections in /proc/<pid>/net/tcp
    let mut file = File::open(format!("/proc/{pid}/net/tcp"))?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    fill_tcp(&content, &mut inodes, &mut result.tcp);

    // match inodes with tcp connections in /proc/<pid>/net/tcp6
    let mut file = File::open(format!("/proc/{pid}/net/tcp6"))?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    fill_tcp(&content, &mut inodes, &mut result.tcp6);

    // match inodes with unix sockets in /proc/<pid>/net/unix
    file = File::open(format!("/proc/{pid}/net/unix"))?;
    content = String::new();
    file.read_to_string(&mut content)?;
    fill_unix(&content, &mut inodes, &mut result);

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
