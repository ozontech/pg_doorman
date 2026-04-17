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
    "Avg Wait Time",
    f'max(pg_doorman_pools_avg_wait_time{{{S}}})',
    unit="ms",
    thresholds=[(None, "green"), (5, "yellow"), (50, "red")],
    desc="Max across pools of average queue wait — adds directly to application latency. Above 50ms: check Pool Utilization and raise pool_size.",
)
p_query_p99 = stat_panel(
    "Query p99",
    f'max(pg_doorman_pools_queries_percentile{{percentile="99", {S}}})',
    unit="ms",
    thresholds=[(None, "green"), (50, "yellow"), (200, "red")],
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
    'pg_doorman_connection_count{type="total", instance=~"$instance"}',
    thresholds=[(None, "blue")],
    color_mode="none",
    desc="Current client connections (all pools). Compare with pool_size for multiplexing ratio — 100:1+ is normal in transaction mode.",
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
    "Avg Wait Time", [
        prom(f'pg_doorman_pools_avg_wait_time{{{S}}}', "{{user}}@{{database}}"),
    ], unit="ms", w=8,
    desc="Average queue time per pool. Pool with low wait count but high wait time has slow query turnover.",
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
        prom(f'max by (database) (pg_doorman_pools_queries_percentile{{percentile="50", {S}}})', "p50"),
        prom(f'max by (database) (pg_doorman_pools_queries_percentile{{percentile="90", {S}}})', "p90"),
        prom(f'max by (database) (pg_doorman_pools_queries_percentile{{percentile="95", {S}}})', "p95"),
        prom(f'max by (database) (pg_doorman_pools_queries_percentile{{percentile="99", {S}}})', "p99"),
    ], unit="ms",
    desc="Server-side query time at p50/p90/p95/p99. p99 diverging from p50 — check pg_stat_activity for lock waits or long-running queries.",
)
p_qps = ts_panel(
    "Queries per Second", [
        prom(f'rate(pg_doorman_pools_queries_count{{{S}}}[$__rate_interval])', "{{user}}@{{database}}"),
    ], unit="ops",
    desc="Query throughput per pool. Flat QPS with rising latency signals PostgreSQL saturation. Rising QPS with stable latency is healthy growth.",
)

# ---------------------------------------------------------------------------
# Row 5: Transaction Latency
# ---------------------------------------------------------------------------
row5 = expanded_row("Transaction Latency")

p_xact_lat = ts_panel(
    "Transaction Latency Percentiles", [
        prom(f'max by (database) (pg_doorman_pools_transactions_percentile{{percentile="50", {S}}})', "p50"),
        prom(f'max by (database) (pg_doorman_pools_transactions_percentile{{percentile="90", {S}}})', "p90"),
        prom(f'max by (database) (pg_doorman_pools_transactions_percentile{{percentile="95", {S}}})', "p95"),
        prom(f'max by (database) (pg_doorman_pools_transactions_percentile{{percentile="99", {S}}})', "p99"),
    ], unit="ms",
    desc="End-to-end transaction time including all queries and inter-query gaps. High values with low query latency indicate application-side delays between queries.",
)
p_tps = ts_panel(
    "Transactions per Second", [
        prom(f'rate(pg_doorman_pools_transactions_count{{{S}}}[$__rate_interval])', "{{user}}@{{database}}"),
    ], unit="ops",
    desc="Transaction throughput per pool. Drop with rising latency indicates lock contention or long transactions holding server connections.",
)

# ---------------------------------------------------------------------------
# Row 6: Traffic (collapsed)
# ---------------------------------------------------------------------------
row6 = collapsed_row("Traffic")

p_bytes_recv = ts_panel(
    "Bytes Received", [
        prom(f'rate(pg_doorman_pools_bytes{{direction="received", {S}}}[$__rate_interval])', "{{user}}@{{database}}"),
    ], unit="Bps",
    desc="Data rate from clients. Spikes correlate with bulk INSERTs/COPYs.",
)
p_bytes_sent = ts_panel(
    "Bytes Sent", [
        prom(f'rate(pg_doorman_pools_bytes{{direction="sent", {S}}}[$__rate_interval])', "{{user}}@{{database}}"),
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
        prom('pg_doorman_connection_count{type="plain", instance=~"$instance"}', "plain"),
        prom('pg_doorman_connection_count{type="tls", instance=~"$instance"}', "tls"),
        prom('pg_doorman_connection_count{type="cancel", instance=~"$instance"}', "cancel"),
    ], w=8,
    desc="Connections by protocol. Track TLS adoption. Elevated cancel count indicates application timeouts.",
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
            f'sum by (user, database) (pg_doorman_servers_prepared_hits{{{S}}}) / '
            f'clamp_min(sum by (user, database) (pg_doorman_servers_prepared_hits{{{S}}}) + '
            f'sum by (user, database) (pg_doorman_servers_prepared_misses{{{S}}}), 1)'
            f', 1)',
            "{{user}}@{{database}}",
        ),
    ], unit="percentunit", w=8,
    desc="Cache hits / total lookups. Below 90%: servers frequently re-parse after multiplexing. Ensure consistent statement names.",
)

# ---------------------------------------------------------------------------
# Row 10: Auth Query (collapsed)
# ---------------------------------------------------------------------------
row10 = collapsed_row("Auth Query")

p_auth_cache = ts_panel(
    "Auth Cache Hit Rate", [
        prom(
            f'clamp_max('
            f'rate(pg_doorman_auth_query_cache{{type="hits", {SD}}}[$__rate_interval]) / '
            f'clamp_min(rate(pg_doorman_auth_query_cache{{type="hits", {SD}}}[$__rate_interval]) + '
            f'rate(pg_doorman_auth_query_cache{{type="misses", {SD}}}[$__rate_interval]), 0.001)'
            f', 1)',
            "{{database}}",
        ),
    ], unit="percentunit", w=8,
    desc="Auth lookups served from cache vs PostgreSQL query. Low rate adds latency to every new connection — increase cache_ttl.",
)
p_auth_outcomes = ts_panel(
    "Auth Outcomes", [
        prom(f'rate(pg_doorman_auth_query_auth{{result="success", {SD}}}[$__rate_interval])', "success/s"),
        prom(f'rate(pg_doorman_auth_query_auth{{result="failure", {SD}}}[$__rate_interval])', "failure/s"),
    ], w=8,
    desc="Auth success vs failure rate. Failure spike after deploy = credential mismatch. Sustained failures = check source IPs in logs.",
)
p_dynamic_pools = ts_panel(
    "Dynamic Pools", [
        prom(f'pg_doorman_auth_query_dynamic_pools{{type="current", {SD}}}', "current"),
    ], w=8,
    desc="Auto-created pools for auth_query users. Unexpected growth indicates wrong database names or unplanned user sprawl.",
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
    # Row 10: Auth Query (collapsed)
    .with_row(row10)
    .with_panel(p_auth_cache)
    .with_panel(p_auth_outcomes)
    .with_panel(p_dynamic_pools)
    # Row 11: System (collapsed)
    .with_row(row11)
    .with_panel(p_memory_ts)
    .with_panel(p_sockets)
)

dashboard_obj = d.build()
print(json.dumps(dashboard_obj, cls=JSONEncoder, indent=2))
