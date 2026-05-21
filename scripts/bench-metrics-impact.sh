#!/bin/bash
# Bench: impact of /metrics scraping on client p50/p95/p99 under high RPS.
#
# Two runs back to back against the same pg_doorman + PostgreSQL:
#   A. baseline — no /metrics traffic
#   B. under scrape — a tight `curl` loop hammers /metrics for the whole run
#
# pgbench writes per-transaction logs; we compute p50/p95/p99 from them and
# print a comparison table. Numbers are local-machine ballpark, not
# production figures — the point is the *delta* between A and B.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORKDIR=$(mktemp -d -t doorman-metrics-bench-XXXXXX)

PG_PORT=${PG_PORT:-15432}
DOORMAN_PORT=${DOORMAN_PORT:-15433}
METRICS_PORT=${METRICS_PORT:-15434}
PGBENCH_CLIENTS=${PGBENCH_CLIENTS:-2000}
PGBENCH_JOBS=${PGBENCH_JOBS:-16}
PGBENCH_DURATION=${PGBENCH_DURATION:-30}
POOL_SIZE=${POOL_SIZE:-40}
SCRAPE_CONCURRENCY=${SCRAPE_CONCURRENCY:-10}

DOORMAN_PID=""
SCRAPE_PID=""

cleanup() {
    local rc=$?
    set +e
    [ -n "$SCRAPE_PID" ] && kill "$SCRAPE_PID" 2>/dev/null
    [ -n "$DOORMAN_PID" ] && kill "$DOORMAN_PID" 2>/dev/null && wait "$DOORMAN_PID" 2>/dev/null
    if [ -d "$WORKDIR/pg" ] && [ -f "$WORKDIR/pg/postmaster.pid" ]; then
        pg_ctl -D "$WORKDIR/pg" stop -m fast >/dev/null 2>&1 || true
    fi
    if [ $rc -eq 0 ]; then
        echo
        echo "==> workdir kept for analysis: $WORKDIR"
    else
        echo
        echo "==> workdir kept (exit $rc): $WORKDIR"
    fi
}
trap cleanup EXIT INT TERM

echo "==> workdir: $WORKDIR"

# 1. PostgreSQL ---------------------------------------------------------------
echo "==> initdb"
initdb -D "$WORKDIR/pg" --auth=trust -U postgres --no-locale --no-sync >"$WORKDIR/initdb.log" 2>&1
cat > "$WORKDIR/pg/postgresql.conf" <<EOF
port = $PG_PORT
listen_addresses = '127.0.0.1'
unix_socket_directories = '$WORKDIR'
fsync = off
synchronous_commit = off
shared_buffers = 256MB
# Only pg_doorman opens backend connections to PG (capped by pool_size +
# admin overhead). Clients connect to pg_doorman, not to PG directly.
max_connections = 200
log_min_messages = warning
EOF
echo "host all all 127.0.0.1/32 trust" > "$WORKDIR/pg/pg_hba.conf"

echo "==> pg_ctl start"
pg_ctl -D "$WORKDIR/pg" -l "$WORKDIR/pg.log" start >/dev/null

# 2. pg_doorman config --------------------------------------------------------
DOORMAN_CONFIG="$WORKDIR/pg_doorman.toml"
cat > "$DOORMAN_CONFIG" <<EOF
[general]
host = "127.0.0.1"
port = $DOORMAN_PORT
admin_username = "admin"
admin_password = "admin"
pg_hba.content = "host all all 127.0.0.1/32 trust"
worker_threads = 4
max_connections = 11000

[web]
enabled = true
host = "127.0.0.1"
port = $METRICS_PORT

[pools.postgres]
server_host = "127.0.0.1"
server_port = $PG_PORT
pool_mode = "transaction"

[[pools.postgres.users]]
username = "postgres"
password = ""
pool_size = $POOL_SIZE
EOF

# 3. Build & start pg_doorman -------------------------------------------------
echo "==> cargo build --release (pg_doorman + metrics_stress)"
(cd "$ROOT" && cargo build --release --bin pg_doorman --bin metrics_stress --quiet)
DOORMAN_BIN="$ROOT/target/release/pg_doorman"
STRESS_BIN="$ROOT/target/release/metrics_stress"

"$DOORMAN_BIN" -l warn "$DOORMAN_CONFIG" >"$WORKDIR/doorman.log" 2>&1 &
DOORMAN_PID=$!

# Wait until /metrics responds
echo -n "==> waiting for pg_doorman /metrics"
for _ in $(seq 1 50); do
    if curl -sf -m 1 "http://127.0.0.1:$METRICS_PORT/metrics" >/dev/null 2>&1; then
        echo " — ready"
        break
    fi
    echo -n "."
    if ! kill -0 "$DOORMAN_PID" 2>/dev/null; then
        echo
        echo "pg_doorman exited prematurely. log:"
        cat "$WORKDIR/doorman.log"
        exit 1
    fi
    sleep 0.2
done

# 4. pgbench harness ----------------------------------------------------------
cat > "$WORKDIR/bench.sql" <<'EOF'
SELECT 1;
EOF

run_pgbench() {
    local label="$1"
    pgbench -h 127.0.0.1 -p "$DOORMAN_PORT" -U postgres \
        -c "$PGBENCH_CLIENTS" -j "$PGBENCH_JOBS" -T "$PGBENCH_DURATION" \
        -n --protocol=simple \
        -l --log-prefix="$WORKDIR/pgbench-$label" \
        -f "$WORKDIR/bench.sql" \
        postgres >"$WORKDIR/pgbench-$label.out" 2>&1
}

# Compute p50/p95/p99 from pgbench --log files for a given label.
# pgbench --log writes one file per client thread named
# `<prefix>.<pid>.<thread>`. Latency is the 3rd column in microseconds.
compute_pcts() {
    local label="$1"
    # shellcheck disable=SC2086
    cat "$WORKDIR"/pgbench-"$label".[0-9]*.[0-9]* 2>/dev/null \
        | awk '{print $3}' \
        | sort -n \
        | awk 'BEGIN{n=0} {a[n++]=$1} END{
            if (n == 0) { print "no transactions logged"; exit }
            p50=a[int(n*0.50)]/1000.0
            p95=a[int(n*0.95)]/1000.0
            p99=a[int(n*0.99)]/1000.0
            pmax=a[n-1]/1000.0
            printf "  n=%d  p50=%.2fms  p95=%.2fms  p99=%.2fms  max=%.2fms\n", n, p50, p95, p99, pmax
          }'
}

# 5. Variant A — baseline -----------------------------------------------------
echo
echo "=== Variant A: baseline (no /metrics scrape) ==="
run_pgbench "baseline"
grep -E 'tps = |number of transactions actually processed' "$WORKDIR/pgbench-baseline.out" | head -5
compute_pcts "baseline"

# Small idle gap so the second run starts from a clean state
sleep 3

# 6. Variant B — under aggressive scrape --------------------------------------
echo
echo "=== Variant B: under aggressive /metrics scrape ==="

# Use the metrics_stress bin shipped in this repo: a tokio program that holds a
# shared reqwest::Client with keep-alive pool and hammers the URL from N tasks
# until a deadline. Same semantics as a Prometheus scraper running at maximum
# rate, in-process, without curl/ab fork() overhead per request.
"$STRESS_BIN" \
    --url "http://127.0.0.1:$METRICS_PORT/metrics" \
    --concurrency "$SCRAPE_CONCURRENCY" \
    --duration-secs "$((PGBENCH_DURATION + 5))" \
    >"$WORKDIR/stress.log" 2>&1 &
SCRAPE_PID=$!

run_pgbench "scraped"

# metrics_stress is time-bounded; it should already be exiting. Kill defensively.
kill "$SCRAPE_PID" 2>/dev/null || true
wait "$SCRAPE_PID" 2>/dev/null || true
SCRAPE_PID=""

echo
echo "--- /metrics scrape stats (metrics_stress) ---"
cat "$WORKDIR/stress.log"

grep -E 'tps = |number of transactions actually processed' "$WORKDIR/pgbench-scraped.out" | head -5
compute_pcts "scraped"

# 7. Summary ------------------------------------------------------------------
echo
echo "=== Summary ==="
printf "%-15s %s\n" "Variant" "Latency percentiles"
printf "%-15s " "baseline"; compute_pcts "baseline" | sed 's/^  //'
printf "%-15s " "scraped"; compute_pcts "scraped" | sed 's/^  //'
echo
echo "Baseline tps:"
grep -E 'tps = ' "$WORKDIR/pgbench-baseline.out" | head -3
echo
echo "Scraped tps:"
grep -E 'tps = ' "$WORKDIR/pgbench-scraped.out" | head -3
