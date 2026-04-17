# pg_doorman Grafana dashboard

## Import

Import `pg_doorman.json` into Grafana. Select your Prometheus datasource when prompted.

pg_doorman must have the Prometheus exporter enabled:

```toml
[prometheus]
enabled = true
port = 9127
```

## Regenerate

```bash
pip install grafana-foundation-sdk
python3 generate_dashboard.py > pg_doorman.json
```

For portable dashboards (grafana.com import):

```bash
GRAFANA_DS_UID='${DS_PROMETHEUS}' python3 generate_dashboard.py > pg_doorman.json
```

## Demo

Local demo with PostgreSQL, pg_doorman, Prometheus, Grafana, and pgbench load:

```bash
cd demo/
docker compose up -d
```

- Grafana: http://localhost:3000
- Prometheus: http://localhost:19090

```bash
docker compose down -v
```
