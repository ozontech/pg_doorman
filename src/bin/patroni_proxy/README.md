# patroni_proxy

High-performance TCP proxy for Patroni-managed PostgreSQL clusters.

## Overview

`patroni_proxy` is a specialized TCP proxy designed to route connections to the appropriate PostgreSQL nodes in a Patroni cluster based on their roles (leader, sync replica, async replica). Unlike traditional solutions like HAProxy + confd, it provides seamless connection management without disrupting existing connections during cluster topology changes.

## Key Features

### Zero-Downtime Connection Management

The main advantage over HAProxy + confd is that `patroni_proxy` **does not terminate existing connections** when the upstream configuration changes. When a new replica is added or removed, only the affected connections are handled - all other connections continue working without interruption.

### Hot Upstream Updates

- Automatic discovery of cluster members via Patroni REST API (`/cluster` endpoint)
- Periodic polling with configurable interval (`cluster_update_interval`)
- Immediate updates via HTTP API (`/update_clusters` endpoint)
- Configuration reload via SIGHUP signal without restart

### Role-Based Routing

Route connections based on PostgreSQL node roles:
- `leader` - primary/master node
- `sync` - synchronous standby replicas
- `async` - asynchronous replicas
- `any` - any available node

### Intelligent Load Balancing

- **Least Connections** strategy for distributing connections across backends
- Connection counters are preserved during cluster updates
- Automatic exclusion of nodes with `noloadbalance` tag

### Replication Lag Awareness

- Configurable `max_lag_in_bytes` per port
- Automatic disconnection of clients when replica lag exceeds threshold
- Only affects replica connections (leader has no lag)

### Member State Filtering

- Only members with `state: "running"` are used as backends
- Members in `starting`, `stopped`, `crashed` states are automatically excluded
- Dynamic state changes are handled during periodic updates

## Configuration

Example `patroni_proxy.yaml`:

```yaml
# Cluster update interval in seconds (default: 3)
cluster_update_interval: 3

# HTTP API listen address for health checks and manual updates (default: 127.0.0.1:8009)
listen_address: "127.0.0.1:8009"

clusters:
  my_cluster:
    # Patroni API endpoints (multiple for redundancy)
    hosts:
      - "http://192.168.1.1:8008"
      - "http://192.168.1.2:8008"
      - "http://192.168.1.3:8008"
    
    # Optional: TLS configuration for Patroni API
    # tls:
    #   ca_cert: "/path/to/ca.crt"
    #   client_cert: "/path/to/client.crt"
    #   client_key: "/path/to/client.key"
    #   skip_verify: false
    
    ports:
      # Primary/master connections
      master:
        listen: "0.0.0.0:6432"
        roles: ["leader"]
        host_port: 5432
      
      # Read-only connections to replicas
      replicas:
        listen: "0.0.0.0:6433"
        roles: ["sync", "async"]
        host_port: 5432
        max_lag_in_bytes: 16777216  # 16MB
```

## Usage

```bash
# Start with configuration file
patroni_proxy /path/to/patroni_proxy.yaml

# Reload configuration (add/remove ports, update hosts)
kill -HUP $(pidof patroni_proxy)

# Trigger immediate cluster update via API
curl http://127.0.0.1:8009/update_clusters
```

## HTTP API

- `GET /update_clusters` - Trigger immediate update of all cluster members
- `GET /` - Health check (returns "OK")

## Comparison with HAProxy + confd

| Feature | patroni_proxy | HAProxy + confd |
|---------|---------------|-----------------|
| Connection preservation on update | ✅ Yes | ❌ No (reload drops connections) |
| Hot upstream updates | ✅ Native | ⚠️ Requires confd + reload |
| Replication lag awareness | ✅ Built-in | ⚠️ Requires custom checks |
| Configuration complexity | ✅ Single YAML | ❌ Multiple configs |
| Resource usage | ✅ Lightweight | ⚠️ HAProxy + confd processes |

## Building

```bash
# Build release binary
cargo build --release --bin patroni_proxy

# Run tests
cargo test --test patroni_proxy_bdd
```

## License

MIT
