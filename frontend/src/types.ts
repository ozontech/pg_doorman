/**
 * DTO mirrors. Phase 5 only exposes the types phase 5 actually uses; phase 6
 * adds the rest as pages need them. Source of truth is `src/web/routes/dto.rs`
 * — keep these manual until divergence becomes painful.
 */
export interface VersionDto {
  version: string;
  git_commit: string;
  build_date: string;
  ts: number;
}

export interface OverviewDto {
  ts: number;
  active_clients: number;
  idle_clients: number;
  waiting_clients: number;
  active_servers: number;
  idle_servers: number;
  connections_total: number;
  connections_tls_total: number;
  connections_plain_total: number;
  connections_cancel_total: number;
  query_count_total: number;
  transaction_count_total: number;
  errors_count_total: number;
  prepared_hits_total: number;
  prepared_misses_total: number;
  pools_total: number;
  pools_paused: number;
  // Process tile fields (added in the backend P0 inventory wedge).
  rss_bytes: number;
  uptime_seconds: number;
  pid: number;
  current_clients: number;
  clients_in_transactions: number;
  shutdown_in_progress: boolean;
  migration_in_progress: boolean;
}

export interface PoolDto {
  id: string;
  user: string;
  database: string;
  host: string;
  port: number;
  pool_mode: string;
  max_connections: number;
  min_connections: number;
  connections: number;
  idle: number;
  active: number;
  waiting: number;
  max_active_age_ms: number;
  query_p95_ms: number;
  query_p99_ms: number;
  transactions_p95_ms: number;
  transactions_p99_ms: number;
  wait_avg_ms: number;
  wait_p95_ms: number;
  queries_total: number;
  transactions_total: number;
  errors_total: number;
  // Cumulative error breakdown by PostgreSQL SQLSTATE. Optional — backend
  // omits the field when no errors have been classified yet.
  errors_by_sqlstate?: Record<string, number>;
  paused: boolean;
  epoch: number;
  // Patroni-assisted fallback flag (mirror of the prometheus gauge).
  fallback_active: boolean;
  // Cumulative count of failed backend TLS handshakes for this pool.
  tls_handshake_errors_total: number;
  // Live TLS-encrypted backend connections held by the pool.
  tls_backend_connections: number;
}

export interface PoolsDto {
  ts: number;
  pools: PoolDto[];
}

export type Severity = "ok" | "degraded" | "critical";

export interface EventEntryDto {
  seq: number;
  ts_ms: number;
  // RELOAD, PAUSE, RESUME, RECONNECT (admin commands).
  target: string;
  message: string;
}

export interface EventsDto {
  ts: number;
  next_seq: number;
  events: EventEntryDto[];
}

export interface AppRowDto {
  application_name: string;
  clients: number;
  queries_total: number;
  transactions_total: number;
  errors_total: number;
}

export interface AppsDto {
  ts: number;
  apps: AppRowDto[];
}

export interface ProcessThreadDto {
  tid: number;
  name: string;
  cpu_user_us: number;
  cpu_system_us: number;
}

export interface ProcessDto {
  ts: number;
  pid: number;
  hostname: string;
  uptime_seconds: number;
  started_at_ms: number;
  rss_bytes: number;
  vm_size_bytes: number;
  threads: number;
  fd_open: number;
  fd_limit: number;
  cpu_user_us: number;
  cpu_system_us: number;
  cpu_cores: number;
  threads_breakdown: ProcessThreadDto[];
}

export interface InternerKindDto {
  entries: number;
  bytes: number;
}

export interface InternerDto {
  ts: number;
  named: InternerKindDto;
  anonymous: InternerKindDto;
}

export interface TcpCounts {
  established: number;
  time_wait: number;
  close_wait: number;
  listen: number;
  // Other states exist (syn_sent/recv, fin_wait1/2, close, last_ack, etc.) —
  // phase 6a-4 surfaces only the four operators most often look at; they can
  // be added later without an api change.
}

export interface UnixStreamCounts {
  established: number;
  listen: number;
}

export interface SocketsDto {
  ts: number;
  tcp: TcpCounts;
  tcp6: TcpCounts;
  unix_stream: UnixStreamCounts;
}

export interface ClientDto {
  client_id: string;
  database: string;
  user: string;
  application_name: string;
  addr: string;
  tls: boolean;
  state: string;
  wait: string;
  wait_ms: number;
  transactions_total: number;
  queries_total: number;
  errors_total: number;
  age_seconds: number;
  current_query_age_ms: number;
}

export interface ClientsDto {
  ts: number;
  total: number;
  limit: number;
  offset: number;
  clients: ClientDto[];
}

export interface PreparedRowDto {
  pool: string;
  hash: string;
  name: string;
  count_used: number;
  hits: number;
  misses: number;
  kind: string;
}

export interface PreparedDto {
  ts: number;
  prepared: PreparedRowDto[];
}

// Admin-only response for /api/prepared/text/{hash}.
export interface PreparedTextDto {
  ts: number;
  hash: string;
  pool: string;
  name: string;
  query: string;
  kind: string;
}

// Admin-only response for /api/interner/top.
export interface InternerTopRowDto {
  hash: string;
  kind: string;
  bytes: number;
  // Idle milliseconds for anonymous entries; -1 for named entries.
  idle_ms: number;
  // First 120 chars of the SQL text, truncated by chars (not bytes).
  preview: string;
}

export interface InternerTopDto {
  ts: number;
  n: number;
  entries: InternerTopRowDto[];
}

export interface LogEntryDto {
  seq: number;
  ts_ms: number;
  level: string;
  target: string;
  message: string;
}

export interface LogsDto {
  ts: number;
  tap_active: boolean;
  tap_capacity_entries: number;
  tap_used_entries: number;
  next_seq: number;
  dropped_before: number;
  dropped_total: number;
  entries: LogEntryDto[];
}

export interface ConfigEntry {
  key: string;
  value: string;
  default: string;
  changeable: string;
}

export interface ConfigDto {
  ts: number;
  config: ConfigEntry[];
}

export interface LogLevelDto {
  ts: number;
  log_level: string;
}

export interface AuthQueryRowDto {
  database: string;
  cache_entries: number;
  cache_hits: number;
  cache_misses: number;
  cache_refetches: number;
  cache_rate_limited: number;
  auth_success: number;
  auth_failure: number;
  executor_queries: number;
  executor_errors: number;
  dynamic_pools_current: number;
  dynamic_pools_created: number;
  dynamic_pools_destroyed: number;
}

export interface AuthQueryDto {
  ts: number;
  pools: AuthQueryRowDto[];
}

export interface DatabaseDto {
  name: string;
  host: string;
  port: number;
  database: string;
  force_user: string;
  pool_size: number;
  min_pool_size: number;
  reserve_pool: number;
  pool_mode: string;
  max_connections: number;
  current_connections: number;
}

export interface DatabasesDto {
  ts: number;
  databases: DatabaseDto[];
}

export interface UserDto {
  name: string;
  pool_mode: string;
}

export interface UsersDto {
  ts: number;
  users: UserDto[];
}

export interface PoolScalingRowDto {
  user: string;
  database: string;
  inflight: number;
  creates: number;
  gate_waits: number;
  gate_budget_ex: number;
  antic_notify: number;
  antic_timeout: number;
  create_fallback: number;
  replenish_def: number;
}

export interface PoolScalingDto {
  ts: number;
  pools: PoolScalingRowDto[];
}

export interface PoolCoordinatorRowDto {
  database: string;
  max_db_conn: number;
  current: number;
  reserve_size: number;
  reserve_used: number;
  evictions: number;
  reserve_acq: number;
  exhaustions: number;
}

export interface PoolCoordinatorDto {
  ts: number;
  databases: PoolCoordinatorRowDto[];
}
