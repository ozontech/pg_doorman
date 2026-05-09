# pg_doorman Grafana dashboard

The dashboard is generated from `generate_dashboard.py` (Python +
[grafana-foundation-sdk](https://pypi.org/project/grafana-foundation-sdk/));
`pg_doorman.json` is the artefact and gets committed alongside the
generator. Edit the Python, regenerate, commit both.

## Layout

`pg_doorman.json` ships 13 rows. The first five expand by default;
the rest collapse so the dashboard opens cleanly.

1. **Overview** — waiting clients, wait time, query P99,
   utilization, memory, connections.
2. **Client Load** — clients by state, waiters, wait timing.
3. **Server Pool** — servers by state, active vs pool size,
   pool utilization.
4. **Query Latency** — query latency, QPS.
5. **Transaction Latency** — transaction latency, TPS.
6. **Traffic** — bytes in/out.
7. **Pool Coordinator** — priority arbitration metrics.
8. **Pool Scaling** — warm pool, fast retries.
9. **Prepared Statements** — pool/client cache, hit ratio,
   anonymous LRU evictions.
10. **Auth Query** — auth cache hit rate, success/failure,
    dynamic pools.
11. **System** — process memory, sockets.
12. **Patroni-assisted fallback** — active flag, API rate,
    errors, duration, cache.
13. **Query Interner** — entries and bytes per kind, eviction
    rate, synthetic SQLSTATE 26000, GC sweep duration. New in
    3.7.0.

Interner, fallback, and Patroni panels are process-global and
filter only by `$instance`; the rest scope through
`$instance` / `$user` / `$database`.

## Import for an existing Grafana

Import `pg_doorman.json`. Grafana asks for the Prometheus
datasource; pick the one that scrapes pg_doorman.

pg_doorman must export Prometheus metrics:

```toml
[prometheus]
enabled = true
port = 9127
```

## Regenerate

```bash
pip install grafana-foundation-sdk
GRAFANA_DS_UID='${DS_PROMETHEUS}' python3 generate_dashboard.py > pg_doorman.json
```

`GRAFANA_DS_UID='${DS_PROMETHEUS}'` (the literal placeholder, not a
shell expansion) keeps the dashboard portable: importers see a
"select datasource" prompt. Use a concrete UID like
`GRAFANA_DS_UID=prometheus` only when wiring the dashboard into a
provisioned environment whose datasource UID is known.

## Demo (docker compose)

```bash
cd demo/
docker build -t pg_doorman:ubuntu2204-tls -f ../../Dockerfile.ubuntu22-tls ../..
GRAFANA_DS_UID=prometheus python3 ../generate_dashboard.py > grafana/provisioning/dashboards/pg_doorman.json
docker compose up -d
```

The first command builds the image referenced by
`docker-compose.yml`. The second regenerates the dashboard with the
provisioned datasource UID `prometheus` matching
`grafana/provisioning/datasources/prometheus.yml`. The third brings
up Postgres, pg_doorman, Prometheus, Grafana, two pgbench load
generators (`pgbench.sh`, `pgbench2.sh`) hammering the pool with
distinct user identities, and a `listener` sidecar that holds three
LISTEN sessions on a session-mode pool — so the Web UI shows
`pool_mode = session` alongside the transaction-mode pgbench traffic.

- Grafana: http://localhost:3000 (anonymous admin login).
- Prometheus: http://localhost:19090.
- pg_doorman Web UI: http://localhost:9127 (admin / `doorman_demo`).

```bash
docker compose down -v
```

removes the volumes too.

## Validation

Two scripts under `scripts/` exercise the demo and report panel
health. Both are run by `make` targets so a CI job and a developer
laptop hit the same flow.

```bash
make dashboard-smoke           # parse pg_doorman.json, query Prometheus
make dashboard-ground-truth    # compare PromQL with logs / pg_stat / TOML / /proc
make dashboard-validate        # both layers in one warmup
make dashboard-validate-ci     # validate + always tear down (used by CI)
```

`scripts/dashboard-smoke.expected.yaml` documents bounds and the
`allow_empty` list (panels for which empty data on the demo is
expected — Patroni-fallback not configured, streaming events not
produced, etc.). `scripts/dashboard-ground-truth.checks.yaml` lists
14 checks: throughput against `pg_stat_database`, latency p99 against
HDR percentiles in the log, active-server counts against
`pg_stat_activity`, RSS against `/proc/1/status`, pool sizes against
the TOML config. The full plan is in
`.github/workflows/dashboard-validation.yml`.
