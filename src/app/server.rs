use std::net::ToSocketAddrs;
use std::process;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{debug, error, info, warn};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpSocket;
#[cfg(not(windows))]
use tokio::signal::unix::{signal as unix_signal, SignalKind};
#[cfg(windows)]
use tokio::signal::windows as win_signal;
use tokio::{runtime::Builder, sync::mpsc};

use crate::app::args::Args;
use crate::config::{get_config, reload_config, Config};
use crate::daemon;
use crate::messages::configure_tcp_socket;
use crate::pool::{retain, ClientServerMap, ConnectionPool};
use crate::prometheus::start_prometheus_server;
use crate::stats::{Collector, Reporter, REPORTER, TOTAL_CONNECTION_COUNTER};
use crate::utils::core_affinity;
use crate::utils::format_duration;
use socket2::SockRef;
#[cfg(not(windows))]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(not(windows))]
use std::os::unix::process::CommandExt;

use crate::app::tls::init_tls;

/// Global counter for clients currently connected to the pg_doorman
pub static CURRENT_CLIENT_COUNT: AtomicI64 = AtomicI64::new(0);

/// Global flag indicating graceful shutdown is in progress
pub static SHUTDOWN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Global counter for clients currently in transactions (holding server connections)
pub static CLIENTS_IN_TRANSACTIONS: AtomicI64 = AtomicI64::new(0);

pub fn run_server(args: Args, config: Config) -> Result<(), Box<dyn std::error::Error>> {
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

    let tls_state = init_tls(&config);

    let thread_id = AtomicUsize::new(0);
    let core_ids = core_affinity::get_core_ids().unwrap();
    let mut worker_cpu_affinity_pinning = config.general.worker_cpu_affinity_pinning;
    if core_ids.len() < 3 {
        worker_cpu_affinity_pinning = false
    }
    if worker_cpu_affinity_pinning {
        core_affinity::set_for_current(core_ids[thread_id.fetch_add(1, Ordering::SeqCst)]);
    }

    let mut runtime_builder = Builder::new_multi_thread();
    runtime_builder
        .worker_threads(config.general.worker_threads)
        .enable_all()
        .thread_name("worker-pg-doorman");

    // Apply optional tokio runtime parameters only if explicitly configured.
    // Modern tokio versions handle defaults well, so these are optional.
    if let Some(interval) = config.general.tokio_global_queue_interval {
        runtime_builder.global_queue_interval(interval);
    }
    if let Some(interval) = config.general.tokio_event_interval {
        runtime_builder.event_interval(interval);
    }
    if let Some(ref stack_size) = config.general.worker_stack_size {
        runtime_builder.thread_stack_size(stack_size.as_usize());
    }
    if let Some(max_threads) = config.general.max_blocking_threads {
        runtime_builder.max_blocking_threads(max_threads);
    }

    let runtime = runtime_builder
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
                }
            }
        })
        .build()?;

    // Store inherit_fd before moving args into runtime
    #[cfg(not(windows))]
    let inherit_fd = args.inherit_fd;

    runtime.block_on(async move {
        // starting listener.
        let addr = format!("{}:{}", config.general.host, config.general.port)
            .to_socket_addrs()
            .unwrap()
            .next()
            .unwrap();

        #[cfg(not(windows))]
        let listener = if let Some(fd) = inherit_fd {
            // Inherit listener from parent process (binary upgrade in foreground mode)
            info!("Inheriting listener from parent process (fd={fd})");
            let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
            std_listener.set_nonblocking(true).expect("can't set nonblocking");
            tokio::net::TcpListener::from_std(std_listener).expect("can't create TcpListener from inherited fd")
        } else {
            // Create new listener
            let listen_socket = if addr.is_ipv4() {
                TcpSocket::new_v4().unwrap()
            } else {
                TcpSocket::new_v6().unwrap()
            };
            listen_socket
                .set_reuseaddr(true)
                .expect("can't set reuseaddr");
            listen_socket
                .set_reuseport(true)
                .expect("can't set reuseport");
            listen_socket
                .set_nodelay(true)
                .expect("can't set nodelay");
            {
                let sock_ref = SockRef::from(&listen_socket);
                sock_ref.set_linger(Some(Duration::from_secs(0)))
                    .expect("could not configure tcp_so_linger for socket");
            }
            // IPTOS_LOWDELAY: u8 = 0x10;
            if addr.is_ipv4() {
                match listen_socket.set_tos_v4(0x10) {
                    Ok(_) => (),
                    Err(err) => {
                        warn!("Can't set IPTOS_LOWDELAY: {err:?}");
                    }
                };
            };
            listen_socket.bind(addr).expect("can't bind");
            // end configure listener.
            let backlog = if config.general.backlog > 0 {
                config.general.backlog
            } else {
                config.general.max_connections as u32
            };
            match listen_socket.listen(backlog) {
                Ok(sock) => sock,
                Err(err) => {
                    error!("Listener socket error: {err:?}");
                    std::process::exit(exitcode::CONFIG);
                }
            }
        };

        #[cfg(windows)]
        let listener = {
            let listen_socket = if addr.is_ipv4() {
                TcpSocket::new_v4().unwrap()
            } else {
                TcpSocket::new_v6().unwrap()
            };
            listen_socket
                .set_reuseaddr(true)
                .expect("can't set reuseaddr");
            listen_socket
                .set_reuseport(true)
                .expect("can't set reuseport");
            listen_socket
                .set_nodelay(true)
                .expect("can't set nodelay");
            listen_socket
                .set_linger(Some(Duration::from_secs(0)))
                .expect("can't set linger 0");
            listen_socket.bind(addr).expect("can't bind");
            let backlog = if config.general.backlog > 0 {
                config.general.backlog
            } else {
                config.general.max_connections as u32
            };
            match listen_socket.listen(backlog) {
                Ok(sock) => sock,
                Err(err) => {
                    error!("Listener socket error: {err:?}");
                    std::process::exit(exitcode::CONFIG);
                }
            }
        };

        info!("Running on {addr}");

        config.show();

        // Tracks which client is connected to which server for query cancellation.
        let client_server_map: ClientServerMap =
            Arc::new(crate::utils::dashmap::new_dashmap(config.general.worker_threads));

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
            retain::retain_connections().await;
        });

        // Prometheus metrics exporter
        if config.prometheus.enabled {
            tokio::task::spawn(async move {
                start_prometheus_server(
                    format!("{}:{}", config.prometheus.host, config.prometheus.port).as_str(),
                )
                .await;
            });
        }

        // Signal readiness to parent process (for binary upgrade in foreground mode)
        #[cfg(not(windows))]
        if let Ok(ready_fd_str) = std::env::var("PG_DOORMAN_READY_FD") {
            if let Ok(ready_fd) = ready_fd_str.parse::<i32>() {
                info!("Signaling readiness to parent process (fd={ready_fd})");
                let ready_signal: [u8; 1] = [1];
                unsafe {
                    libc::write(ready_fd, ready_signal.as_ptr() as *const libc::c_void, 1);
                    libc::close(ready_fd);
                }
                // Remove the env var so it's not inherited by any future child processes
                std::env::remove_var("PG_DOORMAN_READY_FD");
            }
        }

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

        let (exit_tx, mut exit_rx) = mpsc::channel::<()>(1);
        let mut admin_only = false;

        let tls_rate_limiter = tls_state.rate_limiter.clone();
        let tls_acceptor = tls_state.acceptor.clone();

        // Wrap listener in Option to allow dropping it during foreground binary upgrade
        // while still continuing the graceful shutdown process
        let mut listener = Some(listener);

        info!("Waiting for dear clients");
        loop {
            // Create accept future only if listener is available
            let accept_future = async {
                if let Some(ref l) = listener {
                    l.accept().await
                } else {
                    // Listener was dropped (foreground binary upgrade), wait forever
                    std::future::pending().await
                }
            };

            tokio::select! {

                // Reload config:
                // kill -SIGHUP $(pgrep pg_doorman)
                _ = sighup_signal.recv() => {
                    info!("Reloading config");
                    _ = reload_config(client_server_map.clone()).await;
                    get_config().show();
                },

                // Initiate graceful shutdown sequence and run binary upgrade in background
                // kill -SIGINT $(pgrep pg_doorman)
                _ = interrupt_signal.recv() => {
                    info!("Got SIGINT, starting graceful shutdown");

                    // First, validate configuration of the new binary before proceeding with shutdown
                    #[cfg(not(windows))]
                    if !admin_only {
                        let full_exe_args: Vec<_> = std::env::args().collect();
                        let exe_path = &full_exe_args[0];

                        // Find config file from arguments (first positional argument)
                        let config_file = full_exe_args
                            .iter()
                            .skip(1)
                            .find(|arg| !arg.starts_with('-'))
                            .cloned()
                            .unwrap_or_else(|| "pg_doorman.toml".to_string());

                        info!("Validating configuration with: {exe_path} -t {config_file}");

                        let config_test_result = process::Command::new(exe_path)
                            .arg("-t")
                            .arg(&config_file)
                            .stdout(process::Stdio::piped())
                            .stderr(process::Stdio::piped())
                            .output();

                        match config_test_result {
                            Ok(output) => {
                                if !output.status.success() {
                                    // Configuration test FAILED - DO NOT proceed with shutdown!
                                    error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                    error!("!!!                    CRITICAL ERROR                               !!!");
                                    error!("!!!         CONFIGURATION VALIDATION FAILED                        !!!");
                                    error!("!!!         BINARY UPGRADE ABORTED - SHUTDOWN CANCELLED            !!!");
                                    error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                    error!("");
                                    error!("The new binary failed configuration validation!");
                                    error!("Configuration file: {config_file}");
                                    error!("Exit code: {:?}", output.status.code());
                                    if !output.stderr.is_empty() {
                                        error!("Error output: {}", String::from_utf8_lossy(&output.stderr));
                                    }
                                    if !output.stdout.is_empty() {
                                        error!("Standard output: {}", String::from_utf8_lossy(&output.stdout));
                                    }
                                    error!("");
                                    error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                    error!("!!!  FIX THE CONFIGURATION BEFORE ATTEMPTING BINARY UPGRADE AGAIN  !!!");
                                    error!("!!!  THE SERVER WILL CONTINUE RUNNING WITH THE CURRENT BINARY      !!!");
                                    error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                    continue;
                                }
                                info!("Configuration validation successful");
                            }
                            Err(e) => {
                                // Failed to run the config test - DO NOT proceed with shutdown!
                                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                error!("!!!                    CRITICAL ERROR                               !!!");
                                error!("!!!         FAILED TO VALIDATE CONFIGURATION                       !!!");
                                error!("!!!         BINARY UPGRADE ABORTED - SHUTDOWN CANCELLED            !!!");
                                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                error!("");
                                error!("Could not execute configuration test: {e}");
                                error!("Binary path: {exe_path}");
                                error!("");
                                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                error!("!!!  THE SERVER WILL CONTINUE RUNNING WITH THE CURRENT BINARY      !!!");
                                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                                continue;
                            }
                        }
                    }

                    SHUTDOWN_IN_PROGRESS.store(true, Ordering::SeqCst);

                    // Drain all idle connections from pools to release PostgreSQL connections
                    retain::drain_all_pools();

                    #[cfg(not(windows))]
                    if !admin_only {
                        // Binary upgrade: start new process with inherited listener fd
                        let full_exe_args: Vec<_> = std::env::args().collect();
                        let exe_path = &full_exe_args[0];
                        // Filter out any existing --inherit-fd argument and its value
                        let mut exe_args: Vec<String> = Vec::new();
                        let mut skip_next = false;
                        for arg in full_exe_args.iter().skip(1) {
                            if skip_next {
                                skip_next = false;
                                continue;
                            }
                            if arg == "--inherit-fd" {
                                skip_next = true;
                                continue;
                            }
                            if arg.starts_with("--inherit-fd=") {
                                continue;
                            }
                            exe_args.push(arg.to_string());
                        }
                        core_affinity::clear_for_current();

                        let listener_fd = listener.as_ref().unwrap().as_raw_fd();

                        if args.daemon {
                            // Daemon mode: start new daemon process
                            let mut child = {
                                let mut cmd = process::Command::new(exe_path);
                                cmd.args(&exe_args)
                                    .stderr(process::Stdio::null())
                                    .stdout(process::Stdio::null())
                                    .current_dir(std::env::current_dir().unwrap());
                                cmd.process_group(0);
                                cmd.spawn().unwrap()
                            };
                            child.wait().unwrap();
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            unsafe {
                                libc::close(listener_fd);
                            }
                        } else {
                            // Foreground mode: start new process with inherited listener fd
                            info!("Starting new process with inherited listener fd={listener_fd}");

                            // Get current process group to pass to child
                            let current_pgid = unsafe { libc::getpgrp() };
                            // Create a pipe for readiness signaling
                            let mut pipe_fds: [libc::c_int; 2] = [0; 2];
                            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
                                error!("Failed to create pipe for binary upgrade");
                            } else {
                                let pipe_read_fd = pipe_fds[0];
                                let pipe_write_fd = pipe_fds[1];

                                // Spawn child process with inherited listener fd and pipe write fd
                                let child_result = unsafe {
                                    let mut cmd = process::Command::new(exe_path);
                                    cmd.args(&exe_args)
                                        .arg("--inherit-fd")
                                        .arg(listener_fd.to_string())
                                        .env("PG_DOORMAN_READY_FD", pipe_write_fd.to_string())
                                        .current_dir(std::env::current_dir().unwrap())
                                        .pre_exec(move || {
                                            // Clear FD_CLOEXEC for listener_fd and pipe_write_fd
                                            // so they are inherited by the child
                                            libc::fcntl(listener_fd, libc::F_SETFD, 0);
                                            libc::fcntl(pipe_write_fd, libc::F_SETFD, 0);
                                            // Explicitly set process group to parent's group
                                            // This ensures the child stays in the same process group
                                            // even after parent dies
                                            libc::setpgid(0, current_pgid);

                                            Ok(())
                                        });
                                    cmd.spawn()
                                };

                                match child_result {
                                    Ok(_child) => {
                                        // Close write end in parent
                                        unsafe { libc::close(pipe_write_fd); }

                                        // Wait for child to signal readiness (or timeout)
                                        // Use a simple blocking read with timeout via select
                                        let mut buf: [u8; 1] = [0];
                                        let mut read_fds: libc::fd_set = unsafe { std::mem::zeroed() };
                                        unsafe {
                                            libc::FD_ZERO(&mut read_fds);
                                            libc::FD_SET(pipe_read_fd, &mut read_fds);
                                        }
                                        let mut timeout = libc::timeval {
                                            tv_sec: 10,
                                            tv_usec: 0,
                                        };
                                        let select_result = unsafe {
                                            libc::select(
                                                pipe_read_fd + 1,
                                                &mut read_fds,
                                                std::ptr::null_mut(),
                                                std::ptr::null_mut(),
                                                &mut timeout,
                                            )
                                        };

                                        if select_result > 0 {
                                            unsafe {
                                                libc::read(pipe_read_fd, buf.as_mut_ptr() as *mut libc::c_void, 1);
                                            }
                                            info!("New process signaled readiness");
                                        } else {
                                            warn!("Timeout waiting for new process readiness");
                                        }

                                        unsafe {
                                            libc::close(pipe_read_fd);
                                        }
                                        // Drop the listener to release the fd
                                        // Setting to None prevents "Bad file descriptor" errors
                                        // while allowing graceful shutdown to continue
                                        listener = None;
                                        info!("Foreground binary upgrade complete, listener released");
                                    }
                                    Err(e) => {
                                        error!("Failed to spawn new process: {e}");
                                        unsafe {
                                            libc::close(pipe_read_fd);
                                            libc::close(pipe_write_fd);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Don't want this to happen more than once
                    if admin_only {
                        continue;
                    }

                    admin_only = true;

                    let exit_tx = exit_tx.clone();
                    let shutdown_timeout = config.general.shutdown_timeout.as_std();

                    tokio::task::spawn(async move {
                        let clients_in_tx = CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed);
                        info!("waiting for {} client{} in transactions", clients_in_tx, if clients_in_tx == 1 { "" } else { "s" });

                        let mut interval = tokio::time::interval(shutdown_timeout);
                        let start = std::time::Instant::now();

                        loop {
                            interval.tick().await;

                            // Drain all idle connections from pools to release PostgreSQL connections
                            retain::drain_all_pools();

                            let clients_in_tx = CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed);
                            let clients_total = CURRENT_CLIENT_COUNT.load(Ordering::Relaxed);
                            if clients_total == 0 {
                                info!("All clients disconnected, shutting down");
                                let _ = exit_tx.send(()).await;
                                return;
                            }

                            if start.elapsed() >= shutdown_timeout {
                                error!("Graceful shutdown timed out. {clients_in_tx} active clients in transactions being closed");
                                let _ = exit_tx.send(()).await;
                                return;
                            }
                        }
                    });
                },

                _ = term_signal.recv() => {
                    let clients_in_tx = CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed);
                    info!("Got SIGTERM, closing with {clients_in_tx} clients in transactions");
                    break;
                },

                // new client.
                new_client = accept_future => {
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
                           match crate::client::client_entrypoint_too_many_clients_already(
                                socket, client_server_map).await {
                                Ok(()) => (),
                                Err(err) => {
                                    error!("Client {addr:?}: disconnected with error: {err}");
                                }
                            }
                            CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                            return
                        }
                        let start = chrono::offset::Utc::now().naive_utc();

                        match crate::client::client_entrypoint(
                            socket,
                            client_server_map,
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

            }
        }
        info!("Shutting down...");
    });

    Ok(())
}
