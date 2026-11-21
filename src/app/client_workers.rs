use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::Utc;
use log::info;
use tokio::runtime::Builder;
use tokio::sync::mpsc;

use crate::app::server::{log_session_end, CURRENT_CLIENT_COUNT};
use crate::config::General;
use crate::pool::ClientServerMap;
use crate::utils::core_affinity;
use crate::utils::rate_limit::RateLimiter;

thread_local! {
    static CLIENT_WORKER_ID: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

pub fn current_worker_id() -> Option<usize> {
    CLIENT_WORKER_ID.get()
}

pub enum ClientConnection {
    Tcp {
        stream: std::net::TcpStream,
        client_server_map: ClientServerMap,
        admin_only: bool,
        tls_acceptor: Option<tokio_native_tls::TlsAcceptor>,
        tls_rate_limiter: Option<RateLimiter>,
        connection_id: u64,
        peer: String,
    },
    #[cfg(not(windows))]
    Unix {
        stream: std::os::unix::net::UnixStream,
        client_server_map: ClientServerMap,
        admin_only: bool,
        connection_id: u64,
    },
    Task(Pin<Box<dyn Future<Output = ()> + Send + 'static>>),
}

#[derive(Clone)]
pub struct ClientDispatcher {
    workers: Arc<Vec<mpsc::UnboundedSender<ClientConnection>>>,
    next: Arc<AtomicUsize>,
}

impl ClientDispatcher {
    pub fn new(config: &General) -> Result<Self, Box<dyn std::error::Error>> {
        if config.worker_threads == 0 {
            return Err("general.worker_threads must be greater than zero".into());
        }

        let core_ids = core_affinity::get_core_ids().unwrap_or_default();
        let pin_workers = config.worker_cpu_affinity_pinning && core_ids.len() >= 3;
        let mut workers = Vec::with_capacity(config.worker_threads);

        for worker_id in 0..config.worker_threads {
            let (tx, rx) = mpsc::unbounded_channel();
            let runtime = client_runtime(config)?;
            let mut thread =
                std::thread::Builder::new().name(format!("client-pg-doorman-{worker_id}"));
            if let Some(ref stack_size) = config.worker_stack_size {
                thread = thread.stack_size(stack_size.as_usize());
            }
            let core_id = core_ids.get(worker_id % core_ids.len().max(1)).copied();
            thread.spawn(move || {
                CLIENT_WORKER_ID.set(Some(worker_id));
                if pin_workers {
                    if let Some(core_id) = core_id {
                        core_affinity::set_for_current(core_id);
                    }
                }
                tokio::task::LocalSet::new().block_on(&runtime, worker_loop(rx));
            })?;
            workers.push(tx);
        }

        info!(
            "Thread-per-core client dispatcher started with {} workers",
            workers.len()
        );
        Ok(Self {
            workers: Arc::new(workers),
            next: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn dispatch(&self, connection: ClientConnection) -> Result<(), ClientConnection> {
        let worker_id = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        self.workers[worker_id]
            .send(connection)
            .map_err(|err| err.0)
    }
}

fn client_runtime(config: &General) -> Result<tokio::runtime::Runtime, std::io::Error> {
    let mut builder = Builder::new_current_thread();
    builder.enable_all();
    if let Some(interval) = config.tokio_global_queue_interval {
        builder.global_queue_interval(interval);
    }
    if let Some(interval) = config.tokio_event_interval {
        builder.event_interval(interval);
    }
    if let Some(ref stack_size) = config.worker_stack_size {
        builder.thread_stack_size(stack_size.as_usize());
    }
    if let Some(max_threads) = config.max_blocking_threads {
        builder.max_blocking_threads(max_threads);
    }
    builder.build()
}

pub fn spawn_client(
    dispatcher: &Option<ClientDispatcher>,
    task: impl Future<Output = ()> + Send + 'static,
) {
    if let Some(dispatcher) = dispatcher {
        if dispatcher
            .dispatch(ClientConnection::Task(Box::pin(task)))
            .is_err()
        {
            log::error!("failed to dispatch client");
        }
    } else {
        tokio::spawn(task);
    }
}

async fn worker_loop(mut rx: mpsc::UnboundedReceiver<ClientConnection>) {
    while let Some(connection) = rx.recv().await {
        tokio::task::spawn_local(run_client(connection));
    }
}

async fn run_client(connection: ClientConnection) {
    let started_at = Utc::now().naive_utc();
    let log_disconnections = crate::config::get_config()
        .general
        .log_client_disconnections;

    let (result, connection_id, peer) = match connection {
        ClientConnection::Tcp {
            stream,
            client_server_map,
            admin_only,
            tls_acceptor,
            tls_rate_limiter,
            connection_id,
            peer,
        } => {
            let result = match tokio::net::TcpStream::from_std(stream) {
                Ok(stream) => {
                    crate::client::client_entrypoint(
                        stream,
                        client_server_map,
                        admin_only,
                        tls_acceptor,
                        tls_rate_limiter,
                        connection_id,
                    )
                    .await
                }
                Err(err) => Err(crate::errors::Error::SocketError(format!(
                    "client worker from_std: {err}"
                ))),
            };
            (result, connection_id, peer)
        }
        #[cfg(not(windows))]
        ClientConnection::Unix {
            stream,
            client_server_map,
            admin_only,
            connection_id,
        } => {
            let result = match tokio::net::UnixStream::from_std(stream) {
                Ok(stream) => {
                    crate::client::client_entrypoint_unix(
                        stream,
                        client_server_map,
                        admin_only,
                        connection_id,
                    )
                    .await
                }
                Err(err) => Err(crate::errors::Error::SocketError(format!(
                    "unix client worker from_std: {err}"
                ))),
            };
            (result, connection_id, "unix:".to_string())
        }
        ClientConnection::Task(task) => {
            task.await;
            return;
        }
    };

    log_session_end(result, connection_id, &peer, started_at, log_disconnections);
    CURRENT_CLIENT_COUNT.fetch_add(-1, Ordering::SeqCst);
}
