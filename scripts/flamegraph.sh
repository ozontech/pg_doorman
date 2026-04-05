#!/usr/bin/env bash
#
# Automated CPU flamegraph for pg_doorman under pgbench load.
#
# Usage:
#   ./scripts/flamegraph.sh
#   FLAMEGRAPH_DURATION=30 FLAMEGRAPH_CLIENTS=500 ./scripts/flamegraph.sh
#
# Artifacts are written to flamegraph-output/<timestamp>/

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration (all overridable via environment)
# ---------------------------------------------------------------------------
DURATION="${FLAMEGRAPH_DURATION:-60}"
CLIENTS="${FLAMEGRAPH_CLIENTS:-120}"
JOBS="${FLAMEGRAPH_JOBS:-4}"
PROTOCOL="${FLAMEGRAPH_PROTOCOL:-extended}"
POOL_SIZE="${FLAMEGRAPH_POOL_SIZE:-40}"
WORKERS="${FLAMEGRAPH_WORKERS:-$(nproc)}"
PERF_FREQ="${FLAMEGRAPH_FREQ:-99}"
PGBENCH_SCRIPT="${FLAMEGRAPH_SCRIPT:-scripts/noop.sql}"

# ---------------------------------------------------------------------------
# Resolve project root (script may be called from any directory)
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$PROJECT_ROOT/target/profiling/pg_doorman"
PGBENCH_SCRIPT="$PROJECT_ROOT/$PGBENCH_SCRIPT"

# ---------------------------------------------------------------------------
# State variables for cleanup
# ---------------------------------------------------------------------------
TMPDIR=""
DOORMAN_PID=""
PERF_PID=""
PG_PORT=""
DOORMAN_PORT=""
OUT_DIR=""

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
log()  { echo "==> $*"; }
die()  { echo "FATAL: $*" >&2; exit 1; }

cleanup() {
    local exit_code=$?
    set +e

    if [ -n "$DOORMAN_PID" ]; then
        kill "$DOORMAN_PID" 2>/dev/null
        wait "$DOORMAN_PID" 2>/dev/null
        DOORMAN_PID=""
    fi

    if [ -n "$PERF_PID" ]; then
        kill "$PERF_PID" 2>/dev/null
        wait "$PERF_PID" 2>/dev/null
        PERF_PID=""
    fi

    if [ -n "$TMPDIR" ] && [ -d "$TMPDIR/db" ]; then
        pg_ctl -D "$TMPDIR/db" stop -m immediate 2>/dev/null || true
    fi

    if [ -n "$TMPDIR" ] && [ -d "$TMPDIR" ]; then
        rm -rf "$TMPDIR"
    fi

    if [ $exit_code -ne 0 ] && [ -n "$OUT_DIR" ]; then
        echo ""
        echo "Script failed (exit $exit_code). Partial artifacts may be in: $OUT_DIR"
    fi
}

trap cleanup EXIT INT TERM

# Find a free TCP port by binding to port 0
find_free_port() {
    python3 -c '
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
'
}

# Wait for a TCP port to become reachable
wait_for_port() {
    local host="$1" port="$2" label="$3" max_attempts="${4:-40}" delay="${5:-0.25}"
    local attempt=0

    while [ "$attempt" -lt "$max_attempts" ]; do
        if bash -c "echo >/dev/tcp/$host/$port" 2>/dev/null; then
            return 0
        fi
        attempt=$((attempt + 1))
        sleep "$delay"
    done

    die "$label failed to become ready on $host:$port (timeout $((max_attempts * ${delay%.*}))s)"
}

# ---------------------------------------------------------------------------
# Phase 0: Preflight checks
# ---------------------------------------------------------------------------
phase_preflight() {
    log "Phase 0: preflight checks"

    # perf
    if ! command -v perf >/dev/null 2>&1; then
        die "perf not found. Install: sudo dnf install perf"
    fi

    # pgbench
    if ! command -v pgbench >/dev/null 2>&1; then
        log "pgbench not found, installing postgresql-contrib..."
        sudo dnf install -y postgresql-contrib \
            || die "Failed to install postgresql-contrib (provides pgbench)"
    fi

    # PostgreSQL tools
    for cmd in initdb pg_ctl pg_isready; do
        command -v "$cmd" >/dev/null 2>&1 \
            || die "$cmd not found. Install: sudo dnf install postgresql-server"
    done

    # inferno
    if ! command -v inferno-collapse-perf >/dev/null 2>&1 \
       || ! command -v inferno-flamegraph >/dev/null 2>&1; then
        log "inferno not found, installing via cargo..."
        cargo install inferno \
            || die "Failed to install inferno (cargo install inferno)"
    fi

    # perf_event_paranoid
    local paranoid
    paranoid="$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 99)"
    if [ "$paranoid" -gt 1 ]; then
        log "kernel.perf_event_paranoid=$paranoid (need <=1), lowering..."
        sudo sysctl -w kernel.perf_event_paranoid=1 \
            || die "Cannot set perf_event_paranoid. Run: sudo sysctl -w kernel.perf_event_paranoid=1"
    fi

    # pgbench script
    [ -f "$PGBENCH_SCRIPT" ] \
        || die "pgbench script not found: $PGBENCH_SCRIPT"

    log "  perf:    $(perf version 2>&1 | head -1)"
    log "  pgbench: $(pgbench --version 2>&1 | head -1)"
    log "  inferno: $(inferno-collapse-perf --help 2>&1 | head -1)"
}

# ---------------------------------------------------------------------------
# Phase 1: Build
# ---------------------------------------------------------------------------
phase_build() {
    log "Phase 1: building pg_doorman (profile=profiling)"
    (cd "$PROJECT_ROOT" && cargo build --profile profiling --bin pg_doorman)
    [ -x "$BINARY" ] || die "Binary not found: $BINARY"
    log "  binary: $BINARY"
}

# ---------------------------------------------------------------------------
# Phase 2: Start PostgreSQL
# ---------------------------------------------------------------------------
phase_start_postgres() {
    log "Phase 2: starting PostgreSQL"

    TMPDIR="$(mktemp -d)"
    local db_path="$TMPDIR/db"

    initdb --no-sync -D "$db_path" -U postgres >/dev/null 2>&1 \
        || die "initdb failed"

    # Permissive HBA for local benchmarking
    cat > "$db_path/pg_hba.conf" <<'HBA'
local   all   all                 trust
host    all   all   127.0.0.1/32  trust
HBA

    PG_PORT="$(find_free_port)"
    pg_ctl -D "$db_path" \
        -l "$TMPDIR/pg.log" \
        -o "-p $PG_PORT -F -k $TMPDIR" \
        start >/dev/null 2>&1 \
        || die "pg_ctl start failed (see $TMPDIR/pg.log)"

    # Wait for readiness (20 attempts x 500ms = 10s, matching BDD pattern)
    local attempt=0
    while [ "$attempt" -lt 20 ]; do
        if pg_isready -p "$PG_PORT" -h 127.0.0.1 -t 1 >/dev/null 2>&1; then
            log "  PostgreSQL ready on port $PG_PORT"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 0.5
    done

    die "PostgreSQL failed to start on port $PG_PORT (see $TMPDIR/pg.log)"
}

# ---------------------------------------------------------------------------
# Phase 3: Start pg_doorman
# ---------------------------------------------------------------------------
phase_start_doorman() {
    log "Phase 3: starting pg_doorman"

    DOORMAN_PORT="$(find_free_port)"

    # Create output directory early so pg_doorman.log lands there
    OUT_DIR="$PROJECT_ROOT/flamegraph-output/$(date +%Y-%m-%d_%H%M%S)"
    mkdir -p "$OUT_DIR"

    # Create HBA file with trust auth (mirrors bench.feature pattern)
    cat > "$TMPDIR/doorman_hba.conf" <<'HBA'
host all all 0.0.0.0/0 trust
HBA

    # Generate config (mirrors bench.feature pattern)
    cat > "$TMPDIR/config.toml" <<TOML
[general]
host = "127.0.0.1"
port = ${DOORMAN_PORT}
worker_threads = ${WORKERS}
admin_username = "admin"
admin_password = "admin"
pg_hba = {path = "${TMPDIR}/doorman_hba.conf"}

[pools.postgres]
server_host = "127.0.0.1"
server_port = ${PG_PORT}
pool_mode = "transaction"

[[pools.postgres.users]]
username = "postgres"
password = ""
pool_size = ${POOL_SIZE}
TOML

    "$BINARY" "$TMPDIR/config.toml" -l info 2>"$OUT_DIR/pg_doorman.log" &
    DOORMAN_PID=$!

    # Wait for TCP readiness (20 attempts x 250ms = 5s, matching doorman_helper.rs)
    wait_for_port 127.0.0.1 "$DOORMAN_PORT" "pg_doorman" 20 0.25

    log "  pg_doorman ready on port $DOORMAN_PORT (PID $DOORMAN_PID)"
}

# ---------------------------------------------------------------------------
# Phase 4: Warmup
# ---------------------------------------------------------------------------
phase_warmup() {
    log "Phase 4: warming up (5s pgbench)"

    PGSSLMODE=disable pgbench -n \
        -h 127.0.0.1 -p "$DOORMAN_PORT" -U postgres \
        -c "$CLIENTS" -j "$JOBS" -T 5 \
        --protocol="$PROTOCOL" \
        postgres -f "$PGBENCH_SCRIPT" \
        || die "Warmup pgbench failed"
}

# ---------------------------------------------------------------------------
# Phase 5: perf record + pgbench
# ---------------------------------------------------------------------------
phase_record() {
    log "Phase 5: recording perf (${DURATION}s, ${CLIENTS} clients, protocol=$PROTOCOL)"
    log "  output: $OUT_DIR"

    # Start perf in background
    perf record \
        -F "$PERF_FREQ" \
        -p "$DOORMAN_PID" \
        -g --call-graph dwarf \
        -o "$OUT_DIR/perf.data" \
        -- sleep "$DURATION" &
    PERF_PID=$!

    # Run pgbench as load driver (PGSSLMODE=disable mirrors bench.feature)
    PGSSLMODE=disable pgbench -n \
        -h 127.0.0.1 -p "$DOORMAN_PORT" -U postgres \
        -c "$CLIENTS" -j "$JOBS" -T "$DURATION" -P 5 \
        --protocol="$PROTOCOL" \
        postgres -f "$PGBENCH_SCRIPT" 2>&1 \
        | tee "$OUT_DIR/pgbench.log"

    # Wait for perf to finish
    wait "$PERF_PID" 2>/dev/null || true
    PERF_PID=""

    [ -f "$OUT_DIR/perf.data" ] || die "perf.data not created"
}

# ---------------------------------------------------------------------------
# Phase 6: Generate flamegraphs
# ---------------------------------------------------------------------------
phase_flamegraph() {
    log "Phase 6: generating flamegraphs"

    local title="pg_doorman CPU (${PROTOCOL}, ${CLIENTS}c, ${DURATION}s)"

    perf script -i "$OUT_DIR/perf.data" \
        | inferno-collapse-perf \
        > "$OUT_DIR/perf.folded"

    inferno-flamegraph \
        --title "$title" \
        < "$OUT_DIR/perf.folded" \
        > "$OUT_DIR/flamegraph.svg"

    inferno-flamegraph \
        --reverse \
        --title "$title (reverse)" \
        < "$OUT_DIR/perf.folded" \
        > "$OUT_DIR/flamegraph-reverse.svg"

    # Generate machine-readable CSV from folded stacks
    python3 -c "
import sys
from collections import defaultdict

counts = defaultdict(int)
total = 0

for line in open('$OUT_DIR/perf.folded'):
    line = line.strip()
    if not line:
        continue
    parts = line.rsplit(' ', 1)
    if len(parts) != 2:
        continue
    stack_str, val_str = parts
    val = int(val_str)
    total += val
    frames = stack_str.split(';')
    leaf = frames[-1]
    counts[leaf] += val

with open('$OUT_DIR/profile.csv', 'w') as f:
    f.write('function,self_samples,self_pct,category\n')
    for func, cnt in sorted(counts.items(), key=lambda x: -x[1]):
        pct = round(cnt / total * 100, 4)
        if 'pg_doorman' in func:
            cat = 'pg_doorman'
        elif 'tokio' in func:
            cat = 'tokio'
        elif any(x in func for x in ['alloc', 'dealloc', 'malloc', 'free', 'jemalloc']):
            cat = 'allocator'
        elif any(x in func for x in ['mio::', 'epoll']):
            cat = 'mio'
        elif any(x in func for x in ['nft_', 'nf_', 'conntrack']):
            cat = 'kernel_nftables'
        elif any(x in func for x in ['tcp_', 'ip_', 'skb_', '__dev_queue', 'net_rx', '__tcp_']):
            cat = 'kernel_tcp'
        elif any(x in func for x in ['sched', 'enqueue_task', 'dequeue_task', 'update_curr', 'update_load', 'psi_group']):
            cat = 'kernel_scheduler'
        else:
            cat = 'other'
        # Escape quotes in function names
        safe = func.replace('\"', '\"\"')
        f.write(f'\"{ safe }\",{cnt},{pct},{cat}\n')
" || log "  (CSV generation skipped — python3 not available)"

    # Save config for reproducibility
    cp "$TMPDIR/config.toml" "$OUT_DIR/config.toml"

    log "  flamegraph.svg:         $OUT_DIR/flamegraph.svg"
    log "  flamegraph-reverse.svg: $OUT_DIR/flamegraph-reverse.svg"
    log "  profile.csv:            $OUT_DIR/profile.csv"
    log "  perf.folded:            $OUT_DIR/perf.folded"
    log "  pgbench.log:            $OUT_DIR/pgbench.log"
    log "  config.toml:            $OUT_DIR/config.toml"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    echo ""
    echo "pg_doorman flamegraph profiler"
    echo "  duration=$DURATION  clients=$CLIENTS  protocol=$PROTOCOL"
    echo "  pool_size=$POOL_SIZE  workers=$WORKERS  freq=$PERF_FREQ"
    echo ""

    phase_preflight
    phase_build
    phase_start_postgres
    phase_start_doorman
    phase_warmup
    phase_record
    phase_flamegraph

    echo ""
    log "Done. Open flamegraph in browser:"
    echo "  xdg-open $OUT_DIR/flamegraph.svg"
    echo ""
}

main "$@"
