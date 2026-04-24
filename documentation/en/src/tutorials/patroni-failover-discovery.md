# Patroni failover discovery

When pg_doorman runs next to PostgreSQL on the same machine and connects
via unix socket, a Patroni switchover or an unexpected PostgreSQL crash
leaves doorman without a backend. Every client query fails until the DBA
reconfigures doorman or the local PostgreSQL comes back.

Patroni failover discovery lets doorman bridge that gap automatically.
When the local PostgreSQL stops responding, doorman queries the Patroni
REST API, finds another cluster member, and routes new connections there.
Existing pooled connections to the dead backend are recycled normally.

This is a short-term measure. It covers the 10-30 seconds of a typical
Patroni switchover. Long-term routing (pointing doorman at the new
primary permanently) remains the DBA's responsibility via config reload.

## When it helps

**Planned switchover.** A DBA runs `patroni switchover --candidate node2`.
Patroni promotes node2, then shuts down PostgreSQL on node1. Between the
shutdown and the config update, doorman on node1 has no backend. With
discovery enabled, doorman connects to node2 within 1-2 TCP round trips.

**Unplanned crash.** PostgreSQL on node1 is killed by the OOM killer.
Patroni hasn't detected the failure yet. Doorman gets connection refused
on the unix socket, queries the Patroni API, and connects to the
`sync_standby` (most likely the next leader).

## When it does not help

**Machine failure.** If the entire machine is down, doorman dies with it.
No failover logic can run. This scenario requires external routing
(HAProxy, patroni_proxy, DNS failover, VIP).

**Authentication errors.** If PostgreSQL rejects doorman's credentials,
the backend is alive. Discovery does not activate.

## How it works

```
Normal:
  client --unix--> doorman --unix--> PostgreSQL (local)

Failover:
  client --unix--> doorman --TCP---> PostgreSQL (remote, from /cluster)
                      |
                      +-- GET /cluster --> Patroni API
```

1. `ServerPool::create()` tries the local unix socket.
2. Connection refused or socket error: doorman blacklists the local
   host for `failover_blacklist_duration` (default 30 seconds).
3. Doorman sends `GET /cluster` to all configured Patroni URLs
   **in parallel** and takes the first successful response.
4. From the member list, doorman picks the first available host:
   `sync_standby` first, then `replica`, then any other member.
   TCP connect to all candidates runs in parallel; if a `sync_standby`
   responds, it is chosen immediately over any replica.
5. The new connection enters the pool with a **reduced lifetime**
   (default 30 seconds, matching the blacklist duration). It follows
   all normal pool rules: coordinator limits, idle timeout, recycle.
6. Subsequent `create()` calls during the blacklist window connect
   to the same fallback host directly, without querying the Patroni
   API again.
7. When the blacklist expires, doorman tries the local socket again.
   If it works, normal mode resumes. If not, the cycle repeats.

## Write queries on a replica

If the fallback host is a replica that hasn't been promoted yet,
write queries return:

```
ERROR: cannot execute INSERT in a read-only transaction
```

Read queries work normally. In a typical switchover, `sync_standby`
is promoted before doorman even detects the failure, so most write
queries succeed. Worst case, write errors last until the reduced
lifetime expires (30 seconds) and the next connection attempt finds
the new primary via a fresh `/cluster` call.

## Configuration

Add `patroni_discovery_urls` to any pool that should use discovery.
Without this setting, the feature is completely disabled and doorman
behaves as before.

```yaml
pools:
  mydb:
    pool_mode: transaction
    server_host: "/var/run/postgresql"
    server_port: 5432

    # Patroni API endpoints. Specify at least 2 for redundancy.
    # The first URL that responds wins; order does not matter.
    patroni_discovery_urls:
      - "http://10.0.0.1:8008"
      - "http://10.0.0.2:8008"
      - "http://10.0.0.3:8008"
```

TOML equivalent:

```toml
[pools.mydb]
pool_mode = "transaction"
server_host = "/var/run/postgresql"
server_port = 5432

patroni_discovery_urls = [
    "http://10.0.0.1:8008",
    "http://10.0.0.2:8008",
    "http://10.0.0.3:8008",
]
```

### Tuning parameters

All parameters are optional and have sensible defaults.

| Parameter | Default | Description |
|-----------|---------|-------------|
| `failover_blacklist_duration` | `"30s"` | How long the local host stays blacklisted after a connection failure. During this window, all new connections go to the fallback host. |
| `failover_discovery_timeout` | `"5s"` | HTTP timeout for Patroni API requests. Applies per URL; since all URLs are queried in parallel, the effective timeout is this value, not multiplied by the number of URLs. |
| `failover_connect_timeout` | `"5s"` | TCP connect timeout for fallback servers. Applies to the entire parallel connect batch, not per member. |
| `failover_server_lifetime` | same as `failover_blacklist_duration` | Lifetime of fallback connections. Shorter than normal `server_lifetime` to ensure doorman returns to the local host quickly after switchover completes. |

### What to put in `patroni_discovery_urls`

List the Patroni REST API addresses of your cluster nodes. The
`/cluster` endpoint on any Patroni node returns the full cluster
topology, so even a single URL is enough to discover all members.

Two or more URLs are recommended: if the first URL points to the same
machine as the dead PostgreSQL, it won't respond either. Doorman
queries all URLs in parallel and takes the first response.

## Prometheus metrics

| Metric | Type | Description |
|--------|------|-------------|
| `pg_doorman_failover_discovery_total` | counter | Number of `/cluster` requests made |
| `pg_doorman_failover_connections_total` | counter | Fallback connections created |
| `pg_doorman_failover_discovery_errors_total` | counter | Failed `/cluster` requests (all URLs unreachable) |
| `pg_doorman_failover_host_blacklisted` | gauge | 1 if the primary host is currently blacklisted |
| `pg_doorman_failover_discovery_duration_seconds` | histogram | Time spent fetching `/cluster` |

## Active transactions

If PostgreSQL crashes while a client is in the middle of a transaction,
the client receives a connection error. doorman does not migrate
in-flight transactions to a fallback host — the client must retry.

New queries from the same or other clients go through the fallback path
automatically.

## Operational notes

**Credentials.** All cluster nodes must accept the same username and
password that doorman uses. Patroni clusters typically share
`pg_hba.conf` via bootstrap configuration, but this is not guaranteed.
Verify that fallback nodes accept the configured credentials.

**TLS.** Fallback connections use the same `server_tls_mode` as the
primary. If the primary uses a unix socket (no TLS), fallback TCP
connections will also run without TLS. Configure `server_tls_mode`
explicitly if fallback connections must be encrypted.

**DNS.** Use IP addresses in `patroni_discovery_urls`, not hostnames.
DNS resolution failure during a failover adds latency and may cause
discovery to fail entirely.

**standby_leader.** Patroni standby clusters use the `standby_leader`
role. doorman treats it as "other" (lowest priority, after sync_standby
and replica). This is correct for most deployments.

## Relationship to patroni_proxy

patroni_proxy and failover discovery solve different problems.

**patroni_proxy** is a TCP load balancer deployed near application
clients. It routes connections to the correct PostgreSQL node based on
role (leader, sync, async). It does not pool connections.

**Failover discovery** is built into the doorman pooler deployed next to
PostgreSQL. It handles the case where the local backend dies and doorman
needs a temporary alternative. It does pool connections.

In the recommended deployment (patroni_proxy -> pg_doorman -> PostgreSQL),
failover discovery adds resilience at the doorman layer without
affecting the patroni_proxy layer.
