use crate::web::routes::dto::{SocketsDto, TcpCounts, UnixStreamCounts};

use super::now_unix_ms;

pub(crate) fn collect_sockets() -> Result<SocketsDto, &'static str> {
    use crate::stats::socket::{get_socket_states_count, TcpStateCount, UnixStreamStateCount};

    let info = get_socket_states_count(std::process::id())
        .map_err(|_| "failed to read socket states from /proc")?;

    fn tcp(c: &TcpStateCount) -> TcpCounts {
        TcpCounts {
            established: c.established as u64,
            syn_sent: c.syn_sent as u64,
            syn_recv: c.syn_recv as u64,
            fin_wait1: c.fin_wait1 as u64,
            fin_wait2: c.fin_wait2 as u64,
            time_wait: c.time_wait as u64,
            close: c.close as u64,
            close_wait: c.close_wait as u64,
            last_ack: c.last_ack as u64,
            listen: c.listen as u64,
            closing: c.closing as u64,
            new_syn_recv: c.new_syn_recv as u64,
            bound_inactive: c.bound_inactive as u64,
        }
    }

    fn unix_stream(c: &UnixStreamStateCount) -> UnixStreamCounts {
        UnixStreamCounts {
            free: c.free as u64,
            unconnected: c.unconnected as u64,
            connecting: c.connecting as u64,
            connected: c.connected as u64,
            disconnecting: c.disconnecting as u64,
        }
    }

    Ok(SocketsDto {
        ts: now_unix_ms(),
        tcp: tcp(&info.tcp),
        tcp6: tcp(&info.tcp6),
        unix_stream: unix_stream(&info.unix_stream),
        unix_dgram: info.unix_dgram as u64,
        unix_seq_packet: info.unix_seq_packet as u64,
        unknown: info.unknown as u64,
    })
}
