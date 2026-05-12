#!/usr/bin/env python3
"""Generate pg_doorman Grafana dashboard JSON from code.

Usage:
    python3 generate_dashboard.py > dashboards/pg_doorman.json
"""

import json
from grafana_foundation_sdk.builders import (
    dashboard as dash_builder,
    timeseries,
    stat,
    gauge,
    prometheus,
    table,
    common as common_builder,
)
from grafana_foundation_sdk.builders.dashboard import ThresholdsConfig as ThresholdsBuilder
from grafana_foundation_sdk.models import dashboard as dash_models
from grafana_foundation_sdk.models.common import (
    BigValueColorMode,
    LegendDisplayMode,
    LegendPlacement,
)
from grafana_foundation_sdk.cog.encoder import JSONEncoder


# For portable dashboards (grafana.com): "${DS_PROMETHEUS}"
# For provisioned dashboards (docker compose demo): "prometheus"
import os
DS = os.environ.get("GRAFANA_DS_UID", "prometheus")


def prom(expr: str, legend: str = "") -> prometheus.Dataquery:
    q = prometheus.Dataquery().expr(expr)
    if legend:
        q = q.legend_format(legend)
    return q


def stat_panel(title: str, expr: str, unit: str = "", w: int = 4,
               thresholds=None, color_mode="background", desc: str = ""):
    p = (
        stat.Panel()
        .title(title)
        .datasource(dash_models.DataSourceRef(uid=DS))
        .with_target(prom(expr))
        .height(4)
        .span(w)
    )
    if desc:
        p = p.description(desc)
    if unit:
        p = p.unit(unit)
    if thresholds:
        tb = ThresholdsBuilder().mode(dash_models.ThresholdsMode.ABSOLUTE).steps(
            [dash_models.Threshold(color=c, value=v) for v, c in thresholds]
        )
        p = p.thresholds(tb)
    if color_mode:
        p = p.color_mode(BigValueColorMode(color_mode))
    return p


def ts_panel(title: str, targets: list, unit: str = "", w: int = 12,
             desc: str = "", legend_calcs=None):
    if legend_calcs is None:
        legend_calcs = ["min", "max", "lastNotNull"]
    p = (
        timeseries.Panel()
        .title(title)
        .datasource(dash_models.DataSourceRef(uid=DS))
        .height(8)
        .span(w)
        .legend(
            common_builder.VizLegendOptions()
            .show_legend(True)
            .display_mode(LegendDisplayMode.TABLE)
            .placement(LegendPlacement.BOTTOM)
            .calcs(legend_calcs)
        )
    )
    if desc:
        p = p.description(desc)
    for t in targets:
        p = p.with_target(t)
    if unit:
        p = p.unit(unit)
    return p


def collapsed_row(title: str):
    return dash_builder.Row(title).collapsed(True)


def expanded_row(title: str):
    return dash_builder.Row(title).collapsed(False)


# ---------------------------------------------------------------------------
# Variables
# ---------------------------------------------------------------------------

var_datasource = (
    dash_builder.DatasourceVariable("DS_PROMETHEUS")
    .label("Prometheus")
    .type("prometheus")
    .hide(dash_models.VariableHide.HIDE_VARIABLE)
)

var_instance = (
    dash_builder.QueryVariable("instance")
    .label("Instance")
    .query("label_values(pg_doorman_total_memory, instance)")
    .datasource(dash_models.DataSourceRef(uid=DS))
    .include_all(True)
    .multi(True)
    .refresh(dash_models.VariableRefresh.ON_TIME_RANGE_CHANGED)
)

var_database = (
    dash_builder.QueryVariable("database")
    .label("Database")
    .query('label_values(pg_doorman_pools_clients{instance=~"$instance"}, database)')
    .datasource(dash_models.DataSourceRef(uid=DS))
    .include_all(True)
    .multi(True)
    .refresh(dash_models.VariableRefresh.ON_TIME_RANGE_CHANGED)
)

var_user = (
    dash_builder.QueryVariable("user")
    .label("User")
    .query('label_values(pg_doorman_pools_clients{instance=~"$instance", database=~"$database"}, user)')
    .datasource(dash_models.DataSourceRef(uid=DS))
    .include_all(True)
    .multi(True)
    .refresh(dash_models.VariableRefresh.ON_TIME_RANGE_CHANGED)
)

# Selector shorthand
S = 'instance=~"$instance", user=~"$user", database=~"$database"'
SD = 'instance=~"$instance", database=~"$database"'

# ---------------------------------------------------------------------------
# Row 1: Overview
# ---------------------------------------------------------------------------
row1 = expanded_row("Overview")

p_waiting = stat_panel(
    "Waiting Clients",
    f'sum(pg_doorman_pools_clients{{status="waiting", {S}}})',
    thresholds=[(None, "green"), (1, "yellow"), (10, "red")],
    desc="Clients queued for a server connection. Sustained >0 means pool_size is insufficient — increase pool_size or reduce query duration.",
)
p_wait_time = stat_panel(
    "Wait p99",
    f'histogram_quantile(0.99, sum by (le) (rate(pg_doorman_pools_wait_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
    unit="s",
    thresholds=[(None, "green"), (0.005, "yellow"), (0.05, "red")],
    desc="99th percentile client checkout wait. Above 50 ms: pool_size is the bottleneck before PostgreSQL is — raise it or shorten queries.",
)
p_query_p99 = stat_panel(
    "Query p99",
    f'histogram_quantile(0.99, sum by (le) (rate(pg_doorman_pools_query_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
    unit="s",
    thresholds=[(None, "green"), (0.05, "yellow"), (0.2, "red")],
    desc="99th percentile server-side query time (excludes queue wait). Spike without QPS increase — check pg_stat_activity for locks or vacuum.",
)
p_utilization = stat_panel(
    "Pool Utilization",
    f'sum(pg_doorman_pools_servers{{status="active", {S}}}) / sum(pg_doorman_pool_size{{{S}}}) * 100',
    unit="percent",
    thresholds=[(None, "green"), (70, "yellow"), (90, "red")],
    desc="Active server connections / pool_size. Above 70%: anticipate saturation. Above 90%: clients are queuing.",
)
p_memory = stat_panel(
    "Memory",
    'pg_doorman_total_memory{instance=~"$instance"}',
    unit="bytes",
    thresholds=[(None, "green"), (536870912, "yellow"), (1073741824, "red")],
    desc="Process RSS. Sudden growth without new connections usually means unbounded prepared statement cache — check Pool Cache Entries.",
)
p_connections = stat_panel(
    "Total Connections",
    'pg_doorman_connections_total{type="total", instance=~"$instance"}',
    thresholds=[(None, "blue")],
    color_mode="none",
    desc="Cumulative client connections accepted (all pools). Compare with pool_size for multiplexing ratio — 100:1+ is normal in transaction mode.",
)

# ---------------------------------------------------------------------------
# Row 2: Client Load
# ---------------------------------------------------------------------------
row2 = expanded_row("Client Load")

p_clients_state = ts_panel(
    "Clients by State", [
        prom(f'sum by (status) (pg_doorman_pools_clients{{{S}}})', "{{status}}"),
    ], w=8,
    desc="Active (has server), idle (between transactions), waiting (queued). Growing 'waiting' area means pool_size is the bottleneck.",
)
p_waiting_ts = ts_panel(
    "Waiting Clients", [
        prom(f'pg_doorman_pools_clients{{status="waiting", {S}}}', "{{user}}@{{database}}"),
    ], w=8,
    desc="Waiting clients by user@database. Pinpoints which pool needs pool_size increase or query optimization.",
)
p_wait_time_ts = ts_panel(
    "Wait p95 by Pool", [
        prom(
            f'histogram_quantile(0.95, sum by (le, user, database) (rate(pg_doorman_pools_wait_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "{{user}}@{{database}}",
        ),
    ], unit="s", w=8,
    desc="95th percentile client checkout wait per pool. The pool with the highest line is the one that needs pool_size attention.",
)

# ---------------------------------------------------------------------------
# Row 3: Server Pool
# ---------------------------------------------------------------------------
row3 = expanded_row("Server Pool")

p_servers_state = ts_panel(
    "Servers by State", [
        prom(f'sum by (status) (pg_doorman_pools_servers{{{S}}})', "{{status}}"),
    ], w=8,
    desc="Backend connections: active (executing query), idle (available for checkout). Idle approaching zero means no headroom for bursts.",
)
p_pool_vs_active = ts_panel(
    "Pool Size vs Active Servers", [
        prom(f'pg_doorman_pool_size{{{S}}}', "pool_size {{user}}@{{database}}"),
        prom(f'pg_doorman_pools_servers{{status="active", {S}}}', "active {{user}}@{{database}}"),
    ], w=8,
    desc="Active servers overlaid with pool_size ceiling. Gap between lines is spare capacity. When they converge, clients start queuing.",
)
p_pool_util_ts = ts_panel(
    "Pool Utilization %", [
        prom(f'pg_doorman_pools_servers{{status="active", {S}}} / pg_doorman_pool_size{{{S}}} * 100', "{{user}}@{{database}}"),
    ], unit="percent", w=8,
    desc="Active/pool_size ratio over time. Sustained above 70% warrants pool_size increase; above 90% clients are already waiting.",
)

# ---------------------------------------------------------------------------
# Row 4: Query Latency
# ---------------------------------------------------------------------------
row4 = expanded_row("Query Latency")

p_query_lat = ts_panel(
    "Query Latency Percentiles", [
        prom(
            f'histogram_quantile(0.50, sum by (le) (rate(pg_doorman_pools_query_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p50",
        ),
        prom(
            f'histogram_quantile(0.90, sum by (le) (rate(pg_doorman_pools_query_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p90",
        ),
        prom(
            f'histogram_quantile(0.95, sum by (le) (rate(pg_doorman_pools_query_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p95",
        ),
        prom(
            f'histogram_quantile(0.99, sum by (le) (rate(pg_doorman_pools_query_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p99",
        ),
    ], unit="s",
    desc="Server-side query time at p50/p90/p95/p99 from the per-pool histogram. p99 diverging from p50 — check pg_stat_activity for lock waits or long-running queries.",
)
p_qps = ts_panel(
    "Queries per Second", [
        prom(
            f'sum by (user, database) (rate(pg_doorman_pools_queries_total{{{S}}}[$__rate_interval]))',
            "{{user}}@{{database}}",
        ),
    ], unit="ops",
    desc="Query throughput per pool. Flat QPS with rising latency signals PostgreSQL saturation. Rising QPS with stable latency is healthy growth.",
)

# ---------------------------------------------------------------------------
# Row 5: Transaction Latency
# ---------------------------------------------------------------------------
row5 = expanded_row("Transaction Latency")

p_xact_lat = ts_panel(
    "Transaction Latency Percentiles", [
        prom(
            f'histogram_quantile(0.50, sum by (le) (rate(pg_doorman_pools_transaction_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p50",
        ),
        prom(
            f'histogram_quantile(0.90, sum by (le) (rate(pg_doorman_pools_transaction_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p90",
        ),
        prom(
            f'histogram_quantile(0.95, sum by (le) (rate(pg_doorman_pools_transaction_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p95",
        ),
        prom(
            f'histogram_quantile(0.99, sum by (le) (rate(pg_doorman_pools_transaction_duration_seconds_bucket{{{S}}}[$__rate_interval])))',
            "p99",
        ),
    ], unit="s",
    desc="End-to-end transaction time at p50/p90/p95/p99 from the per-pool histogram. High values with low query latency indicate application-side delays between queries.",
)
p_tps = ts_panel(
    "Transactions per Second", [
        prom(
            f'sum by (user, database) (rate(pg_doorman_pools_transactions_total{{{S}}}[$__rate_interval]))',
            "{{user}}@{{database}}",
        ),
    ], unit="ops",
    desc="Transaction throughput per pool. Drop with rising latency indicates lock contention or long transactions holding server connections.",
)

# ---------------------------------------------------------------------------
# Row 6: Traffic (collapsed)
# ---------------------------------------------------------------------------
row6 = collapsed_row("Traffic")

p_bytes_recv = ts_panel(
    "Bytes Received", [
        prom(
            f'sum by (user, database) (rate(pg_doorman_pools_bytes_total{{direction="received", {S}}}[$__rate_interval]))',
            "{{user}}@{{database}}",
        ),
    ], unit="Bps",
    desc="Data rate from clients. Spikes correlate with bulk INSERTs/COPYs.",
)
p_bytes_sent = ts_panel(
    "Bytes Sent", [
        prom(
            f'sum by (user, database) (rate(pg_doorman_pools_bytes_total{{direction="sent", {S}}}[$__rate_interval]))',
            "{{user}}@{{database}}",
        ),
    ], unit="Bps",
    desc="Data rate to clients. Large spikes indicate fat result sets — consider LIMIT if unexpected.",
)

# ---------------------------------------------------------------------------
# Row 7: Pool Coordinator (collapsed)
# ---------------------------------------------------------------------------
row7 = collapsed_row("Pool Coordinator")

p_coord_conns = ts_panel(
    "Connections vs Max", [
        prom(f'pg_doorman_pool_coordinator{{type="connections", {SD}}}', "current"),
        prom(f'pg_doorman_pool_coordinator{{type="max_connections", {SD}}}', "max"),
    ], w=8,
    desc="Total backend connections across all user pools vs max_db_connections. When current approaches max, coordinator evicts idle connections from lower-priority pools.",
)
p_coord_reserve = ts_panel(
    "Reserve Pool", [
        prom(f'pg_doorman_pool_coordinator{{type="reserve_in_use", {SD}}}', "in_use"),
        prom(f'pg_doorman_pool_coordinator{{type="reserve_pool_size", {SD}}}', "size"),
    ], w=8,
    desc="Reserve connections activated when all max_db_connections slots are full. Any usage means primary capacity exhausted — raise max_db_connections.",
)
p_coord_events = ts_panel(
    "Coordinator Events", [
        prom(f'rate(pg_doorman_pool_coordinator_total{{type="evictions", {SD}}}[$__rate_interval])', "evictions/s"),
        prom(f'rate(pg_doorman_pool_coordinator_total{{type="reserve_acquisitions", {SD}}}[$__rate_interval])', "reserve_acq/s"),
        prom(f'rate(pg_doorman_pool_coordinator_total{{type="exhaustions", {SD}}}[$__rate_interval])', "exhaustions/s"),
    ], w=8,
    desc="Evictions: idle connections reclaimed across pools (normal). Exhaustions: client errors, no connection available (critical — increase capacity).",
)

# ---------------------------------------------------------------------------
# Row 8: Pool Scaling (collapsed)
# ---------------------------------------------------------------------------
row8 = collapsed_row("Pool Scaling")

p_inflight = ts_panel(
    "Inflight Creates", [
        prom(f'pg_doorman_pool_scaling{{type="inflight_creates", {S}}}', "{{user}}@{{database}}"),
    ], w=8,
    desc="Connections mid-handshake (TCP + auth + startup). Sustained high count — PostgreSQL slow to accept, check auth method or backend CPU.",
)
p_scaling_events = ts_panel(
    "Scaling Events", [
        prom(f'sum by (database) (rate(pg_doorman_pool_scaling_total{{type="creates_started", {S}}}[$__rate_interval]))', "creates/s"),
        prom(f'sum by (database) (rate(pg_doorman_pool_scaling_total{{type="burst_gate_waits", {S}}}[$__rate_interval]))', "gate_waits/s"),
        prom(f'sum by (database) (rate(pg_doorman_pool_scaling_total{{type="create_fallback", {S}}}[$__rate_interval]))', "fallback/s"),
    ], w=8,
    desc="creates/s: new connections. gate_waits/s: throttled by burst limiter. fallback/s: anticipation missed, created on-demand.",
)
p_conn_types = ts_panel(
    "Connections by Type", [
        prom(
            'rate(pg_doorman_connections_total{type="plain", instance=~"$instance"}[$__rate_interval])',
            "plain",
        ),
        prom(
            'rate(pg_doorman_connections_total{type="tls", instance=~"$instance"}[$__rate_interval])',
            "tls",
        ),
        prom(
            'rate(pg_doorman_connections_total{type="cancel", instance=~"$instance"}[$__rate_interval])',
            "cancel",
        ),
    ], unit="ops", w=8,
    desc="Connection rate per second by protocol. Track TLS adoption. Elevated cancel rate indicates application timeouts.",
)

# ---------------------------------------------------------------------------
# Row 9: Prepared Statements (collapsed)
# ---------------------------------------------------------------------------
row9 = collapsed_row("Prepared Statements")

p_cache_entries = ts_panel(
    "Pool Cache Entries", [
        prom(f'pg_doorman_pool_prepared_cache_entries{{{S}}}', "{{user}}@{{database}}"),
    ], w=8,
    desc="Unique prepared statements per pool. Unbounded growth means dynamic statement names — fix the app or cap with prepared_statements_cache_size.",
)
p_cache_bytes = ts_panel(
    "Cache Memory (Pool + Client)", [
        prom(f'pg_doorman_pool_prepared_cache_bytes{{{S}}}', "pool {{user}}@{{database}}"),
        prom(f'pg_doorman_clients_prepared_cache_bytes{{{S}}}', "client {{user}}@{{database}}"),
    ], unit="bytes", w=8,
    desc="Memory in prepared caches (pool + client). When this dominates total memory, reduce cache size or fix dynamic statement names.",
)
p_hit_ratio = ts_panel(
    "Prepared Statement Hit Ratio", [
        prom(
            f'clamp_max('
            f'sum by (user, database) (rate(pg_doorman_servers_prepared_hits_total{{{S}}}[$__rate_interval])) / '
            f'clamp_min(sum by (user, database) (rate(pg_doorman_servers_prepared_hits_total{{{S}}}[$__rate_interval])) + '
            f'sum by (user, database) (rate(pg_doorman_servers_prepared_misses_total{{{S}}}[$__rate_interval])), 0.001)'
            f', 1)',
            "{{user}}@{{database}}",
        ),
    ], unit="percentunit", w=8,
    desc="Cache hits / total lookups (rate over the per-pool counters). Below 90%: servers frequently re-parse after multiplexing. Ensure consistent statement names.",
)
p_client_named = ts_panel(
    "Client Named Entries", [
        prom(f'pg_doorman_clients_prepared_named_entries{{{S}}}', "{{user}}@{{database}}"),
    ], w=8,
    desc="Sum of Named entries across all clients in the pool. Named is unbounded — drivers that mint per-query named statements (some pgjdbc / Hibernate / Npgsql configurations) drive this up without limit. Application is responsible for DEALLOCATE or name reuse.",
)
p_client_anonymous = ts_panel(
    "Client Anonymous Entries", [
        prom(f'pg_doorman_clients_prepared_anonymous_entries{{{S}}}', "{{user}}@{{database}}"),
    ], w=8,
    desc="Sum of Anonymous entries across all clients. Bounded per client by client_anonymous_prepared_cache_size (default 256). Approaches at most connected_clients * cache_size.",
)
p_client_anonymous_evictions = ts_panel(
    "Anonymous LRU Eviction Rate", [
        prom(
            f'rate(pg_doorman_clients_prepared_anonymous_evictions_total{{{S}}}[$__rate_interval])',
            "{{user}}@{{database}}",
        ),
    ], unit="ops", w=8,
    desc="Rate of evictions on the per-client Anonymous LRU. Sustained non-zero rate means client_anonymous_prepared_cache_size is too small for the workload, or the application generates unique queries on the hot path. Alert template: > 10/s for 10m.",
)

# ---------------------------------------------------------------------------
# Row 10: Auth Query (collapsed)
# ---------------------------------------------------------------------------
row10 = collapsed_row("Auth Query")

p_auth_cache = ts_panel(
    "Auth Cache Hit Rate", [
        prom(
            f'clamp_max('
            f'rate(pg_doorman_auth_query_cache_total{{type="hits", {SD}}}[$__rate_interval]) / '
            f'clamp_min(rate(pg_doorman_auth_query_cache_total{{type="hits", {SD}}}[$__rate_interval]) + '
            f'rate(pg_doorman_auth_query_cache_total{{type="misses", {SD}}}[$__rate_interval]), 0.001)'
            f', 1)',
            "{{database}}",
        ),
    ], unit="percentunit", w=8,
    desc="Auth lookups served from cache vs PostgreSQL query. Low rate adds latency to every new connection — increase cache_ttl.",
)
p_auth_outcomes = ts_panel(
    "Auth Outcomes", [
        prom(
            f'rate(pg_doorman_auth_query_auth_total{{result="success", {SD}}}[$__rate_interval])',
            "success/s",
        ),
        prom(
            f'rate(pg_doorman_auth_query_auth_total{{result="failure", {SD}}}[$__rate_interval])',
            "failure/s",
        ),
    ], w=8,
    desc="Auth success vs failure rate. Failure spike after deploy = credential mismatch. Sustained failures = check source IPs in logs.",
)
p_dynamic_pools = ts_panel(
    "Dynamic Pools", [
        prom(f'pg_doorman_auth_query_dynamic_pools{{type="current", {SD}}}', "current"),
    ], w=8,
    desc="Auto-created pools for auth_query users (snapshot count, gauge). Unexpected growth indicates wrong database names or unplanned user sprawl.",
)

# ---------------------------------------------------------------------------
# Row 11: System (collapsed)
# ---------------------------------------------------------------------------
row11 = collapsed_row("System")

p_memory_ts = ts_panel(
    "Process Memory", [
        prom('pg_doorman_total_memory{instance=~"$instance"}', "{{instance}}"),
    ], unit="bytes",
    desc="Process RSS over time. Correlate with Total Connections and Cache Memory to isolate growth driver.",
)
p_sockets = ts_panel(
    "Sockets by Type", [
        prom('pg_doorman_sockets{type="tcp", instance=~"$instance"}', "tcp"),
        prom('pg_doorman_sockets{type="tcp6", instance=~"$instance"}', "tcp6"),
        prom('pg_doorman_sockets{type="unix", instance=~"$instance"}', "unix"),
    ],
    desc="Open sockets by protocol. Count growing faster than connections indicates FD leak — check CLOSE_WAIT via SHOW SOCKETS.",
)

# ---------------------------------------------------------------------------
# Row 12: Patroni-assisted fallback (collapsed)
# ---------------------------------------------------------------------------
SI = 'instance=~"$instance"'

row12 = collapsed_row("Patroni-assisted fallback")

p_fallback_active = stat_panel(
    "Local backend in cooldown",
    f'max(pg_doorman_fallback_active{{{SI}}})',
    thresholds=[(None, "green"), (1, "red")],
    desc="1 if any pool is currently using a fallback host. Sustained = local backend not recovering.",
)
p_fallback_connections = stat_panel(
    "Fallback Connections",
    f'sum(increase(pg_doorman_fallback_connections_total{{{SI}}}[$__rate_interval]))',
    thresholds=[(None, "blue")],
    color_mode="none",
    desc="Connections routed to fallback hosts in the current window.",
)
p_patroni_api_errors = stat_panel(
    "Patroni API Errors",
    f'sum(increase(pg_doorman_patroni_api_errors_total{{{SI}}}[$__rate_interval]))',
    thresholds=[(None, "green"), (1, "red")],
    desc="Failed /cluster requests (all Patroni URLs unreachable).",
)

p_patroni_api_rate = ts_panel(
    "Patroni API Rate", [
        prom(f'rate(pg_doorman_patroni_api_requests_total{{{SI}}}[$__rate_interval])', "{{pool}} requests/s"),
        prom(f'rate(pg_doorman_fallback_connections_total{{{SI}}}[$__rate_interval])', "{{pool}} connections/s"),
        prom(f'rate(pg_doorman_patroni_api_errors_total{{{SI}}}[$__rate_interval])', "{{pool}} errors/s"),
    ], w=8,
    desc="Patroni API calls, fallback connections, and errors per second. Errors without connections = all Patroni URLs or all candidates unreachable.",
)
p_patroni_api_duration = ts_panel(
    "Patroni API Duration", [
        prom(f'histogram_quantile(0.50, rate(pg_doorman_patroni_api_duration_seconds_bucket{{{SI}}}[$__rate_interval]))', "p50"),
        prom(f'histogram_quantile(0.99, rate(pg_doorman_patroni_api_duration_seconds_bucket{{{SI}}}[$__rate_interval]))', "p99"),
    ], unit="s", w=8,
    desc="Time to fetch /cluster from Patroni API. p99 above 1s = network issues or overloaded Patroni nodes.",
)
p_fallback_cache_hits = ts_panel(
    "Fallback Cache Hits", [
        prom(f'rate(pg_doorman_fallback_cache_hits_total{{{SI}}}[$__rate_interval])', "{{pool}}"),
    ], w=8,
    desc="Fallback host served from cache without querying Patroni API. High rate during cooldown = cache working correctly.",
)

# ---------------------------------------------------------------------------
# Row 13: Query Interner (collapsed)
# ---------------------------------------------------------------------------
row13 = collapsed_row("Query Interner")

p_interner_entries = ts_panel(
    "Interner Entries by Kind", [
        prom(f'pg_doorman_query_interner_entries{{{SI}}}', "{{kind}}"),
    ], w=8,
    desc="Live entries in the global query interner. NAMED is bounded by passive Arc::strong_count GC; ANON is bounded by query_interner_anon_idle_ttl_seconds. Sustained growth on either line points to either a long TTL on a unique-anon workload or an Arc<str> leak on the named side.",
)
p_interner_bytes = ts_panel(
    "Interner Bytes by Kind", [
        prom(f'pg_doorman_query_interner_bytes{{{SI}}}', "{{kind}}"),
    ], unit="bytes", w=8,
    desc="Total length of interned Parse text per kind. ANON > 1.5 GiB is the alert threshold for the bundled prometheus rule (PgDoormanAnonInternerMemoryHigh).",
)
p_interner_evictions = ts_panel(
    "Interner Eviction Rate", [
        prom(
            f'rate(pg_doorman_query_interner_evictions_total{{{SI}}}[$__rate_interval])',
            "{{kind}}/{{reason}}",
        ),
    ], unit="ops", w=8,
    desc="Evictions per second, split by kind (named|anonymous) and reason (gc_passive for named, ttl_expired for anonymous). Steady ANON eviction is normal; steady NAMED eviction is unusual unless a flood of unique named statements just landed.",
)
p_interner_synthetic_misses = ts_panel(
    "Synthetic SQLSTATE 26000 Rate", [
        prom(
            f'rate(pg_doorman_query_interner_synthetic_misses_total{{{SI}}}[$__rate_interval])',
            "synthetic 26000/s",
        ),
    ], unit="ops", w=12,
    desc="Bind referencing an anonymous prepared whose text is no longer in any cache. Flat zero is the normal case. Sustained > 1/s = TTL too short for the workload, or a driver depending on cross-batch unnamed prepared statements.",
)
p_interner_gc_duration = ts_panel(
    "GC Sweep Duration", [
        prom(f'histogram_quantile(0.50, sum by (le) (rate(pg_doorman_query_interner_gc_duration_seconds_bucket{{{SI}}}[$__rate_interval])))', "p50"),
        prom(f'histogram_quantile(0.99, sum by (le) (rate(pg_doorman_query_interner_gc_duration_seconds_bucket{{{SI}}}[$__rate_interval])))', "p99"),
    ], unit="s", w=12,
    desc="Wall-clock time of one GC sweep cycle (named + anonymous combined). P99 above 50 ms means the sweep is starting to bite into request latency tails — increase query_interner_gc_interval_seconds or shrink the interner via RESET INTERNER plus cache-size tuning.",
)

# ---------------------------------------------------------------------------
# Row 14: Pool State (collapsed) — pause/maxwait per pool
# ---------------------------------------------------------------------------
row14 = collapsed_row("Pool State")

p_pool_paused = ts_panel(
    "Paused Pools", [
        prom(f'pg_doorman_pools_paused{{{S}}}', "{{user}}@{{database}}"),
    ], w=12,
    desc="1 when the pool is currently paused via PAUSE admin command, 0 when running. A pool stuck at 1 after incident triage drops all client traffic until manually resumed.",
)
p_pool_maxwait = ts_panel(
    "Pool Max Wait (worst client)", [
        prom(
            f'pg_doorman_pools_maxwait_microseconds{{{S}}} / 1000',
            "{{user}}@{{database}}",
        ),
    ], unit="ms", w=12,
    desc="Largest single client checkout wait in each pool, taken as max(client.max_wait_time) across alive clients. Each client tracks its own lifetime maximum, so a spike means 'someone in this pool ever waited this long', not 'someone is waiting now'.",
)

# ---------------------------------------------------------------------------
# Row 15: Pool Errors (collapsed) — SQLSTATE class breakdown
# ---------------------------------------------------------------------------
row15 = collapsed_row("Pool Errors")

p_pool_errors_by_sqlstate = ts_panel(
    "Pool Errors per Second by SQLSTATE Class", [
        prom(
            f'sum by (sqlstate) (rate(pg_doorman_pools_errors_total{{{S}}}[$__rate_interval]))',
            "{{sqlstate}}",
        ),
    ], unit="ops", w=12,
    desc="Backend errors per pool, bucketed by SQLSTATE class: 08 (connection_exception), 53 (insufficient_resources), 57 (operator_intervention), 25P02 (in_failed_sql_transaction), 26000 (invalid_sql_statement_name), other. The full 5-character code is in /api/pools and the Web UI.",
)
p_pool_errors_by_pool = ts_panel(
    "Pool Errors per Second by Pool", [
        prom(
            f'sum by (user, database) (rate(pg_doorman_pools_errors_total{{{S}}}[$__rate_interval]))',
            "{{user}}@{{database}}",
        ),
    ], unit="ops", w=12,
    desc="Same counter aggregated per pool. Use this view to find which pool produces the bulk of errors when the SQLSTATE breakdown shows a spike.",
)

# ---------------------------------------------------------------------------
# Row 16: Listener Rejections (collapsed) — pre-auth client drops
# ---------------------------------------------------------------------------
row16 = collapsed_row("Listener Rejections")

p_listener_rejections = ts_panel(
    "Pre-auth Rejections per Second by Reason", [
        prom(
            f'rate(pg_doorman_listener_rejections_total{{{SI}}}[$__rate_interval])',
            "{{reason}}",
        ),
    ], unit="ops", w=24,
    desc="Clients dropped before authentication, by reason: hba (HBA denied), tls_required (only_ssl_connections rejected plain text), tls_handshake_fail (TLS negotiation failed), protocol_error (unexpected startup sequence), invalid_startup (bad startup or socket error), too_many_clients (listener at capacity). Sustained 'hba' or 'tls_handshake_fail' is the bruteforce-from-outside signal.",
)

# ---------------------------------------------------------------------------
# Row 17: Protocol Streaming (collapsed) — large-message byte forwarding
# ---------------------------------------------------------------------------
row17 = collapsed_row("Protocol Streaming")

p_streaming_events = ts_panel(
    "Streaming Events per Second", [
        prom(
            f'sum by (kind, result) (rate(pg_doorman_streaming_events_total{{{S}}}[$__rate_interval]))',
            "{{kind}}/{{result}}",
        ),
    ], unit="ops", w=12,
    desc="Large messages forwarded byte-for-byte by pg_doorman. kind is data_row or copy_data; result is ok or error. Sustained non-zero rate signals oversized BYTEA/JSONB payloads, COPY rows with pathological content, or a misbehaving ORM — pg_doorman buffers most messages in RAM, but anything above max_message_size is streamed to keep memory bounded.",
)
p_streaming_bytes = ts_panel(
    "Streaming Bytes per Second", [
        prom(
            f'sum by (kind) (rate(pg_doorman_streaming_bytes_total{{{S}}}[$__rate_interval]))',
            "{{kind}}",
        ),
    ], unit="Bps", w=12,
    desc="Bytes pushed through the streaming path (header + payload). Counted even on failed events, so this measures what actually reached the client wire, not only fully delivered messages.",
)

# ---------------------------------------------------------------------------
# Row 18: Backend Setup Latency (collapsed) — connect/tls/auth/startup phases
# ---------------------------------------------------------------------------
row18 = collapsed_row("Backend Setup Latency")

p_backend_phase_p99 = ts_panel(
    "Backend Setup p99 by Phase", [
        prom(
            f'histogram_quantile(0.99, sum by (le, phase) (rate(pg_doorman_backend_create_duration_seconds_bucket{{{SI}}}[$__rate_interval])))',
            "{{phase}} p99",
        ),
    ], unit="s", w=12,
    desc="99th percentile of each backend connection setup phase: tcp_connect (raw socket), tls (SSL request + handshake), auth (StartupMessage to AuthenticationOK), startup (AuthenticationOK to ReadyForQuery). The phase you don't see is the phase that failed before completing.",
)
p_backend_phase_p50 = ts_panel(
    "Backend Setup p50 by Phase", [
        prom(
            f'histogram_quantile(0.50, sum by (le, phase) (rate(pg_doorman_backend_create_duration_seconds_bucket{{{SI}}}[$__rate_interval])))',
            "{{phase}} p50",
        ),
    ], unit="s", w=12,
    desc="Median of each setup phase. Compare against p99 to spot tail-latency outliers vs steady slowness.",
)
p_backend_phase_rate = ts_panel(
    "Backend Setup Rate by Phase", [
        prom(
            f'sum by (phase) (rate(pg_doorman_backend_create_duration_seconds_count{{{SI}}}[$__rate_interval]))',
            "{{phase}}",
        ),
    ], unit="ops", w=24,
    desc="Backend connections completing each phase per second. tcp_connect rate equals total backend creates; gaps to tls/auth/startup mark drop-offs at each step.",
)

# ---------------------------------------------------------------------------
# Row 19: Startup Parameters (collapsed) — PG-rejected and pre-wire-dropped GUCs
# ---------------------------------------------------------------------------
row19 = collapsed_row("Startup Parameters")

p_sp_errors_by_sqlstate = ts_panel(
    "PG-Side Rejections by SQLSTATE", [
        prom(
            f'sum by (sqlstate) (rate(pg_doorman_backend_startup_parameter_errors_total{{{S}}}[$__rate_interval]))',
            "{{sqlstate}}",
        ),
    ], unit="ops", w=12,
    desc="Per-pool rate of backend startups PG rejected because of an operator-supplied parameter. Split by SQLSTATE: 22023 invalid_value, 42704 undefined_object, 42501 insufficient_privilege, 55P02 cant_change_runtime_param. Non-zero for the same pool over a few minutes means every connect through that pool fails on the same operator GUC — fix general/pool/auth_query.",
)
p_sp_errors_by_pool = ts_panel(
    "PG-Side Rejections by Pool", [
        prom(
            f'sum by (pool) (rate(pg_doorman_backend_startup_parameter_errors_total{{{S}}}[$__rate_interval]))',
            "{{pool}}",
        ),
    ], unit="ops", w=12,
    desc="Same counter aggregated by pool. The pool name shows which user@database is affected; check pg_doorman warn log for the parameter name and username.",
)
p_sp_dropped_by_reason = ts_panel(
    "Pre-Wire Drops by Reason", [
        prom(
            f'sum by (reason) (rate(pg_doorman_startup_parameters_dropped_total{{{S}}}[$__rate_interval]))',
            "{{reason}}",
        ),
    ], unit="ops", w=24,
    desc="Operator-supplied entries pg_doorman dropped BEFORE the StartupMessage went on the wire — the failure mode the PG-side counter above cannot see. Reasons: cascade_budget_exceeded (merged map past 9 488 bytes), packet_cap_exceeded (full packet past PG MAX_STARTUP_PACKET_LENGTH 10 000 bytes), auth_query_oversize (per-user JSON column past operator budget), auth_query_invalid_entry (one JSON entry failed validation), dedicated_mode (per-user GUC ignored because the pool shares one backend across users). Non-zero on any reason needs operator attention — backends are connecting with PG defaults instead of the configured cascade.",
)

# ---------------------------------------------------------------------------
# Build dashboard
# ---------------------------------------------------------------------------
d = (
    dash_builder.Dashboard("pg_doorman")
    .uid("pg-doorman-overview")
    .tags(["pg_doorman", "postgresql", "connection-pooler"])
    .refresh("10s")
    .time("now-30m", "now")
    .timezone("browser")
    .editable()
    .with_variable(var_datasource)
    .with_variable(var_instance)
    .with_variable(var_database)
    .with_variable(var_user)
    # Row 1: Overview
    .with_row(row1)
    .with_panel(p_waiting)
    .with_panel(p_wait_time)
    .with_panel(p_query_p99)
    .with_panel(p_utilization)
    .with_panel(p_memory)
    .with_panel(p_connections)
    # Row 2: Client Load
    .with_row(row2)
    .with_panel(p_clients_state)
    .with_panel(p_waiting_ts)
    .with_panel(p_wait_time_ts)
    # Row 3: Server Pool
    .with_row(row3)
    .with_panel(p_servers_state)
    .with_panel(p_pool_vs_active)
    .with_panel(p_pool_util_ts)
    # Row 4: Query Latency
    .with_row(row4)
    .with_panel(p_query_lat)
    .with_panel(p_qps)
    # Row 5: Transaction Latency
    .with_row(row5)
    .with_panel(p_xact_lat)
    .with_panel(p_tps)
    # Row 6: Traffic (collapsed)
    .with_row(row6)
    .with_panel(p_bytes_recv)
    .with_panel(p_bytes_sent)
    # Row 7: Pool Coordinator (collapsed)
    .with_row(row7)
    .with_panel(p_coord_conns)
    .with_panel(p_coord_reserve)
    .with_panel(p_coord_events)
    # Row 8: Pool Scaling (collapsed)
    .with_row(row8)
    .with_panel(p_inflight)
    .with_panel(p_scaling_events)
    .with_panel(p_conn_types)
    # Row 9: Prepared Statements (collapsed)
    .with_row(row9)
    .with_panel(p_cache_entries)
    .with_panel(p_cache_bytes)
    .with_panel(p_hit_ratio)
    .with_panel(p_client_named)
    .with_panel(p_client_anonymous)
    .with_panel(p_client_anonymous_evictions)
    # Row 10: Auth Query (collapsed)
    .with_row(row10)
    .with_panel(p_auth_cache)
    .with_panel(p_auth_outcomes)
    .with_panel(p_dynamic_pools)
    # Row 11: System (collapsed)
    .with_row(row11)
    .with_panel(p_memory_ts)
    .with_panel(p_sockets)
    # Row 12: Patroni-assisted fallback (collapsed)
    .with_row(row12)
    .with_panel(p_fallback_active)
    .with_panel(p_fallback_connections)
    .with_panel(p_patroni_api_errors)
    .with_panel(p_patroni_api_rate)
    .with_panel(p_patroni_api_duration)
    .with_panel(p_fallback_cache_hits)
    # Row 13: Query Interner (collapsed). Order is SLO-first: synthetic
    # misses and bytes go up top because they trip the alerts; entries
    # and eviction rate are drill-down; GC duration last.
    .with_row(row13)
    .with_panel(p_interner_synthetic_misses)
    .with_panel(p_interner_bytes)
    .with_panel(p_interner_entries)
    .with_panel(p_interner_evictions)
    .with_panel(p_interner_gc_duration)
    # Row 14: Pool State (collapsed)
    .with_row(row14)
    .with_panel(p_pool_paused)
    .with_panel(p_pool_maxwait)
    # Row 15: Pool Errors (collapsed)
    .with_row(row15)
    .with_panel(p_pool_errors_by_sqlstate)
    .with_panel(p_pool_errors_by_pool)
    # Row 16: Listener Rejections (collapsed)
    .with_row(row16)
    .with_panel(p_listener_rejections)
    # Row 17: Protocol Streaming (collapsed)
    .with_row(row17)
    .with_panel(p_streaming_events)
    .with_panel(p_streaming_bytes)
    # Row 18: Backend Setup Latency (collapsed)
    .with_row(row18)
    .with_panel(p_backend_phase_p99)
    .with_panel(p_backend_phase_p50)
    .with_panel(p_backend_phase_rate)
    # Row 19: Startup Parameters
    .with_row(row19)
    .with_panel(p_sp_errors_by_sqlstate)
    .with_panel(p_sp_errors_by_pool)
    .with_panel(p_sp_dropped_by_reason)
)

dashboard_obj = d.build()
result = json.loads(json.dumps(dashboard_obj, cls=JSONEncoder))

# For portable dashboards (grafana.com import): add __inputs and __requires
# so Grafana prompts the user to select a datasource on import.
if DS == "${DS_PROMETHEUS}":
    result["__inputs"] = [
        {
            "name": "DS_PROMETHEUS",
            "label": "Prometheus",
            "description": "Prometheus datasource for pg_doorman metrics",
            "type": "datasource",
            "pluginId": "prometheus",
            "pluginName": "Prometheus",
        }
    ]
    result["__requires"] = [
        {"type": "datasource", "id": "prometheus", "name": "Prometheus", "version": ""},
        {"type": "panel", "id": "stat", "name": "Stat", "version": ""},
        {"type": "panel", "id": "timeseries", "name": "Time series", "version": ""},
    ]

print(json.dumps(result, indent=2))
