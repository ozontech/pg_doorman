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
use crate::messages::{configure_tcp_socket, configure_unix_socket};
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
    if args.daemon && std::env::var("NOTIFY_SOCKET").is_ok() {
        warn!(
            "--daemon is incompatible with systemd Type=notify. \
             Remove --daemon from ExecStart or switch to Type=forking."
        );
    }
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

        // Unix socket listener (when unix_socket_dir is set).
        //
        // Delegated to `create_unix_listener` so tests can exercise the
        // bind/chmod/ownership pipeline in a tempdir. `unix_socket_ownership`
        // captures the (dev, ino) of the inode we create here so the
        // shutdown path can tell our socket apart from one bound by a
        // successor process during a SIGUSR2 binary upgrade.
        let (unix_listener, unix_socket_ownership) = match config.general.unix_socket_dir {
            Some(ref dir) => {
                let path = format!("{dir}/.s.PGSQL.{}", config.general.port);
                let mode = crate::config::General::parse_unix_socket_mode(
                    &config.general.unix_socket_mode,
                )
                .expect("unix_socket_mode validated at config load");
                match create_unix_listener(&path, mode) {
                    Ok((listener, ownership)) => {
                        info!("Unix socket listening on {path} (mode={mode:#o})");
                        (Some(listener), Some(ownership))
                    }
                    Err(err) => {
                        error!("{err}");
                        std::process::exit(exitcode::OSERR);
                    }
                }
            }
            None => (None, None),
        };

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
        #[cfg(unix)]
        let mut _migration_handles: Option<MigrationHandles> = None;

        // Detect foreground + TTY mode: SIGINT should only do graceful shutdown (no binary upgrade).
        // PG_DOORMAN_CI_SHUTDOWN_ONLY=1 forces shutdown-only mode for testing in non-TTY environments.
        let is_foreground_tty = {
            #[cfg(not(windows))]
            {
                use std::io::IsTerminal;
                let force_shutdown = std::env::var("PG_DOORMAN_CI_SHUTDOWN_ONLY")
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

        // Notify systemd that the service is ready to accept connections.
        // No-op when NOTIFY_SOCKET is not set (non-systemd environments).
        if let Err(e) = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]) {
            error!("sd_notify READY failed: {e}. If running under systemd Type=notify, the service will not reach active state.");
        }
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
                        match binary_upgrade_and_shutdown(
                            &args, admin_only, &mut listener, shutdown_timeout, &exit_tx,
                        ).await {
                            None => continue,
                            handles => { _migration_handles = handles; }
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
                        match binary_upgrade_and_shutdown(
                            &args, admin_only, &mut listener, shutdown_timeout, &exit_tx,
                        ).await {
                            None => continue,
                            handles => { _migration_handles = handles; }
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
                        if current_clients as u64 > max_connections {
                            warn!("[#c{connection_id}] client {addr} rejected: too many clients (current={current_clients}, max={max_connections})");
                            if let Err(err) = crate::client::client_entrypoint_too_many_clients_already(
                                socket, client_server_map).await {
                                error!("[#c{connection_id}] client {addr} disconnected with error: {err}");
                            }
                            CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                            return;
                        }
                        let start = Utc::now().naive_utc();
                        let result = crate::client::client_entrypoint(
                            socket,
                            client_server_map,
                            admin_only,
                            tls_acceptor,
                            tls_rate_limiter,
                            connection_id,
                        )
                        .await;
                        log_session_end(
                            result,
                            connection_id,
                            &addr.to_string(),
                            start,
                            log_client_disconnections,
                        );
                        CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                    });
                }

                // Unix socket client
                new_unix = async {
                    if let Some(ref l) = unix_listener {
                        l.accept().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    let (socket, _unix_addr) = match new_unix {
                        Ok(pair) => pair,
                        Err(err) => {
                            error!("Failed to accept Unix connection: {err}");
                            continue;
                        }
                    };
                    if admin_only {
                        drop(socket);
                        continue;
                    }
                    configure_unix_socket(&socket);
                    let client_server_map = client_server_map.clone();
                    let config = get_config();
                    let log_client_disconnections = config.general.log_client_disconnections;
                    let max_connections = config.general.max_connections;

                    tokio::task::spawn(async move {
                        let connection_id = TOTAL_CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed) as u64 + 1;
                        let current_clients = CURRENT_CLIENT_COUNT.fetch_add(1, Ordering::SeqCst);
                        if current_clients as u64 > max_connections {
                            warn!("[#c{connection_id}] unix client rejected: too many clients (current={current_clients}, max={max_connections})");
                            if let Err(err) = crate::client::client_entrypoint_too_many_clients_already_unix(
                                socket,
                                connection_id,
                            )
                            .await
                            {
                                warn!("[#c{connection_id}] unix client rejection response failed: {err}");
                            }
                            CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                            return;
                        }
                        let start = Utc::now().naive_utc();
                        let result = crate::client::client_entrypoint_unix(
                            socket,
                            client_server_map,
                            admin_only,
                            connection_id,
                        )
                        .await;
                        log_session_end(
                            result,
                            connection_id,
                            "unix:",
                            start,
                            log_client_disconnections,
                        );
                        CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
                    });
                }

                _ = exit_rx.recv() => {
                    break;
                }

            }
        }
        // Cleanup Unix socket file only if the inode on disk is still the
        // one this process created. During a SIGUSR2 binary upgrade the
        // successor rebinds the same path before we reach this point, so
        // an unconditional unlink here would wipe out the new listener.
        if let Some(ref ownership) = unix_socket_ownership {
            match ownership.cleanup_if_ours() {
                UnixSocketCleanup::Removed => {}
                UnixSocketCleanup::Missing => {}
                UnixSocketCleanup::Skipped { reason } => {
                    info!(
                        "Leaving Unix socket {} in place: {reason}",
                        ownership.path
                    );
                }
                UnixSocketCleanup::Failed { err } => {
                    warn!("Failed to remove Unix socket {}: {err}", ownership.path);
                }
            }
        }

        info!("Shutting down...");

        // Signal migration_sender_task to stop, then wait for it to
        // flush all pending payloads over the Unix socket. Without
        // this, process::exit would kill the sender before it finishes
        // sending data to the new process, losing migrated clients.
        #[cfg(unix)]
        if let Some(handles) = _migration_handles.take() {
            drop(handles.shutdown_tx);
            let _ = handles.sender_handle.await;
            info!("Migration sender finished");
        }

        // Background tokio tasks (stats, retain, prometheus) run in
        // infinite loops — the runtime drop would hang waiting for
        // worker threads to drain them.
        std::process::exit(0);
    });

    Ok(())
}

/// Migration handles returned by binary_upgrade_and_shutdown.
/// Dropping shutdown_tx signals the sender task to exit.
/// Awaiting sender_handle ensures all payloads are flushed to the socket.
#[cfg(not(windows))]
struct MigrationHandles {
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    sender_handle: tokio::task::JoinHandle<()>,
}

/// Perform binary upgrade (spawn new process) and initiate graceful shutdown.
/// Returns None if upgrade was aborted (e.g. config validation failed).
/// Returns Some(MigrationHandles) if upgrade started with client migration.
#[cfg(not(windows))]
async fn binary_upgrade_and_shutdown(
    args: &Args,
    admin_only: bool,
    listener: &mut Option<tokio::net::TcpListener>,
    shutdown_timeout: Duration,
    exit_tx: &mpsc::Sender<()>,
) -> Option<MigrationHandles> {
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
                    return None;
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
                return None;
            }
        }
    }

    // Set MIGRATION_IN_PROGRESS before SHUTDOWN_IN_PROGRESS so that
    // idle clients in the handle() loop see migration=true and wait
    // to migrate instead of disconnecting with "pooler is shut down".
    // If the upgrade fails later (spawn error), we clear the flag.
    if !admin_only {
        MIGRATION_IN_PROGRESS.store(true, Ordering::Relaxed);
    }
    SHUTDOWN_IN_PROGRESS.store(true, Ordering::SeqCst);

    let mut migration_handles: Option<MigrationHandles> = None;

    // Drain idle server connections — but only when there is no client
    // migration. During migration, in-transaction clients still need
    // their server connections to finish the current query and COMMIT.
    // Draining here would mark those servers bad, causing "pooler is
    // shut down" errors before clients get a chance to migrate.
    if admin_only {
        retain::drain_all_pools();
    }

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
                    Ok(child) => {
                        let child_pid = child.id();
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

                            // Tell systemd the new process is now the main PID.
                            // systemd stops tracking the old process and won't
                            // restart the service when we exit.
                            if let Err(e) = sd_notify::notify(
                                false,
                                &[sd_notify::NotifyState::MainPid(child_pid)],
                            ) {
                                warn!("sd_notify MAINPID failed: {e}. systemd may restart the service after old process exits.");
                            }
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
                            // MIGRATION_IN_PROGRESS already set above (before SHUTDOWN)
                            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
                            let sender_handle = tokio::spawn(migration_sender_task(
                                migration_parent_fd,
                                rx,
                                shutdown_rx,
                            ));
                            migration_handles = Some(MigrationHandles {
                                shutdown_tx,
                                sender_handle,
                            });
                            info!("Client migration enabled");
                        }

                        info!("Foreground binary upgrade complete, listener released");
                    }
                    Err(e) => {
                        error!("Failed to spawn new process: {}", e);
                        MIGRATION_IN_PROGRESS.store(false, Ordering::Relaxed);
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
        return migration_handles;
    }

    spawn_shutdown_timer(exit_tx.clone(), shutdown_timeout);
    migration_handles
}

/// Spawn a task that waits for all clients to disconnect (or timeout) and then signals exit.
fn spawn_shutdown_timer(exit_tx: mpsc::Sender<()>, shutdown_timeout: Duration) {
    tokio::task::spawn(async move {
        let clients_in_tx = CLIENTS_IN_TRANSACTIONS.load(Ordering::Relaxed);
        let clients_total = CURRENT_CLIENT_COUNT.load(Ordering::Relaxed);
        info!(
            "waiting for {} client{} to disconnect ({} in transactions)",
            clients_total,
            if clients_total == 1 { "" } else { "s" },
            clients_in_tx
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

            // Only drain pools when NOT migrating. During migration,
            // in-transaction clients need their server connections.
            if !MIGRATION_IN_PROGRESS.load(Ordering::Relaxed)
                && last_drain.elapsed() >= Duration::from_secs(1)
            {
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
                    "Graceful shutdown timed out. {} client{} remain ({} in transactions), closing forcibly",
                    clients_total,
                    if clients_total == 1 { "" } else { "s" },
                    clients_in_tx
                );
                let _ = exit_tx.send(()).await;
                return;
            }
        }
    });
}

/// Identity of a Unix socket file this process bound to, captured as
/// `(dev, ino)` plus the original path. Used to decide at shutdown whether
/// the inode on disk is still ours or has been replaced by a successor
/// process during a binary upgrade.
#[cfg(unix)]
#[derive(Debug, Clone)]
struct UnixSocketOwnership {
    path: String,
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum UnixSocketCleanup {
    /// The inode matched; the file has been removed.
    Removed,
    /// Nothing was on disk at the captured path.
    Missing,
    /// A different inode sits at the path — a successor rebound it.
    Skipped { reason: String },
    /// Removal was attempted but the syscall returned an error.
    Failed { err: String },
}

#[cfg(unix)]
impl UnixSocketOwnership {
    /// Stat the path and remember `(dev, ino)` so future cleanup can verify
    /// the inode has not been replaced.
    fn capture(path: &str) -> Result<Self, std::io::Error> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            path: path.to_string(),
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }

    /// Remove the socket file only if the inode on disk still matches the
    /// one captured at `capture` time.
    fn cleanup_if_ours(&self) -> UnixSocketCleanup {
        match Self::inspect(&self.path, self.dev, self.ino) {
            CleanupDecision::Remove => match std::fs::remove_file(&self.path) {
                Ok(()) => UnixSocketCleanup::Removed,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    UnixSocketCleanup::Missing
                }
                Err(err) => UnixSocketCleanup::Failed {
                    err: err.to_string(),
                },
            },
            CleanupDecision::Missing => UnixSocketCleanup::Missing,
            CleanupDecision::Skip(reason) => UnixSocketCleanup::Skipped { reason },
        }
    }

    /// Pure decision function: given a path and the expected `(dev, ino)`,
    /// should the caller proceed to unlink the file? Split out so the logic
    /// can be unit-tested without touching real filesystem state.
    fn inspect(path: &str, expected_dev: u64, expected_ino: u64) -> CleanupDecision {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                let dev = meta.dev();
                let ino = meta.ino();
                if dev == expected_dev && ino == expected_ino {
                    CleanupDecision::Remove
                } else {
                    CleanupDecision::Skip(format!(
                        "inode changed (expected dev={expected_dev} ino={expected_ino}, found dev={dev} ino={ino}); another process owns the path now"
                    ))
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => CleanupDecision::Missing,
            Err(err) => CleanupDecision::Skip(format!("stat failed: {err}")),
        }
    }
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum CleanupDecision {
    Remove,
    Missing,
    Skip(String),
}

/// Log the end of a client session using a shared format string. Both the
/// TCP and Unix accept branches used to inline the same match on
/// `Result<Option<ClientSessionInfo>, Error>` — same identity string,
/// same elapsed-time rendering, same warn vs info split. Centralising
/// it keeps the two remaining accept sites down to a single call.
fn log_session_end(
    result: Result<Option<crate::client::ClientSessionInfo>, crate::errors::Error>,
    connection_id: u64,
    peer_label: &str,
    session_start: chrono::NaiveDateTime,
    log_disconnections: bool,
) {
    let session = format_duration(&(Utc::now().naive_utc() - session_start));
    match result {
        Ok(session_info) => {
            if log_disconnections || log::log_enabled!(log::Level::Debug) {
                let identity = match &session_info {
                    Some(si) => {
                        format!("[{}@{} #c{}]", si.username, si.pool_name, si.connection_id)
                    }
                    None => format!("[#c{connection_id}]"),
                };
                info!("{identity} client disconnected from {peer_label}, session={session}");
            }
        }
        Err(err) => {
            // Pre-auth failures: identity unknown, only connection_id available.
            // Post-auth failures already logged with [user@pool #cN] inside entrypoint.
            warn!("[#c{connection_id}] client {peer_label} disconnected with error: {err}, session={session}");
        }
    }
}

/// Create a Tokio Unix socket listener at `path` with the given permission
/// `mode`.
///
/// This is the whole bring-up sequence the pooler runs at startup, factored
/// out of `run_server` so unit tests can reproduce the failure modes (stale
/// file, dead-end directory, chmod failure) in a tempdir without launching a
/// full server. On success the returned [`UnixSocketOwnership`] records the
/// (dev, ino) of the inode so the shutdown path can decide whether the
/// successor of a binary upgrade has already replaced it.
#[cfg(unix)]
fn create_unix_listener(
    path: &str,
    mode: u32,
) -> Result<(tokio::net::UnixListener, UnixSocketOwnership), String> {
    prepare_unix_socket_path(path)
        .map_err(|err| format!("Cannot reuse Unix socket path {path}: {err}"))?;

    // Clamp the umask so the socket inode created by bind() never exists with
    // weaker permissions than `mode`. Without this a concurrent client
    // connecting in the window between bind() and set_permissions() would
    // land on the umask-derived rights (typically 0644) and bypass the
    // configured restriction. set_permissions() still runs afterwards so
    // callers can loosen the mode (e.g. 0660 with a group bit).
    let restrict_bits = !(mode & 0o777) & 0o777;
    let _umask_guard = UmaskGuard::restrict(restrict_bits as libc::mode_t);

    let listener = tokio::net::UnixListener::bind(path)
        .map_err(|err| format!("Failed to bind Unix socket {path}: {err}"))?;
    drop(_umask_guard);

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|err| format!("Failed to set mode {mode:#o} on Unix socket {path}: {err}"))?;

    let ownership = UnixSocketOwnership::capture(path)
        .map_err(|err| format!("Failed to stat Unix socket {path} after bind: {err}"))?;

    Ok((listener, ownership))
}

/// Prepare a Unix socket path for bind() by clearing any stale file without
/// clobbering a live peer.
///
/// The previous implementation called `remove_file` unconditionally, which
/// meant pointing pg_doorman at a shared directory like `/var/run/postgresql`
/// could silently delete another process's live socket. This helper instead:
///
/// 1. Returns Ok if nothing exists at the path.
/// 2. Attempts a connect — if it succeeds, a live peer owns the socket and
///    we refuse to touch it so the caller can fail loudly.
/// 3. Otherwise removes the stale inode (typical case after a crash).
///
/// Errors are returned as strings with enough context for the caller to log
/// and exit; unit tests exercise the three branches without touching the
/// process umask or the real server bring-up.
#[cfg(unix)]
fn prepare_unix_socket_path(path: &str) -> Result<(), String> {
    use std::os::unix::net::UnixStream;

    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("stat failed: {err}")),
    }

    // Short probe: a local Unix connect that would succeed does so in
    // microseconds. If it refuses, the socket is stale (no listener bound to
    // the inode) and we can reclaim it.
    match UnixStream::connect(path) {
        Ok(_) => Err(format!(
            "another process is already listening on {path}; refusing to remove it"
        )),
        Err(_) => std::fs::remove_file(path)
            .map_err(|err| format!("failed to remove stale socket {path}: {err}")),
    }
}

/// Temporarily tighten the process umask for the lifetime of the guard.
///
/// The Unix listener startup needs the socket inode to be created with no
/// weaker permissions than the configured `unix_socket_mode`. Since `bind()`
/// applies `0666 & ~umask` at the moment the file appears in the filesystem,
/// we ratchet the umask up, perform the bind, then restore the original
/// value on drop. The guard is also safe to drop explicitly once the socket
/// is in place and `set_permissions` has run.
#[cfg(unix)]
struct UmaskGuard {
    previous: libc::mode_t,
}

#[cfg(unix)]
impl UmaskGuard {
    /// Ensure the process umask masks at least `additional_bits` on top of
    /// whatever was already set.
    fn restrict(additional_bits: libc::mode_t) -> Self {
        // SAFETY: umask is a process-global knob; we snapshot the current
        // value by setting a known mask, OR in our extra bits, and restore
        // it on drop. No Rust invariants are touched.
        let previous = unsafe { libc::umask(0o777) };
        unsafe { libc::umask(previous | additional_bits) };
        Self { previous }
    }
}

#[cfg(unix)]
impl Drop for UmaskGuard {
    fn drop(&mut self) {
        // SAFETY: same rationale as `restrict`; we only touch the umask.
        unsafe { libc::umask(self.previous) };
    }
}

#[cfg(test)]
mod create_unix_listener_tests {
    use super::create_unix_listener;
    use serial_test::serial;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    // Serialised because the umask_guard_tests in this crate flip the
    // process umask to 0o777 while running; any concurrent tempdir-backed
    // bind() would land on an inaccessible file and fail with EACCES.
    #[tokio::test]
    #[serial]
    async fn binds_and_applies_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".s.PGSQL.6432");
        let path_str = path.to_str().unwrap();

        let (listener, ownership) =
            create_unix_listener(path_str, 0o600).expect("bind must succeed in empty tempdir");

        let meta = std::fs::metadata(path_str).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(ownership.path, path_str);

        drop(listener);
    }

    #[tokio::test]
    #[serial]
    async fn bind_fails_when_directory_missing() {
        // Directory we never created → bind must return a structured error
        // instead of panicking or exiting the process.
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("does")
            .join("not")
            .join("exist")
            .join(".s.PGSQL.6432");

        let err = create_unix_listener(path.to_str().unwrap(), 0o600)
            .expect_err("bind must fail when parent directory is missing");
        assert!(err.contains("Failed to bind"), "unexpected error: {err}");
    }

    #[tokio::test]
    #[serial]
    async fn group_readable_mode_is_applied() {
        // 0660 exercises the path where set_permissions *loosens* the bits
        // the umask guard masked off; if we mess that up the file stays
        // owner-only and client groups lose access silently.
        let dir = tempdir().unwrap();
        let path = dir.path().join(".s.PGSQL.6432");

        let (listener, _ownership) =
            create_unix_listener(path.to_str().unwrap(), 0o660).expect("bind must succeed");

        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o660);
        drop(listener);
    }
}

#[cfg(test)]
mod unix_socket_ownership_tests {
    use super::{CleanupDecision, UnixSocketCleanup, UnixSocketOwnership};
    use serial_test::serial;
    use std::os::unix::net::UnixListener;
    use tempfile::tempdir;

    #[test]
    #[serial]
    fn capture_and_cleanup_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("owned.sock");
        let _listener = UnixListener::bind(&path).unwrap();

        let ownership = UnixSocketOwnership::capture(path.to_str().unwrap())
            .expect("capture must succeed right after bind");
        assert_eq!(ownership.cleanup_if_ours(), UnixSocketCleanup::Removed);
        assert!(!path.exists(), "our socket file must be removed");
    }

    #[test]
    #[serial]
    fn cleanup_skips_replaced_inode() {
        // Linux is free to recycle a freed inode immediately on tmpfs/ext4,
        // so bind→remove→bind on the same path can land on the same ino on
        // CI runners. We forge the mismatch directly: a stale ownership
        // claim against a live file is the same observable state the parent
        // would see after a successor rebound the socket.
        let dir = tempdir().unwrap();
        let path = dir.path().join("shared.sock");
        let live = UnixListener::bind(&path).unwrap();
        let real = UnixSocketOwnership::capture(path.to_str().unwrap()).unwrap();
        let stale = UnixSocketOwnership {
            path: real.path.clone(),
            dev: real.dev,
            ino: real.ino.wrapping_add(1),
        };

        match stale.cleanup_if_ours() {
            UnixSocketCleanup::Skipped { reason } => {
                assert!(
                    reason.contains("inode changed"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
        assert!(path.exists(), "live socket file must be preserved");
        drop(live);
    }

    #[test]
    #[serial]
    fn cleanup_reports_missing_when_already_removed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("gone.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let ownership = UnixSocketOwnership::capture(path.to_str().unwrap()).unwrap();

        std::fs::remove_file(&path).unwrap();
        assert_eq!(ownership.cleanup_if_ours(), UnixSocketCleanup::Missing);
    }

    #[test]
    #[serial]
    fn inspect_remove_on_exact_match() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("inspect.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let ownership = UnixSocketOwnership::capture(path.to_str().unwrap()).unwrap();

        assert_eq!(
            UnixSocketOwnership::inspect(path.to_str().unwrap(), ownership.dev, ownership.ino),
            CleanupDecision::Remove
        );
    }

    #[test]
    #[serial]
    fn inspect_skip_on_mismatched_ino() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("inspect2.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let ownership = UnixSocketOwnership::capture(path.to_str().unwrap()).unwrap();

        // Pretend we captured a different inode to simulate replacement.
        let fake_ino = ownership.ino.wrapping_add(1);
        match UnixSocketOwnership::inspect(path.to_str().unwrap(), ownership.dev, fake_ino) {
            CleanupDecision::Skip(reason) => {
                assert!(reason.contains("inode changed"), "unexpected: {reason}");
            }
            other => panic!("expected Skip, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn inspect_missing_when_no_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.sock");
        assert_eq!(
            UnixSocketOwnership::inspect(path.to_str().unwrap(), 0, 0),
            CleanupDecision::Missing
        );
    }
}

#[cfg(test)]
mod prepare_unix_socket_path_tests {
    use super::prepare_unix_socket_path;
    use serial_test::serial;
    use std::os::unix::net::UnixListener;
    use tempfile::tempdir;

    #[test]
    #[serial]
    fn missing_path_is_ok() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.sock");
        assert!(prepare_unix_socket_path(path.to_str().unwrap()).is_ok());
    }

    #[test]
    #[serial]
    fn stale_file_is_removed() {
        // A regular file (not a live listener) simulates a post-crash leftover
        // — prepare_unix_socket_path should clean it up silently.
        let dir = tempdir().unwrap();
        let path = dir.path().join("stale.sock");
        std::fs::write(&path, b"leftover").unwrap();
        assert!(path.exists());

        prepare_unix_socket_path(path.to_str().unwrap()).expect("stale file must be removable");
        assert!(!path.exists(), "stale socket file must be removed");
    }

    #[test]
    #[serial]
    fn live_listener_is_preserved() {
        // Bind a real UnixListener in a temp dir; the helper must refuse to
        // touch it and return a descriptive error.
        let dir = tempdir().unwrap();
        let path = dir.path().join("live.sock");
        let _listener = UnixListener::bind(&path).unwrap();

        let err = prepare_unix_socket_path(path.to_str().unwrap())
            .expect_err("live socket must trigger an error");
        assert!(err.contains("already listening"), "unexpected error: {err}");
        assert!(path.exists(), "live socket file must stay on disk");
    }
}

#[cfg(test)]
mod umask_guard_tests {
    use super::UmaskGuard;
    use serial_test::serial;

    #[test]
    #[serial]
    fn restore_previous_umask_on_drop() {
        let prior = unsafe { libc::umask(0o022) };
        {
            let _guard = UmaskGuard::restrict(0o077);
            let inside = unsafe { libc::umask(0o777) };
            unsafe { libc::umask(inside) };
            assert_eq!(
                inside & 0o077,
                0o077,
                "guard must ensure the restrict bits are set"
            );
        }
        let after = unsafe { libc::umask(0o022) };
        assert_eq!(after, 0o022, "drop must restore the original umask");
        unsafe { libc::umask(prior) };
    }

    #[test]
    #[serial]
    fn restrict_preserves_existing_bits() {
        let prior = unsafe { libc::umask(0o027) };
        {
            let _guard = UmaskGuard::restrict(0o050);
            let inside = unsafe { libc::umask(0o777) };
            unsafe { libc::umask(inside) };
            // Prior bits (027) AND new bits (050) must both be present.
            assert_eq!(inside & 0o077, 0o077);
        }
        unsafe { libc::umask(prior) };
    }
}
