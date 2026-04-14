use std::net::ToSocketAddrs;
use std::process;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use log::{error, info, warn};
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
use crate::client::migration::MigrationPayload;
#[cfg(unix)]
use crate::client::migration::{migration_receiver_task, migration_sender_task};

/// Global counter for clients currently connected to the pg_doorman
pub static CURRENT_CLIENT_COUNT: AtomicI64 = AtomicI64::new(0);

/// Global flag indicating graceful shutdown is in progress
pub static SHUTDOWN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Global counter for clients currently in transactions (holding server connections)
pub static CLIENTS_IN_TRANSACTIONS: AtomicI64 = AtomicI64::new(0);

/// Global flag: migration to new process is active. Clients should self-migrate at idle points.
pub static MIGRATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Channel sender for migration payloads. Set once when migration starts.
pub static MIGRATION_TX: std::sync::OnceLock<mpsc::Sender<MigrationPayload>> =
    std::sync::OnceLock::new();

/// Max buffered migration payloads before sender blocks.
/// Sized to handle all clients migrating simultaneously without contention.
const MIGRATION_CHANNEL_CAPACITY: usize = 4096;

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
                    "Pinning tokio worker thread {} to core {}",
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
            info!("Inheriting listener from parent process (fd={})", fd);
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
                        warn!("Failed to set IPTOS_LOWDELAY on listener socket: {err}");
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
                    error!("Listener socket error: {err}");
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
                    error!("Listener socket error: {err}");
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
                error!("Failed to initialize connection pools: {err}");
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

        // Dynamic pool GC — cheap no-op when DYNAMIC_POOLS is empty
        {
            let gc_interval = config.general.retain_connections_time.as_std();
            crate::pool::gc::spawn_dynamic_pool_gc(gc_interval);
        }

        let shutdown_timeout = config.general.shutdown_timeout.as_std();

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
                info!("Signaling readiness to parent process (fd={})", ready_fd);
                let ready_signal: [u8; 1] = [1];
                unsafe {
                    libc::write(ready_fd, ready_signal.as_ptr() as *const libc::c_void, 1);
                    libc::close(ready_fd);
                }
                // Remove the env var so it's not inherited by any future child processes
                std::env::remove_var("PG_DOORMAN_READY_FD");
            }
        }

        // Migration receiver is spawned below after tls_acceptor is available

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
        // SIGUSR2 for binary upgrade (unix only; on windows this future never resolves)
        #[cfg(not(windows))]
        let mut upgrade_signal = unix_signal(SignalKind::user_defined2()).unwrap();

        let (exit_tx, mut exit_rx) = mpsc::channel::<()>(1);
        let mut admin_only = false;

        // Detect foreground + TTY mode: SIGINT should only do graceful shutdown (no binary upgrade).
        // PG_DOORMAN_SHUTDOWN_ONLY=1 forces shutdown-only mode for testing in non-TTY environments.
        let is_foreground_tty = {
            #[cfg(not(windows))]
            {
                use std::io::IsTerminal;
                let force_shutdown = std::env::var("PG_DOORMAN_SHUTDOWN_ONLY")
                    .map(|v| v == "1")
                    .unwrap_or(false);
                force_shutdown || (!args.daemon && std::io::stdin().is_terminal())
            }
            #[cfg(windows)]
            {
                false
            }
        };

        let tls_rate_limiter = tls_state.rate_limiter.clone();
        let tls_acceptor = tls_state.acceptor.clone();

        // Spawn migration receiver if parent passed a migration socket
        #[cfg(not(windows))]
        if let Ok(fd_str) = std::env::var("PG_DOORMAN_MIGRATION_FD") {
            if let Ok(migration_fd) = fd_str.parse::<i32>() {
                info!(
                    "Migration socket received from parent (fd={})",
                    migration_fd
                );
                std::env::remove_var("PG_DOORMAN_MIGRATION_FD");
                tokio::spawn(migration_receiver_task(
                    migration_fd,
                    client_server_map.clone(),
                    tls_acceptor.clone(),
                ));
            }
        }

        // Wrap listener in Option to allow dropping it during foreground binary upgrade
        // while still continuing the graceful shutdown process
        let mut listener = Some(listener);

        info!("Accepting connections");
        loop {
            // Create upgrade signal future (SIGUSR2 on unix, never resolves on windows)
            let upgrade_future = async {
                #[cfg(not(windows))]
                {
                    upgrade_signal.recv().await;
                }
                #[cfg(windows)]
                {
                    std::future::pending::<()>().await;
                }
            };

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

                // SIGINT handler:
                // - Foreground + TTY (Ctrl+C): graceful shutdown only (no binary upgrade)
                // - Daemon / no TTY: legacy binary upgrade + graceful shutdown
                _ = interrupt_signal.recv() => {
                    if is_foreground_tty {
                        // Foreground + TTY: graceful shutdown only (no binary upgrade)
                        info!("Got SIGINT (Ctrl+C), starting graceful shutdown");
                        SHUTDOWN_IN_PROGRESS.store(true, Ordering::SeqCst);
                        retain::drain_all_pools();
                        if admin_only { continue; }
                        admin_only = true;
                        spawn_shutdown_timer(exit_tx.clone(), shutdown_timeout);
                        continue;
                    }

                    // Daemon / no TTY: legacy binary upgrade + graceful shutdown
                    #[cfg(not(windows))]
                    {
                        info!("Got SIGINT, starting binary upgrade and graceful shutdown");
                        if !binary_upgrade_and_shutdown(
                            &args, admin_only, &mut listener, shutdown_timeout, &exit_tx,
                        ).await {
                            continue;
                        }
                        admin_only = true;
                    }
                },

                // SIGUSR2: binary upgrade + graceful shutdown (recommended, works in all modes)
                // kill -USR2 $(pgrep pg_doorman)
                _ = upgrade_future => {
                    #[cfg(not(windows))]
                    {
                        info!("Got SIGUSR2, starting binary upgrade and graceful shutdown");
                        if !binary_upgrade_and_shutdown(
                            &args, admin_only, &mut listener, shutdown_timeout, &exit_tx,
                        ).await {
                            continue;
                        }
                        admin_only = true;
                    }
                },

                _ = term_signal.recv() => {
                    let clients_in_tx = CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed);
                    info!("Got SIGTERM, closing with {} clients in transactions", clients_in_tx);
                    break;
                },

                // new client.
                new_client = accept_future => {
                    let (mut socket, addr) = match new_client {
                        Ok((socket, addr)) => (socket, addr),
                        Err(err) => {
                            error!("Failed to accept new connection: {err}");
                            continue;
                        }
                    };
                    if admin_only {
                        warn!("Rejecting connection from {addr}: pooler shutting down");
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
                        let connection_id = TOTAL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed) as u64 + 1;
                        let current_clients = CURRENT_CLIENT_COUNT.fetch_add(1, Ordering::SeqCst);
                        // max clients.
                        if current_clients as u64 > max_connections {
                            warn!("[#c{connection_id}] client {addr} rejected: too many clients (current={current_clients}, max={max_connections})");
                           match crate::client::client_entrypoint_too_many_clients_already(
                                socket, client_server_map).await {
                                Ok(()) => (),
                                Err(err) => {
                                    error!("[#c{connection_id}] client {addr} disconnected with error: {err}");
                                }
                            }
                            CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                            return
                        }
                        let start = Utc::now().naive_utc();

                        match crate::client::client_entrypoint(
                            socket,
                            client_server_map,
                            admin_only,
                            tls_acceptor,
                            tls_rate_limiter,
                            connection_id,
                        )
                        .await
                        {
                            Ok(session_info) => {
                                if log_client_disconnections
                                    || log::log_enabled!(log::Level::Debug)
                                {
                                    let session = format_duration(
                                        &(Utc::now().naive_utc() - start),
                                    );
                                    let identity = match &session_info {
                                        Some(si) => format!("[{}@{} #c{}]", si.username, si.pool_name, si.connection_id),
                                        None => format!("[#c{connection_id}]"),
                                    };
                                    info!("{identity} client disconnected from {addr}, session={session}");
                                }
                            }

                            Err(err) => {
                                // Pre-auth failures: identity unknown, only connection_id available.
                                // Post-auth failures already logged with [user@pool #cN] inside entrypoint.
                                let session = format_duration(&(Utc::now().naive_utc() - start));
                                warn!("[#c{connection_id}] client {addr} disconnected with error: {err}, session={session}");
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

/// Perform binary upgrade (spawn new process) and initiate graceful shutdown.
/// Returns `true` if shutdown was initiated, `false` if upgrade was aborted (e.g. config validation failed).
#[cfg(not(windows))]
async fn binary_upgrade_and_shutdown(
    args: &Args,
    admin_only: bool,
    listener: &mut Option<tokio::net::TcpListener>,
    shutdown_timeout: Duration,
    exit_tx: &mpsc::Sender<()>,
) -> bool {
    // First, validate configuration of the new binary before proceeding with shutdown
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

        info!(
            "Validating configuration with: {} -t {}",
            exe_path, config_file
        );

        let config_test_result = process::Command::new(exe_path)
            .arg("-t")
            .arg(&config_file)
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .output();

        match config_test_result {
            Ok(output) => {
                if !output.status.success() {
                    error!(
                        "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
                    );
                    error!(
                        "!!!                    CRITICAL ERROR                               !!!"
                    );
                    error!(
                        "!!!         CONFIGURATION VALIDATION FAILED                        !!!"
                    );
                    error!(
                        "!!!         BINARY UPGRADE ABORTED - SHUTDOWN CANCELLED            !!!"
                    );
                    error!(
                        "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
                    );
                    error!("");
                    error!("The new binary failed configuration validation!");
                    error!("Configuration file: {}", config_file);
                    error!("Exit code: {:?}", output.status.code());
                    if !output.stderr.is_empty() {
                        error!("Error output: {}", String::from_utf8_lossy(&output.stderr));
                    }
                    if !output.stdout.is_empty() {
                        error!(
                            "Standard output: {}",
                            String::from_utf8_lossy(&output.stdout)
                        );
                    }
                    error!("");
                    error!(
                        "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
                    );
                    error!(
                        "!!!  FIX THE CONFIGURATION BEFORE ATTEMPTING BINARY UPGRADE AGAIN  !!!"
                    );
                    error!(
                        "!!!  THE SERVER WILL CONTINUE RUNNING WITH THE CURRENT BINARY      !!!"
                    );
                    error!(
                        "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
                    );
                    return false;
                }
                info!("Configuration validation successful");
            }
            Err(e) => {
                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                error!("!!!                    CRITICAL ERROR                               !!!");
                error!("!!!         FAILED TO VALIDATE CONFIGURATION                       !!!");
                error!("!!!         BINARY UPGRADE ABORTED - SHUTDOWN CANCELLED            !!!");
                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                error!("");
                error!("Could not execute configuration test: {}", e);
                error!("Binary path: {}", exe_path);
                error!("");
                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                error!("!!!  THE SERVER WILL CONTINUE RUNNING WITH THE CURRENT BINARY      !!!");
                error!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                return false;
            }
        }
    }

    SHUTDOWN_IN_PROGRESS.store(true, Ordering::SeqCst);

    // Drain all idle connections from pools to release PostgreSQL connections
    retain::drain_all_pools();

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
            info!(
                "Starting new process with inherited listener fd={}",
                listener_fd
            );

            // Get current process group to pass to child
            let current_pgid = unsafe { libc::getpgrp() };
            // Create a pipe for readiness signaling
            let mut pipe_fds: [libc::c_int; 2] = [0; 2];
            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
                error!("Failed to create pipe for binary upgrade");
            } else {
                let pipe_read_fd = pipe_fds[0];
                let pipe_write_fd = pipe_fds[1];

                // Create a Unix socketpair for client migration
                let mut migration_fds: [libc::c_int; 2] = [0; 2];
                let migration_ok = unsafe {
                    libc::socketpair(
                        libc::AF_UNIX,
                        libc::SOCK_STREAM,
                        0,
                        migration_fds.as_mut_ptr(),
                    )
                } == 0;
                if !migration_ok {
                    warn!("Failed to create migration socketpair, clients will not be migrated");
                }
                let migration_parent_fd = migration_fds[0]; // kept by old process
                let migration_child_fd = migration_fds[1]; // passed to new process

                // Spawn child process with inherited listener fd, pipe, and migration socket
                let child_result = unsafe {
                    let mut cmd = process::Command::new(exe_path);
                    cmd.args(&exe_args)
                        .arg("--inherit-fd")
                        .arg(listener_fd.to_string())
                        .env("PG_DOORMAN_READY_FD", pipe_write_fd.to_string());
                    if migration_ok {
                        cmd.env("PG_DOORMAN_MIGRATION_FD", migration_child_fd.to_string());
                    }
                    cmd.current_dir(std::env::current_dir().unwrap())
                        .pre_exec(move || {
                            libc::fcntl(listener_fd, libc::F_SETFD, 0);
                            libc::fcntl(pipe_write_fd, libc::F_SETFD, 0);
                            if migration_ok {
                                libc::fcntl(migration_child_fd, libc::F_SETFD, 0);
                            }
                            libc::setpgid(0, current_pgid);
                            Ok(())
                        });
                    cmd.spawn()
                };

                match child_result {
                    Ok(_child) => {
                        unsafe {
                            libc::close(pipe_write_fd);
                            if migration_ok {
                                libc::close(migration_child_fd);
                            }
                        }

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
                        *listener = None;

                        // Start client migration if socketpair was created
                        if migration_ok {
                            let (tx, rx) = mpsc::channel(MIGRATION_CHANNEL_CAPACITY);
                            let _ = MIGRATION_TX.set(tx);
                            MIGRATION_IN_PROGRESS.store(true, Ordering::SeqCst);
                            tokio::spawn(migration_sender_task(migration_parent_fd, rx));
                            info!("Client migration enabled");
                        }

                        info!("Foreground binary upgrade complete, listener released");
                    }
                    Err(e) => {
                        error!("Failed to spawn new process: {}", e);
                        unsafe {
                            libc::close(pipe_read_fd);
                            libc::close(pipe_write_fd);
                            if migration_ok {
                                libc::close(migration_parent_fd);
                                libc::close(migration_child_fd);
                            }
                        }
                    }
                }
            }
        }
    }

    // Don't want this to happen more than once
    if admin_only {
        return true;
    }

    spawn_shutdown_timer(exit_tx.clone(), shutdown_timeout);
    true
}

/// Spawn a task that waits for all clients to disconnect (or timeout) and then signals exit.
fn spawn_shutdown_timer(exit_tx: mpsc::Sender<()>, shutdown_timeout: Duration) {
    tokio::task::spawn(async move {
        let clients_in_tx = CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed);
        info!(
            "waiting for {} client{} in transactions",
            clients_in_tx,
            if clients_in_tx == 1 { "" } else { "s" }
        );

        // Poll frequently to detect client count reaching zero quickly,
        // but enforce the overall shutdown_timeout deadline.
        // Drain idle server connections once per second (not every poll tick)
        // to avoid interfering with binary upgrade readiness.
        let poll_interval = Duration::from_millis(250);
        let mut interval = tokio::time::interval(poll_interval);
        let start = std::time::Instant::now();
        let mut last_drain = std::time::Instant::now();

        loop {
            interval.tick().await;

            if last_drain.elapsed() >= Duration::from_secs(1) {
                retain::drain_all_pools();
                last_drain = std::time::Instant::now();
            }

            let clients_in_tx = CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed);
            let clients_total = CURRENT_CLIENT_COUNT.load(Ordering::Relaxed);
            if clients_total == 0 {
                info!("All clients disconnected, shutting down");
                let _ = exit_tx.send(()).await;
                return;
            }

            if start.elapsed() >= shutdown_timeout {
                error!(
                    "Graceful shutdown timed out. {} active clients in transactions being closed",
                    clients_in_tx
                );
                let _ = exit_tx.send(()).await;
                return;
            }
        }
    });
}
