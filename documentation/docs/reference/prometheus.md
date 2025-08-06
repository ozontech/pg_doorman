---
title: Prometheus Settings
---

# Prometheus Settings

pg_doorman includes a Prometheus metrics exporter that provides detailed insights into the performance and behavior of your connection pools. This document describes how to enable and use the Prometheus metrics exporter, as well as the available metrics.

## Enabling Prometheus Metrics

To enable the Prometheus metrics exporter, add the following configuration to your `pg_doorman.toml` file:

```toml
[prometheus]
enabled = true
host = "0.0.0.0"  # The host on which the metrics server will listen
port = 9127       # The port on which the metrics server will listen
```

### Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `enabled` | Enable or disable the Prometheus metrics exporter | `false` |
| `host` | The host on which the Prometheus metrics exporter will listen | `"0.0.0.0"` |
| `port` | The port on which the Prometheus metrics exporter will listen | `9127` |

## Configuring Prometheus

Add the following job to your Prometheus configuration to scrape metrics from pg_doorman:

```yaml
scrape_configs:
  - job_name: 'pg_doorman'
    static_configs:
      - targets: ['<pg_doorman_host>:9127']
```

Replace `<pg_doorman_host>` with the hostname or IP address of your pg_doorman instance.

## Available Metrics

pg_doorman exposes the following metrics:

### System Metrics

| Metric | Description |
|--------|-------------|
| `pg_doorman_total_memory` | Total memory allocated to the pg_doorman process in bytes. Monitors the memory footprint of the application. |

### Connection Metrics

| Metric | Description |
|--------|-------------|
| `pg_doorman_connection_count` | Counter of new connections by type handled by pg_doorman. Types include: 'plain' (unencrypted connections), 'tls' (encrypted connections), 'cancel' (connection cancellation requests), and 'total' (sum of all connections). |

### Socket Metrics (Linux only)

| Metric | Description |
|--------|-------------|
| `pg_doorman_sockets` | Counter of sockets used by pg_doorman by socket type. Types include: 'tcp' (IPv4 TCP sockets), 'tcp6' (IPv6 TCP sockets), 'unix' (Unix domain sockets), and 'unknown' (sockets of unrecognized type). Only available on Linux systems. |

### Pool Metrics

| Metric | Description |
|--------|-------------|
| `pg_doorman_pools_clients` | Number of clients in connection pools by status, user, and database. Status values include: 'idle' (connected but not executing queries), 'waiting' (waiting for a server connection), and 'active' (currently executing queries). Helps monitor connection pool utilization and client distribution. |
| `pg_doorman_pools_servers` | Number of servers in connection pools by status, user, and database. Status values include: 'active' (actively serving clients) and 'idle' (available for new connections). Helps monitor server availability and load distribution. |
| `pg_doorman_pools_bytes` | Total bytes transferred through connection pools by direction, user, and database. Direction values include: 'received' (bytes received from clients) and 'sent' (bytes sent to clients). Useful for monitoring network traffic and identifying high-volume connections. |

### Query and Transaction Metrics

| Metric | Description |
|--------|-------------|
| `pg_doorman_pools_queries_percentile` | Query execution time percentiles by user and database. Percentile values include: '99', '95', '90', and '50' (median). Values are in milliseconds. Helps identify slow queries and performance trends across different users and databases. |
| `pg_doorman_pools_transactions_percentile` | Transaction execution time percentiles by user and database. Percentile values include: '99', '95', '90', and '50' (median). Values are in milliseconds. Helps monitor transaction performance and identify long-running transactions that might impact database performance. |
| `pg_doorman_pools_transactions_count` | Counter of transactions executed in connection pools by user and database. Helps track transaction volume and identify users or databases with high transaction rates. |
| `pg_doorman_pools_transactions_total_time` | Total time spent executing transactions in connection pools by user and database. Values are in milliseconds. Helps monitor overall transaction performance and identify users or databases with high transaction execution times. |
| `pg_doorman_pools_queries_count` | Counter of queries executed in connection pools by user and database. Helps track query volume and identify users or databases with high query rates. |
| `pg_doorman_pools_queries_total_time` | Total time spent executing queries in connection pools by user and database. Values are in milliseconds. Helps monitor overall query performance and identify users or databases with high query execution times. |
| `pg_doorman_pools_avg_wait_time` | Average wait time for clients in connection pools by user and database. Values are in milliseconds. Helps monitor client wait times and identify potential bottlenecks. |

### Server Metrics

| Metric | Description |
|--------|-------------|
| `pg_doorman_servers_prepared_hits` | Counter of prepared statement hits in databases backends by user and database. Helps track the effectiveness of prepared statements in reducing query parsing overhead. |
| `pg_doorman_servers_prepared_misses` | Counter of prepared statement misses in databases backends by user and database. Helps identify queries that could benefit from being prepared to improve performance. |

## Grafana Dashboard

You can create a Grafana dashboard to visualize these metrics. Here's a simple example of panels you might want to include:

1. Connection counts by type
2. Memory usage over time
3. Client and server counts by pool
4. Query and transaction performance percentiles
5. Network traffic by pool

## Example Queries

Here are some example Prometheus queries that you might find useful:

### Connection Rate

```
rate(pg_doorman_connection_count{type="total"}[5m])
```

### Pool Utilization

```
sum by (database) (pg_doorman_pools_clients{status="active"}) / sum by (database) (pg_doorman_pools_servers{status="active"} + pg_doorman_pools_servers{status="idle"})
```

### Slow Queries

```
pg_doorman_pools_queries_percentile{percentile="99"}
```

### Client Wait Time

```
pg_doorman_pools_avg_wait_time
```
