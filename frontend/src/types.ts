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
  paused: boolean;
  epoch: number;
}

export interface PoolsDto {
  ts: number;
  pools: PoolDto[];
}

export type Severity = "ok" | "degraded" | "critical";

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
