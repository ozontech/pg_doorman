use crate::stats::pool::PoolStats;
#[cfg(target_os = "linux")]
use crate::stats::socket::get_socket_states_count;
#[cfg(target_os = "linux")]
use log::error;
use log::info;

pub fn print_all_stats() {
    let pool_lookup = PoolStats::construct_pool_lookup();
    let mut clients_flag: bool = false;
    pool_lookup.iter().for_each(|(identifier, pool_stats)| {
        let total_clients = pool_stats.cl_waiting
            + pool_stats.cl_idle
            + pool_stats.cl_active
            + pool_stats.cl_cancel_req;
        let total_servers = pool_stats.sv_active + pool_stats.sv_idle;
        if total_clients > 0 {
            clients_flag = true;
            info!(
                "[{}@{}] \
                {}qps/{}tps, \
                {} clients [active: {}, idle: {}, waiting: {}], \
                {} servers [active: {}, idle: {}], \
                query/xact ms: [p99: {:.3}/{:.3} p95: {:.3}/{:.3} p90: {:.3}/{:.3} p50: {:.3}/{:.3}], \
                avg_wait (per query): {:.5} {:?} ms.",

                identifier.user, identifier.db,

                pool_stats.avg_query_count,
                pool_stats.avg_xact_count,

                total_clients,
                pool_stats.cl_active,
                pool_stats.cl_idle,
                pool_stats.cl_waiting,

                total_servers,
                pool_stats.sv_active,
                pool_stats.sv_idle,

                pool_stats.query_percentile.p99 as f64 / 1_000f64,
                pool_stats.xact_percentile.p99 as f64 / 1_000f64,
                pool_stats.query_percentile.p95 as f64 / 1_000f64,
                pool_stats.xact_percentile.p95 as f64 / 1_000f64,
                pool_stats.query_percentile.p90 as f64 / 1_000f64,
                pool_stats.xact_percentile.p90 as f64 / 1_000f64,
                pool_stats.query_percentile.p50 as f64 / 1_000f64,
                pool_stats.xact_percentile.p50 as f64 / 1_000f64,

                pool_stats.avg_wait_time as f64 / 1_000f64,
                pool_stats.avg_wait_time_vp_ms,
            );
        }
    });
    #[cfg(target_os = "linux")]
    {
        if clients_flag {
            match get_socket_states_count(std::process::id()) {
                Ok(info) => {
                    info!("Connection states: {}", info)
                }
                Err(err) => {
                    error!("Connection states: {}", err)
                }
            };
        }
    }
}
