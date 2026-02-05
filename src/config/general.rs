//! General configuration settings for the connection pooler.

use bytes::{BufMut, BytesMut};
use ipnet::IpNet;
use serde_derive::{Deserialize, Serialize};
use std::mem;

use super::tls;
use super::{ByteSize, Duration, Include};
use crate::auth::hba::PgHba;

/// General configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct General {
    #[serde(default = "General::default_host")]
    pub host: String,

    #[serde(default = "General::default_port")]
    pub port: u16,

    #[serde(default)]
    pub tokio_global_queue_interval: Option<u32>,

    #[serde(default)]
    pub tokio_event_interval: Option<u32>,

    #[serde(default = "General::default_connect_timeout")]
    pub connect_timeout: Duration,

    #[serde(default = "General::default_query_wait_timeout")]
    pub query_wait_timeout: Duration,

    #[serde(default = "General::default_idle_timeout")]
    pub idle_timeout: Duration,

    #[serde(default = "General::default_tcp_keepalives_idle")]
    pub tcp_keepalives_idle: u64,
    #[serde(default = "General::default_tcp_keepalives_count")]
    pub tcp_keepalives_count: u32,
    #[serde(default = "General::default_tcp_keepalives_interval")]
    pub tcp_keepalives_interval: u64,
    #[serde(default = "General::default_tcp_so_linger")]
    pub tcp_so_linger: u64,
    #[serde(default = "General::default_tcp_no_delay")]
    pub tcp_no_delay: bool,

    #[serde(default = "General::default_unix_socket_buffer_size")]
    pub unix_socket_buffer_size: ByteSize,

    #[serde(default)] // True
    pub log_client_connections: bool,

    #[serde(default)] // True
    pub log_client_disconnections: bool,

    #[serde(default = "General::default_shutdown_timeout")] // 10_000
    pub shutdown_timeout: Duration,

    #[serde(default = "General::default_message_size_to_be_stream")] // 1024 * 1024
    pub message_size_to_be_stream: ByteSize,

    #[serde(default = "General::default_max_memory_usage")] // 256m
    pub max_memory_usage: ByteSize,

    #[serde(default = "General::default_max_connections")]
    pub max_connections: u64,

    /// Maximum number of server connections that can be created concurrently.
    /// Uses a semaphore to limit parallel connection creation instead of serializing with mutex.
    #[serde(default = "General::default_max_concurrent_creates")]
    pub max_concurrent_creates: usize,

    #[serde(default = "General::default_server_lifetime")]
    pub server_lifetime: Duration,

    #[serde(default = "General::default_retain_connections_time")]
    pub retain_connections_time: Duration,

    /// Maximum number of idle connections to close per retain cycle.
    /// 0 means unlimited (close all idle connections that exceed timeout).
    /// Default: 0 (unlimited)
    #[serde(default = "General::default_retain_connections_max")]
    pub retain_connections_max: usize,

    #[serde(default = "General::default_server_round_robin")] // False
    pub server_round_robin: bool,

    #[serde(default = "General::default_sync_server_parameters")] // False
    pub sync_server_parameters: bool,

    #[serde(default = "General::default_worker_threads")]
    pub worker_threads: usize,

    #[serde(default = "General::default_proxy_copy_data_timeout")] // 15_000
    pub proxy_copy_data_timeout: Duration,

    // worker_cpu_affinity_pinning: пытаемся пинить каждый worker на CPU, начиная со второго CPU.
    #[serde(default = "General::default_worker_cpu_affinity_pinning")]
    pub worker_cpu_affinity_pinning: bool,
    // worker_stack_size: размера стэка каждого воркера.
    #[serde(default)]
    pub worker_stack_size: Option<ByteSize>,
    // max_blocking_threads: максимальное количество блокирующих потоков tokio.
    #[serde(default)]
    pub max_blocking_threads: Option<usize>,
    // tcp backlog.
    #[serde(default = "General::default_backlog")]
    pub backlog: u32,

    // pooler_check_query: ping pooler with simple query like '/* ping pooler */;'.
    #[serde(default = "General::default_pooler_check_query")]
    pub pooler_check_query: String,
    pooler_check_query_request_bytes: Option<Vec<u8>>,

    pub tls_certificate: Option<String>,
    pub tls_private_key: Option<String>,
    pub tls_ca_cert: Option<String>,
    pub tls_mode: Option<String>,
    #[serde(default = "General::default_tls_rate_limit_per_second")]
    pub tls_rate_limit_per_second: usize,

    #[serde(default)] // false
    pub server_tls: bool,

    #[serde(default)] // false
    pub verify_server_certificate: bool,

    pub admin_username: String,
    pub admin_password: String,

    #[serde(default = "General::default_prepared_statements")]
    pub prepared_statements: bool,

    #[serde(default = "General::default_prepared_statements_cache_size")]
    pub prepared_statements_cache_size: usize,

    /// Maximum number of prepared statements cached per client connection.
    /// This is a protection against malicious clients that don't call DEALLOCATE
    /// and could cause memory exhaustion by creating unlimited prepared statements.
    /// When the limit is reached, the oldest (least recently added) statement is evicted.
    /// Default: 0 (unlimited - no protection, relies on client calling DEALLOCATE)
    #[serde(default = "General::default_client_prepared_statements_cache_size")]
    pub client_prepared_statements_cache_size: usize,

    #[serde(default = "General::default_daemon_pid_file")]
    pub daemon_pid_file: String, // can be enabled only in daemon mode.

    pub syslog_prog_name: Option<String>,

    #[serde(
        default = "General::default_hba",
        skip_serializing_if = "<[_]>::is_empty"
    )]
    pub hba: Vec<IpNet>,

    // New pg_hba rules: either inline content or a file path (see `PgHba` deserialization).
    #[serde(default, skip_serializing)]
    pub pg_hba: Option<PgHba>,
}

impl General {
    pub fn default_host() -> String {
        "0.0.0.0".into()
    }

    pub fn default_port() -> u16 {
        5432
    }

    pub fn default_tls_rate_limit_per_second() -> usize {
        0
    }
    pub fn default_server_lifetime() -> Duration {
        Duration::from_mins(5) // 5 min
    }

    pub fn default_retain_connections_time() -> Duration {
        Duration::from_secs(60) // 60 seconds
    }

    pub fn default_retain_connections_max() -> usize {
        0 // unlimited
    }

    pub fn default_connect_timeout() -> Duration {
        Duration::from_millis(3_000)
    }

    pub fn default_query_wait_timeout() -> Duration {
        Duration::from_millis(5000)
    }

    pub fn default_tcp_so_linger() -> u64 {
        0 // 0 seconds
    }

    pub fn default_unix_socket_buffer_size() -> ByteSize {
        ByteSize::from_mb(1) // 1mb
    }

    pub fn default_worker_cpu_affinity_pinning() -> bool {
        false
    }

    pub fn default_max_memory_usage() -> ByteSize {
        ByteSize::from_mb(256) // 256mb
    }

    pub fn default_max_connections() -> u64 {
        8 * 1024
    }

    /// Default maximum number of concurrent server connection creates.
    /// Allows up to 4 connections to be created in parallel per pool.
    pub fn default_max_concurrent_creates() -> usize {
        4
    }

    pub fn default_backlog() -> u32 {
        0
    }

    pub fn default_tcp_no_delay() -> bool {
        true
    }

    pub fn default_sync_server_parameters() -> bool {
        false
    }

    // These keepalive defaults should detect a dead connection within 30 seconds.
    // Tokio defaults to disabling keepalives which keeps dead connections around indefinitely.
    // This can lead to permanent server pool exhaustion
    pub fn default_tcp_keepalives_idle() -> u64 {
        5 // 5 seconds
    }

    pub fn default_tcp_keepalives_count() -> u32 {
        5 // 5 time
    }

    pub fn default_tcp_keepalives_interval() -> u64 {
        5 // 5 seconds
    }

    pub fn default_idle_timeout() -> Duration {
        Duration::from_millis(300_000_000) // 5000 minutes
    }

    pub fn default_shutdown_timeout() -> Duration {
        Duration::from_secs(10) // 10 seconds
    }

    pub fn default_proxy_copy_data_timeout() -> Duration {
        Duration::from_secs(15) // 15 seconds
    }

    pub fn default_message_size_to_be_stream() -> ByteSize {
        ByteSize::from_mb(1) // 1mb
    }

    pub fn default_worker_threads() -> usize {
        4
    }

    pub fn default_server_round_robin() -> bool {
        false
    }

    pub fn default_prepared_statements_cache_size() -> usize {
        8 * 1024
    }
    pub fn default_prepared_statements() -> bool {
        true
    }
    /// Default: 0 (unlimited - no protection against malicious clients)
    pub fn default_client_prepared_statements_cache_size() -> usize {
        0
    }

    pub fn default_daemon_pid_file() -> String {
        "/tmp/pg_doorman.pid".to_string()
    }

    pub fn default_pooler_check_query() -> String {
        ";".to_string()
    }

    pub fn poller_check_query_request_bytes_vec(&self) -> Vec<u8> {
        if let Some(ref bytes) = self.pooler_check_query_request_bytes {
            return bytes.clone();
        }
        let mut buf = BytesMut::from(&b"Q"[..]);
        buf.put_i32(self.pooler_check_query.len() as i32 + mem::size_of::<i32>() as i32 + 1);
        buf.put_slice(self.pooler_check_query.as_bytes());
        buf.put_u8(b'\0');
        buf.to_vec()
    }

    pub fn default_hba() -> Vec<IpNet> {
        vec![]
    }

    pub fn default_include_files() -> Vec<String> {
        vec![]
    }

    pub fn default_include() -> Include {
        Include {
            files: Self::default_include_files(),
        }
    }

    pub fn only_ssl_connections(&self) -> bool {
        self.tls_mode
            .as_ref()
            .map(|mode| tls::TLSMode::from_string(mode.as_str()))
            .is_some_and(|result| match result {
                Ok(tls_mode) => {
                    match tls_mode {
                        tls::TLSMode::VerifyFull | tls::TLSMode::Require => true,
                        _ => false, // allow non-ssl connections
                    }
                }
                Err(_) => false,
            })
    }
}

impl Default for General {
    fn default() -> General {
        General {
            host: Self::default_host(),
            port: Self::default_port(),
            tokio_global_queue_interval: None,
            tokio_event_interval: None,
            connect_timeout: General::default_connect_timeout(),
            query_wait_timeout: General::default_query_wait_timeout(),
            idle_timeout: General::default_idle_timeout(),
            shutdown_timeout: Self::default_shutdown_timeout(),
            proxy_copy_data_timeout: Self::default_proxy_copy_data_timeout(),
            message_size_to_be_stream: Self::default_message_size_to_be_stream(),
            max_memory_usage: Self::default_max_memory_usage(),
            max_connections: Self::default_max_connections(),
            max_concurrent_creates: Self::default_max_concurrent_creates(),
            worker_threads: Self::default_worker_threads(),
            worker_cpu_affinity_pinning: Self::default_worker_cpu_affinity_pinning(),
            worker_stack_size: None,
            max_blocking_threads: None,
            tcp_keepalives_idle: Self::default_tcp_keepalives_idle(),
            tcp_keepalives_count: Self::default_tcp_keepalives_count(),
            tcp_keepalives_interval: Self::default_tcp_keepalives_interval(),
            tcp_so_linger: Self::default_tcp_so_linger(),
            tcp_no_delay: Self::default_tcp_no_delay(),
            unix_socket_buffer_size: Self::default_unix_socket_buffer_size(),
            log_client_connections: true,
            log_client_disconnections: true,
            sync_server_parameters: Self::default_sync_server_parameters(),
            tls_certificate: None,
            tls_private_key: None,
            tls_ca_cert: None,
            tls_mode: None,
            tls_rate_limit_per_second: Self::default_tls_rate_limit_per_second(),
            server_tls: false,
            verify_server_certificate: false,
            admin_username: String::from("admin"),
            admin_password: String::from("admin"),
            server_lifetime: Self::default_server_lifetime(),
            retain_connections_time: Self::default_retain_connections_time(),
            retain_connections_max: Self::default_retain_connections_max(),
            server_round_robin: Self::default_server_round_robin(),
            prepared_statements: Self::default_prepared_statements(),
            prepared_statements_cache_size: Self::default_prepared_statements_cache_size(),
            client_prepared_statements_cache_size:
                Self::default_client_prepared_statements_cache_size(),
            hba: Self::default_hba(),
            pg_hba: None,
            daemon_pid_file: Self::default_daemon_pid_file(),
            syslog_prog_name: None,
            pooler_check_query: Self::default_pooler_check_query(),
            pooler_check_query_request_bytes: None,
            backlog: Self::default_backlog(),
        }
    }
}
