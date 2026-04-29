# Patroni-assisted fallback

When pg_doorman runs next to PostgreSQL on the same machine and connects
via unix socket, a Patroni switchover or an unexpected PostgreSQL crash
leaves doorman without a backend. Until Patroni finishes promoting a
replica or restarting the local PostgreSQL, every client query fails.

Patroni-assisted fallback bridges that gap. When the local PostgreSQL
stops responding, pg_doorman queries the Patroni REST API, picks another
cluster member, and routes new connections there. Existing pooled
connections to the dead backend are recycled normally.

This is a short-term measure. It bridges the 10-30 seconds while
Patroni completes its own failover. Once Patroni restores the local
PostgreSQL — as a replica of the new primary, or as the recovered
primary itself — pg_doorman returns to the local socket.

## Quick start

The recommended deployment puts pg_doorman next to PostgreSQL on the
same host and talks to it through the unix socket. With Patroni's REST
API also on `localhost`, fallback turns on with one line in `[general]`:

```yaml
general:
  patroni_api_urls: ["http://localhost:8008"]
```

Every pool picks this up automatically. When the unix socket stops
responding, pg_doorman queries `/cluster`, prefers `sync_standby` over
`replica` over leader, and routes new connections to the chosen host
until the local PostgreSQL recovers. Defaults: cooldown 30s, HTTP
timeout 5s, TCP timeout 5s, fallback connection lifetime 30s. Override
them under [Tuning parameters](#tuning-parameters).

## When it helps

**Planned switchover.** A DBA runs `patroni switchover --candidate node2`.
Patroni promotes node2, then shuts down PostgreSQL on node1. Between the
shutdown and Patroni restarting node1 as a replica of node2, doorman on
node1 has no backend. With fallback enabled, the next client request
that fails to reach the local socket triggers a `/cluster` lookup and
the new connection is opened to node2.

**Unplanned crash.** PostgreSQL on node1 is killed by the OOM killer.
Patroni hasn't detected the failure yet. Doorman gets connection refused
on the unix socket, queries the Patroni API, and connects to the
`sync_standby` (most likely the next leader).

## When it does not help

**Machine failure.** If the entire machine is down, doorman dies with it.
No fallback logic can run. This scenario requires external routing
(HAProxy, patroni_proxy, DNS failover, VIP).

**Authentication errors.** If PostgreSQL rejects doorman's credentials,
the backend is alive. Fallback does not activate.

## How it works

```
Normal:
  client --unix--> doorman --unix--> PostgreSQL (local)

Fallback:
  client --unix--> doorman --TCP---> PostgreSQL (remote, from /cluster)
                      |
                      +-- GET /cluster --> Patroni API
```

1. Doorman tries the local unix socket.
2. Connection refused or socket error: doorman puts the local backend
   into cooldown for `fallback_cooldown` (default 30 seconds).
3. Doorman sends `GET /cluster` to all configured Patroni URLs
   **in parallel** and takes the first successful response.
4. From the member list, doorman builds a TCP-alive candidate list
   ordered by priority: `sync_standby` first, then `replica`, then any
   other member. TCP probe runs in parallel; non-responding candidates
   drop out of the list.
5. Doorman walks the list and runs `Server::startup` against each
   candidate, bounded by `fallback_connect_timeout` (default 5 seconds).
   The first candidate to complete startup wins.
6. If a candidate refuses startup (auth error, `database is starting
   up`, timeout), doorman marks it unhealthy and tries the next one.
   On exhaustion the doorman log records
   `all fallback candidates rejected (3 startup_error, 1 timeout)`
   aggregated by failure reason. The client always sees the same
   sanitized FATAL pg_doorman uses for startup-time errors —
   `Unable to retrieve server parameters … may be unavailable or
   misconfigured` — read the doorman log for the breakdown.
7. The successful connection enters the pool with a **reduced lifetime**
   (default 30 seconds, matching the cooldown). It follows all normal
   pool rules: coordinator limits, idle timeout, recycle.
8. Subsequent connections during the cooldown go to the same fallback
   host directly, without re-querying the Patroni API. If that cached
   host fails on a later startup, doorman clears the cache and runs
   one extra discovery round.
9. When the cooldown expires, doorman tries the local socket again.
   If it works, normal mode resumes. If not, the cycle repeats.

### Wait time bounds

A client never waits for fallback longer than `query_wait_timeout`
(default 5 seconds). When that deadline elapses with no candidate
ready, the doorman log records `fallback total deadline {ms}ms
exceeded` and the client sees the same sanitized FATAL it gets for
any startup-time failure. This is the same deadline the rest of
pg_doorman uses to limit how long a client waits for any server
connection — fallback inherits it so a slow Patroni member or a long
candidate list cannot push past it.

### Per-host cooldown

A candidate that fails startup stays out of the next discovery for
`fallback_connect_timeout` (default 5 seconds). Each consecutive
failure on the same host doubles the cooldown, capped at 60 seconds.
After the window elapses the entry is dropped and the counter resets
on the next failure. This prevents a stuck candidate (postgres in
recovery, persistent auth misconfiguration, slow network path) from
being retried on every client request and hammering both the
candidate and the Patroni API.

The cooldown map is bounded at 256 entries; expired entries are
pruned before any new insert past that mark.

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

Add `patroni_api_urls` to any pool that should use fallback.
Without this setting, the feature is disabled and doorman behaves
as before.

```yaml
pools:
  mydb:
    pool_mode: transaction
    server_host: "/var/run/postgresql"
    server_port: 5432

    # Patroni API endpoints. Specify at least 2 for redundancy.
    # The first URL that responds wins; order does not matter.
    patroni_api_urls:
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

patroni_api_urls = [
    "http://10.0.0.1:8008",
    "http://10.0.0.2:8008",
    "http://10.0.0.3:8008",
]
```

### Tuning parameters

All parameters are optional and have sensible defaults.

| Parameter | Default | Description |
|-----------|---------|-------------|
| `fallback_cooldown` | `"30s"` | How long the local backend stays marked as down after a failed connect. During this window, all new connections go to the fallback host. |
| `patroni_api_timeout` | `"5s"` | HTTP timeout for Patroni API requests. Applies per URL; since all URLs are queried in parallel, the effective timeout is this value, not multiplied by the number of URLs. |
| `fallback_connect_timeout` | `"5s"` | TCP probe timeout, per-candidate `Server::startup` deadline, and the per-host cooldown base after a failed startup. The same parameter governs all three because they share the "candidate looks unresponsive" semantics. |
| `fallback_lifetime` | same as `fallback_cooldown` | Lifetime of fallback connections. Shorter than normal `server_lifetime` so the pool returns to the local backend quickly after recovery. |
| `connect_timeout` (`[general]`) | `"3s"` | Deadline for the local-backend `Server::startup`, in addition to its existing role for alive-check and TCP probe. Raise this if your local PostgreSQL has slow startup (large WAL replay, big `shared_buffers` warmup). |
| `query_wait_timeout` (`[general]`) | `"5s"` | Outer deadline for the entire fallback path. The client never waits longer than this for a server connection, regardless of how many candidates are walked. |

### What to put in `patroni_api_urls`

List the Patroni REST API addresses of your cluster nodes. The
`/cluster` endpoint on any Patroni node returns the full cluster
topology, so even a single URL is enough to enumerate all members.

Two or more URLs are recommended: if the first URL points to the same
machine as the dead PostgreSQL, it won't respond either. Doorman
queries all URLs in parallel and takes the first response.

## Prometheus metrics

| Metric | Type | Description |
|--------|------|-------------|
| `pg_doorman_patroni_api_requests_total` | counter | Number of `/cluster` requests made |
| `pg_doorman_fallback_connections_total` | counter | Fallback connections created |
| `pg_doorman_patroni_api_errors_total` | counter | Failed `/cluster` requests (all URLs unreachable) |
| `pg_doorman_fallback_active` | gauge | 1 while the local backend is in cooldown and the pool is using a fallback |
| `pg_doorman_fallback_host` | gauge | Currently active fallback host (1 = active). Labels: pool, host, port |
| `pg_doorman_fallback_cache_hits_total` | counter | Cached fallback host reused without re-querying Patroni |
| `pg_doorman_fallback_candidate_failures_total` | counter | Per-candidate startup failure. Labels: `pool`, `reason` (`connect_error`, `startup_error`, `server_unavailable`, `timeout`, `other`). Use this to tell apart "everyone refused on auth" from "kernel-level connectivity broken" during exhaustion. |
| `pg_doorman_patroni_api_duration_seconds` | histogram | Time spent fetching `/cluster` |

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
local backend. If the local backend uses a unix socket (no TLS),
fallback TCP connections will also run without TLS. Configure
`server_tls_mode` explicitly if fallback connections must be encrypted.

**DNS.** Use IP addresses in `patroni_api_urls` and in Patroni
`member.host`, not hostnames. The startup-timeout wrapper covers DNS
resolution via `TcpStream::connect`, but a 5s DNS hang consumes the
full `fallback_connect_timeout` budget for that candidate before the
next one is tried.

**Log volume under failure storm.** The per-candidate
`<host>:<port> rejected (...)` WARN is rate-limited to one line per
10 seconds per `(pool, host, port)`. Suppressed lines log at DEBUG.
If you see only one WARN where you expected many, that's the
rate-limit, not lost data — check the
`pg_doorman_fallback_candidate_failures_total` counter for the real
attempt count.

**Whitelist switchover and `pg_doorman_fallback_host`.** When the
fallback target changes (cooldown drains, retry round picks a
different host), the gauge for the previous `(host, port)` is
removed atomically with the gauge for the new one being set.
Dashboards do not see two hosts marked active at once during the
transition.

**standby_leader.** Patroni standby clusters use the `standby_leader`
role. doorman treats it as "other" (lowest priority, after sync_standby
and replica). For a primary-cluster deployment this matches what you
want; if you are running pg_doorman on a standby cluster you most
likely don't want fallback at all because you have no writeable target.

## Relationship to patroni_proxy

patroni_proxy and Patroni-assisted fallback solve different problems.

**patroni_proxy** is a TCP load balancer deployed near application
clients. It routes connections to the correct PostgreSQL node based on
role (leader, sync, async). It does not pool connections.

**Patroni-assisted fallback** is built into the doorman pooler deployed
next to PostgreSQL. It handles the case where the local backend dies and
doorman needs a temporary alternative. It does pool connections.

In the recommended deployment (patroni_proxy → pg_doorman → PostgreSQL),
fallback keeps read traffic flowing at the doorman layer when the local
backend dies, without affecting patroni_proxy routing.
