use crate::stats::pool::PoolStats;
#[cfg(target_os = "linux")]
use crate::stats::socket::cached_socket_states_count;
#[cfg(target_os = "linux")]
use log::error;
use log::info;
#[cfg(target_os = "linux")]
use std::time::Duration;

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
                "[{}@{}] qps={} tps={} \
                | clients={} active={} idle={} wait={} \
                | servers={} active={} idle={} \
                | query_ms p50={:.2} p90={:.2} p95={:.2} p99={:.2} \
                | xact_ms p50={:.2} p90={:.2} p95={:.2} p99={:.2} \
                | wait_ms p50={:.2} p90={:.2} p95={:.2} p99={:.2} \
                | avg_wait={:.3}ms",
                identifier.user,
                identifier.db,
                pool_stats.avg_query_count,
                pool_stats.avg_xact_count,
                total_clients,
                pool_stats.cl_active,
                pool_stats.cl_idle,
                pool_stats.cl_waiting,
                total_servers,
                pool_stats.sv_active,
                pool_stats.sv_idle,
                pool_stats.query_percentile.p50 as f64 / 1_000f64,
                pool_stats.query_percentile.p90 as f64 / 1_000f64,
                pool_stats.query_percentile.p95 as f64 / 1_000f64,
                pool_stats.query_percentile.p99 as f64 / 1_000f64,
                pool_stats.xact_percentile.p50 as f64 / 1_000f64,
                pool_stats.xact_percentile.p90 as f64 / 1_000f64,
                pool_stats.xact_percentile.p95 as f64 / 1_000f64,
                pool_stats.xact_percentile.p99 as f64 / 1_000f64,
                pool_stats.wait_percentile.p50 as f64 / 1_000f64,
                pool_stats.wait_percentile.p90 as f64 / 1_000f64,
                pool_stats.wait_percentile.p95 as f64 / 1_000f64,
                pool_stats.wait_percentile.p99 as f64 / 1_000f64,
                pool_stats.avg_wait_time as f64 / 1_000f64,
            );
        }
    });
    #[cfg(target_os = "linux")]
    {
        // Same 10-s budget as the Prometheus exporter — both consumers go
        // through the shared cache so they never duplicate the walk through
        // /proc/<pid>/fd and the kernel socket tables.
        const SOCKETS_TTL: Duration = Duration::from_secs(10);

        if clients_flag {
            match cached_socket_states_count(std::process::id(), SOCKETS_TTL) {
                // The `Display` impl now emits the full `[sockets] ...` line
                // so that grep/awk pipelines can parse it the same way as the
                // pool-stats lines above.
                Ok(info) => info!("{}", *info),
                Err(err) => error!("[sockets] error: {err}"),
            };
        }
    }
}
