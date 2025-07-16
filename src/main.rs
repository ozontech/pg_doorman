// Copyright (c) 2022 Lev Kokotov <hi@levthe.dev>
// Copyright (c) 2023 Dmitriy Vasiliev <dmitrivasilyev@ozon.ru>

// Permission is hereby granted, free of charge, to any person obtaining
// a copy of this software and associated documentation files (the
// "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish,
// distribute, sublicense, and/or sell copies of the Software, and to
// permit persons to whom the Software is furnished to do so, subject to
// the following conditions:

// The above copyright notice and this permission notice shall be
// included in all copies or substantial portions of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
// EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
// NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
// LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
// WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use tikv_jemallocator::Jemalloc;
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpSocket;
#[cfg(not(windows))]
use tokio::signal::unix::{signal as unix_signal, SignalKind};
#[cfg(windows)]
use tokio::signal::windows as win_signal;
use tokio::sync::broadcast;
use tokio::{runtime::Builder, sync::mpsc};

extern crate exitcode;

use pg_doorman::config::{get_config, reload_config, VERSION};
use pg_doorman::core_affinity;
use pg_doorman::daemon;
use pg_doorman::format_duration;
use pg_doorman::messages::configure_tcp_socket;
use pg_doorman::pool::{retain_connections, ClientServerMap, ConnectionPool};
use pg_doorman::rate_limit::RateLimiter;
use pg_doorman::stats::{Collector, Reporter, REPORTER, TOTAL_CONNECTION_COUNTER};
use pg_doorman::tls::load_identity;
use pg_doorman::{cmd_args, logger};

pub static CURRENT_CLIENT_COUNT: Lazy<Arc<AtomicI64>> = Lazy::new(|| Arc::new(AtomicI64::new(0)));

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = cmd_args::parse();

    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic(info);
        std::process::exit(1);
    }));

    // Create a transient runtime for loading the config for the first time.
    {
        let runtime = Builder::new_multi_thread().worker_threads(1).build()?;

        runtime.block_on(async {
            match pg_doorman::config::parse(args.config_file.as_str()).await {
                Ok(_) => (),
                Err(err) => {
                    let stdin = io::stdin();
                    if stdin.is_terminal() {
                        eprintln!("Config parse error: {err}");
                        io::stdout().flush().unwrap();
                    } else {
                        error!("Config parse error: {err:?}");
                    }
                    std::process::exit(exitcode::CONFIG);
                }
            };
        });
    }

    let config = get_config();
    logger::init(&args, config.general.syslog_prog_name.clone());

    info!("Welcome to PgDoorman! (Version {VERSION})");

    if args.daemon {
        let pid_file = config.general.daemon_pid_file.clone();
        let daemonize = daemon::lib::Daemonize::new()
            .pid_file(pid_file)
            .working_directory(std::env::current_dir().unwrap())
            .chown_pid_file(true);
        match daemonize.start() {
            Ok(_) => println!("Success, daemonized"),
            Err(e) => {
                eprintln!("Error daemonize: {e}");
                process::exit(exitcode::OSERR);
            }
        }
    }

    let thread_id = AtomicUsize::new(0);
    let core_ids = core_affinity::get_core_ids().unwrap();
    let mut worker_cpu_affinity_pinning = config.general.worker_cpu_affinity_pinning;
    if core_ids.len() < 3 {
        worker_cpu_affinity_pinning = false
    }
    if worker_cpu_affinity_pinning {
        core_affinity::set_for_current(core_ids[thread_id.fetch_add(1, Ordering::SeqCst)]);
    }
    // Create the runtime now we know required worker_threads.
    let runtime = Builder::new_multi_thread()
        .worker_threads(config.general.worker_threads)
        .enable_all()
        .thread_name("worker-pg-doorman")
        .global_queue_interval(config.general.tokio_global_queue_interval)
        .event_interval(config.general.tokio_event_interval)
        .thread_stack_size(config.general.worker_stack_size)
        .max_blocking_threads(16 * config.general.worker_stack_size)
        .on_thread_start(move || {
            if worker_cpu_affinity_pinning {
                let core_id = thread_id.fetch_add(1, Ordering::SeqCst);
                info!(
                    "Affinity pin tokio thread {} on core: {}",
                    core_id, core_ids[core_id].id
                );
                core_affinity::set_for_current(core_ids[core_id]);
                if core_id == core_ids.len() - 1 {
                    thread_id.store(0, Ordering::SeqCst);
                    thread_id.fetch_add(1, Ordering::SeqCst);
                }
            }
        })
        .build()?;

    runtime.block_on(async move {

        // starting listener.
        let addr = format!("{}:{}", config.general.host, config.general.port).parse().unwrap();
        let listen_socket = TcpSocket::new_v4().unwrap();
        listen_socket.set_reuseaddr(true).expect("can't set reuseaddr");
        listen_socket.set_reuseport(true).expect("can't set reuseport");
        listen_socket.set_nodelay(true).expect("can't set nodelay");
        listen_socket.set_linger(Some(Duration::from_secs(0))).expect("can't set linger 0");
        // IPTOS_LOWDELAY: u8 = 0x10;
        listen_socket.set_tos(0x10).expect("can't set tos");
        listen_socket.bind(addr).expect("can't bind");
        // end configure listener.
        let backlog = if config.general.backlog > 0 {
            config.general.backlog
        } else {
            config.general.max_connections as u32
        };
        let listener = match listen_socket.listen(backlog) {
            Ok(sock) => sock,
            Err(err) => {
                error!("Listener socket error: {err:?}");
                std::process::exit(exitcode::CONFIG);
            }
        };
        info!("Running on {addr}");

        config.show();

        // Tracks which client is connected to which server for query cancellation.
        let client_server_map: ClientServerMap = Arc::new(Mutex::new(HashMap::new()));

        // Statistics reporting.
        REPORTER.store(Arc::new(Reporter::default()));

        // Connection pool that allows to query all databases.
        match ConnectionPool::from_config(client_server_map.clone()).await {
            Ok(_) => (),
            Err(err) => {
                error!("Pool error: {err:?}");
                std::process::exit(exitcode::CONFIG);
            }
        };

        tokio::task::spawn(async move {
            let mut stats_collector = Collector::default();
            stats_collector.collect().await;
        });

        tokio::task::spawn(async move {
            retain_connections().await;
        });

        #[cfg(windows)]
        let mut term_signal = win_signal::ctrl_close().unwrap();
        #[cfg(windows)]
        let mut interrupt_signal = win_signal::ctrl_c().unwrap();
        #[cfg(windows)]
        let mut sighup_signal = win_signal::ctrl_shutdown().unwrap();
        #[cfg(not(windows))]
        let mut term_signal = unix_signal(SignalKind::terminate()).unwrap();
        #[cfg(not(windows))]
        let mut interrupt_signal = unix_signal(SignalKind::interrupt()).unwrap();
        #[cfg(not(windows))]
        let mut sighup_signal = unix_signal(SignalKind::hangup()).unwrap();
        let (shutdown_tx, _) = broadcast::channel::<()>(1);
        let (drain_tx, mut drain_rx) = mpsc::channel::<i32>(2048);
        let (exit_tx, mut exit_rx) = mpsc::channel::<()>(1);
        let mut admin_only = false;
        let mut total_clients = 0;

        // It is not updated by 'HUP'.
        let tls_rate_limiter: Option<RateLimiter> = if config.general.tls_rate_limit_per_second > 0 {
            info!("Building rate limit: {} per second", config.general.tls_rate_limit_per_second);
            Some(RateLimiter::new(config.general.tls_rate_limit_per_second/100, 10))
        } else {
            None
        };

        // It is not updated by 'HUP'.
        let tls_acceptor: Option<tokio_native_tls::TlsAcceptor> = if config.general.tls_certificate.is_some() {
            match load_identity(
                Path::new(&config.general.tls_certificate.unwrap()),
                Path::new(&config.general.tls_private_key.unwrap())) {
                Ok(identity) => {
                    Some(tokio_native_tls::TlsAcceptor::from(native_tls::TlsAcceptor::new(identity).unwrap()))
                },
                Err(_) => None
            }
        } else {
            None
        };

        info!("Waiting for dear clients");
        loop {
            tokio::select! {

                // Reload config:
                // kill -SIGHUP $(pgrep pg_doorman)
                _ = sighup_signal.recv() => {
                    info!("Reloading config");
                    _ = reload_config(client_server_map.clone()).await;
                    get_config().show();
                },

                // Initiate graceful shutdown sequence on sig int
                // kill -SIGINT $(pgrep pg_doorman)
                _ = interrupt_signal.recv() => {
                    info!("Got SIGINT, starting graceful shutdown");

                    if args.daemon && !admin_only {
                        // start daemon.
                        let full_exe_args: Vec<_> = std::env::args().collect();
                        let exe_path = &full_exe_args[0];
                        let exe_args = full_exe_args.iter().skip(1);
                        core_affinity::clear_for_current();
                        let mut child = process::Command::new(exe_path)
                            .args(exe_args)
                            .stderr(process::Stdio::null())
                            .stdout(process::Stdio::null())
                            .current_dir(std::env::current_dir().unwrap())
                            .process_group(0)
                            .spawn().unwrap();
                        child.wait().unwrap();
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        unsafe { libc::close(listener.as_raw_fd()); }
                    }

                    // Don't want this to happen more than once
                    if admin_only {
                        continue;
                    }

                    admin_only = true;

                    // Broadcast that client tasks need to finish
                    let _ = shutdown_tx.send(());
                    let exit_tx = exit_tx.clone();
                    let _ = drain_tx.send(0).await;

                    tokio::task::spawn(async move {
                        info!("waiting {total_clients} client");

                        let mut interval = tokio::time::interval(Duration::from_millis(config.general.shutdown_timeout));

                        // First tick fires immediately.
                        interval.tick().await;

                        // Second one in the interval time.
                        interval.tick().await;

                        // We're done waiting.
                        error!("Graceful shutdown timed out. {total_clients} active clients being closed");

                        let _ = exit_tx.send(()).await;
                    });
                },

                _ = term_signal.recv() => {
                    info!("Got SIGTERM, closing with {total_clients} clients active");
                    break;
                },

                // new client.
                new_client = listener.accept() => {
                    let (mut socket, addr) = match new_client {
                        Ok((socket, addr)) => (socket, addr),
                        Err(err) => {
                            error!("accept error: {err:?}");
                            continue;
                        }
                    };
                    if admin_only {
                        error!("Accepting new client {addr} after shutdown");
                        let _ = socket.shutdown().await;
                        continue;
                    }
                    let tls_rate_limiter = tls_rate_limiter.clone();
                    let tls_acceptor = tls_acceptor.clone();
                    let shutdown_rx = shutdown_tx.subscribe();
                    let drain_tx = drain_tx.clone();
                    let client_server_map = client_server_map.clone();
                    let config = get_config();

                    let log_client_disconnections = config.general.log_client_connections;
                    let max_connections = config.general.max_connections;

                    configure_tcp_socket(&socket);
                    tokio::task::spawn(async move {
                        TOTAL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
                        let current_clients = CURRENT_CLIENT_COUNT.fetch_add(1, Ordering::SeqCst);
                        // max clients.
                        if current_clients as u64 > max_connections {
                            warn!("Client {addr:?}: too many clients already");
                           match pg_doorman::client::client_entrypoint_too_many_clients_already(
                                socket, client_server_map, shutdown_rx, drain_tx).await {
                                Ok(()) => (),
                                Err(err) => {
                                    error!("Client {addr:?}: disconnected with error: {err}");
                                }
                            }
                            CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                            return
                        }
                        let start = chrono::offset::Utc::now().naive_utc();

                        match pg_doorman::client::client_entrypoint(
                            socket,
                            client_server_map,
                            shutdown_rx,
                            drain_tx,
                            admin_only,
                            tls_acceptor,
                            tls_rate_limiter,
                        )
                        .await
                        {
                            Ok(()) => {
                                let duration = chrono::offset::Utc::now().naive_utc() - start;

                                if log_client_disconnections {
                                    info!(
                                        "Client {:?} disconnected, session duration: {}",
                                        addr,
                                        format_duration(&duration)
                                    );
                                } else {
                                    debug!(
                                        "Client {:?} disconnected, session duration: {}",
                                        addr,
                                        format_duration(&duration)
                                    );
                                }
                            }

                            Err(err) => {
                                let duration = chrono::offset::Utc::now().naive_utc() - start;
                                warn!("Client {:?} disconnected with error {:?}, duration: {}", addr, err, format_duration(&duration));
                            }
                        };
                        CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                    });
                }

                _ = exit_rx.recv() => {
                    break;
                }

                client_ping = drain_rx.recv() => {
                    let client_ping = client_ping.unwrap();
                    total_clients += client_ping;

                    if total_clients == 0 && admin_only {
                        let _ = exit_tx.send(()).await;
                    }
                }

            }
        }
    info!("Shutting down...");
    });

    Ok(())
}
