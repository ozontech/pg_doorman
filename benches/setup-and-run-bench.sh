#!/usr/bin/env bash
# Run me as root on a fresh Ubuntu 22.04 host. Internet plus the repo source
# under $WORKSPACE (default /workspace) is enough.
#
# Layout: postgres, pg_doorman, odyssey and pgbouncer all live as runit
# services under /etc/service/. runsvdir supervises them; chpst handles the
# chuid and RLIMIT_NOFILE per service. The script's only jobs are install,
# build, write configs, hand control to runit, then drive pgbench.
#
# Output: /tmp/bench-results.tar.gz with raw pgbench stdout, per-transaction
# pgbench --log files, and the four service logs. Parsing happens elsewhere
# (see benches/parse-bench-logs.py).
#
# Tunables (env): BENCH_DOORMAN_WORKERS, BENCH_ODYSSEY_WORKERS, BENCH_DURATION,
#                 BENCH_PGBENCH_JOBS_C{1,40,120,500,10000}, WORKSPACE,
#                 CARGO_TARGET_DIR.

set -euo pipefail

[[ $EUID -eq 0 ]] || { echo "Run me as root (e.g. sudo bash $0)"; exit 1; }

WORKSPACE="${WORKSPACE:-/workspace}"
FEATURE_FILE="$WORKSPACE/tests/bdd/features/bench.feature"
RESULTS_DIR=/tmp/bench-results
RESULTS_TARBALL=/tmp/bench-results.tar.gz

PG_VERSION=14
PG_PORT=5433
DOORMAN_PORT=6432
ODYSSEY_PORT=6433
PGBOUNCER_PORT=6434

BENCH_DURATION="${BENCH_DURATION:-30}"
DOORMAN_WORKERS="${BENCH_DOORMAN_WORKERS:-12}"
ODYSSEY_WORKERS="${BENCH_ODYSSEY_WORKERS:-12}"
PGBENCH_JOBS_C1="${BENCH_PGBENCH_JOBS_C1:-1}"
PGBENCH_JOBS_C40="${BENCH_PGBENCH_JOBS_C40:-4}"
PGBENCH_JOBS_C120="${BENCH_PGBENCH_JOBS_C120:-4}"
PGBENCH_JOBS_C500="${BENCH_PGBENCH_JOBS_C500:-4}"
PGBENCH_JOBS_C10000="${BENCH_PGBENCH_JOBS_C10000:-4}"

PGBIN="/usr/lib/postgresql/$PG_VERSION/bin"
PGDATA=/tmp/pgdata
SSL_KEY=/tmp/bench-key.pem
SSL_CERT=/tmp/bench-cert.pem
PGBENCH_FILE=/tmp/pgbench.sql
SVDIR=/etc/service
SERVICES=(postgres pg_doorman odyssey pgbouncer)

log() { printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$*"; }

install_packages() {
  log "Installing system packages"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y --no-install-recommends \
    ca-certificates curl gnupg lsb-release
  install -d /usr/share/postgresql-common/pgdg
  curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc \
    -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc
  local codename
  codename=$(. /etc/os-release && echo "${VERSION_CODENAME:-jammy}")
  cat > /etc/apt/sources.list.d/pgdg.list <<EOF
deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt $codename-pgdg main
EOF
  apt-get update -qq
  apt-get install -y --no-install-recommends \
    "postgresql-$PG_VERSION" "postgresql-contrib-$PG_VERSION" \
    pgbouncer postgresql-client \
    build-essential pkg-config libssl-dev libpq-dev cmake git \
    netcat-openbsd openssl jq runit
  # The Ubuntu/PGDG packages auto-start postgres on 5432 and pgbouncer on
  # 6432 — exactly the ports our runit instances want to claim. Without
  # disabling them, our pg_doorman silently loses 6432 to the system
  # pgbouncer and pgbench then talks to the wrong process.
  systemctl stop postgresql pgbouncer 2>/dev/null || true
  systemctl disable postgresql pgbouncer 2>/dev/null || true
}

install_rust() {
  if command -v cargo >/dev/null; then
    log "Rust already present: $(cargo --version)"
    return
  fi
  log "Installing rustup 1.87.0"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain 1.87.0 --profile minimal
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
}

build_pg_doorman() {
  log "Building pg_doorman (cargo build --release)"
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env" 2>/dev/null || true
  cd "$WORKSPACE"
  local target_dir="${CARGO_TARGET_DIR:-$WORKSPACE/target}"
  CARGO_TARGET_DIR="$target_dir" cargo build --release
  install -m 0755 "$target_dir/release/pg_doorman" /usr/local/bin/
  /usr/local/bin/pg_doorman --version || true
}

build_odyssey() {
  if [[ -x /usr/local/bin/odyssey ]]; then
    log "odyssey already installed"
    return
  fi
  log "Building odyssey 1.4.1"
  rm -rf /tmp/odyssey-src /tmp/odyssey-build
  git clone --depth 1 -b v1.4.1 https://github.com/yandex/odyssey.git /tmp/odyssey-src
  cmake -S /tmp/odyssey-src -B /tmp/odyssey-build \
    -DBUILD_COMPRESSION=OFF -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_C_FLAGS="-Wno-error=implicit-int -Wno-error=incompatible-pointer-types"
  cmake --build /tmp/odyssey-build -j"$(nproc)"
  local bin
  bin=$(find /tmp/odyssey-build -name odyssey -type f -executable | head -1)
  [[ -z "$bin" ]] && { echo "odyssey binary not found" >&2; find /tmp/odyssey-build -name 'odyssey*'; exit 1; }
  install -m 0755 "$bin" /usr/local/bin/odyssey
}

init_postgres_datadir() {
  log "Initializing postgres data directory at $PGDATA"
  rm -rf "$PGDATA"
  install -d -o postgres -g postgres "$PGDATA"
  chpst -u postgres:postgres -- \
    "$PGBIN/initdb" -D "$PGDATA" --auth-host=trust --auth-local=trust >/dev/null
  cat > /tmp/pg_hba.conf <<EOF
local   all all trust
host    all all 127.0.0.1/32 trust
host    all all ::1/128       trust
EOF
  install -m 0600 -o postgres -g postgres /tmp/pg_hba.conf "$PGDATA/pg_hba.conf"
}

generate_ssl() {
  log "Generating self-signed SSL cert"
  openssl req -x509 -nodes -newkey rsa:2048 -days 1 \
    -subj "/CN=bench" -keyout "$SSL_KEY" -out "$SSL_CERT" >/dev/null 2>&1
  # All four services need to read the cert; ephemeral, world-readable is fine.
  chmod 644 "$SSL_KEY" "$SSL_CERT"
}

write_pgbench_script() {
  cat > "$PGBENCH_FILE" <<'EOF'
\set aid random(1, 100000)
select :aid;
EOF
}

write_configs() {
  cat > /tmp/doorman.hba <<EOF
host all all 0.0.0.0/0 trust
hostssl all all 0.0.0.0/0 trust
EOF
  cat > /tmp/doorman.toml <<EOF
[general]
host = "127.0.0.1"
port = $DOORMAN_PORT
worker_threads = $DOORMAN_WORKERS
pg_hba = {path = "/tmp/doorman.hba"}
admin_username = "admin"
admin_password = "admin"
tls_private_key = "$SSL_KEY"
tls_certificate = "$SSL_CERT"
max_connections = 11000

[pools.postgres]
server_host = "127.0.0.1"
server_port = $PG_PORT
pool_mode = "transaction"

[[pools.postgres.users]]
username = "postgres"
password = ""
pool_size = 40
EOF

  cat > /tmp/odyssey.conf <<EOF
workers $ODYSSEY_WORKERS
log_to_stdout no
log_file "/dev/null"
log_format "%p %t %l [%i %s] (%c) %m\n"
log_debug no
log_config no
log_session no
log_query no
log_stats no

storage "postgres_server" {
  type "remote"
  host "127.0.0.1"
  port $PG_PORT
}

database "postgres" {
  user "postgres" {
    authentication "none"
    storage "postgres_server"
    pool "transaction"
    pool_size 40
    pool_discard no
    pool_reserve_prepared_statement yes
  }
}

listen {
  host "127.0.0.1"
  port $ODYSSEY_PORT
  tls "allow"
  tls_cert_file "$SSL_CERT"
  tls_key_file "$SSL_KEY"
}
EOF

  cat > /tmp/pgbouncer.users <<'EOF'
"postgres" ""
EOF
  cat > /tmp/pgbouncer.ini <<EOF
[databases]
postgres = host=127.0.0.1 port=$PG_PORT dbname=postgres

[pgbouncer]
listen_addr = 127.0.0.1
listen_port = $PGBOUNCER_PORT
unix_socket_dir =
auth_type = trust
auth_file = /tmp/pgbouncer.users
pool_mode = transaction
max_client_conn = 11000
default_pool_size = 40
admin_users = postgres
client_tls_sslmode = allow
client_tls_key_file = $SSL_KEY
client_tls_cert_file = $SSL_CERT
client_tls_ca_file = $SSL_CERT
log_pooler_errors = 0
verbose = 0
log_connections = 0
log_disconnections = 0
logfile = /dev/null
EOF
  chown postgres /tmp/pgbouncer.ini /tmp/pgbouncer.users
}

write_runit_service() {
  local name=$1 logfile=$2
  local dir="$SVDIR/$name"
  shift 2
  install -d -m 0755 "$dir"
  # 'down' marker keeps the service from auto-starting until we 'sv up' it.
  touch "$dir/down"
  {
    echo '#!/bin/sh'
    printf 'exec >>%s 2>&1\n' "$logfile"
    printf 'exec '
    printf '%q ' "$@"
    echo
  } > "$dir/run"
  chmod 0755 "$dir/run"
}

create_runit_services() {
  log "Creating runit service definitions under $SVDIR"
  rm -rf "$SVDIR"
  install -d -m 0755 "$SVDIR"

  write_runit_service postgres /tmp/pg.log \
    chpst -u postgres:postgres -- \
    "$PGBIN/postgres" -D "$PGDATA" \
    -c max_connections=500 -p "$PG_PORT" \
    -c listen_addresses=127.0.0.1 \
    -c unix_socket_directories=/tmp

  write_runit_service pg_doorman /tmp/doorman.log \
    chpst -o 1048576 -- \
    /usr/local/bin/pg_doorman /tmp/doorman.toml

  write_runit_service odyssey /tmp/odyssey.log \
    chpst -o 1048576 -- \
    /usr/local/bin/odyssey /tmp/odyssey.conf

  write_runit_service pgbouncer /tmp/pgbouncer.log \
    chpst -u postgres:postgres -o 1048576 -- \
    /usr/sbin/pgbouncer /tmp/pgbouncer.ini
}

start_runsvdir() {
  log "Starting runsvdir"
  mkdir -p /var/log/runit
  nohup runsvdir "$SVDIR" >/var/log/runit/runsvdir.log 2>&1 &
  RUNSVDIR_PID=$!
  # runsvdir scans the dir every ~5s; give it a moment to spawn the runsv's.
  for i in $(seq 1 20); do
    local missing=0
    for s in "${SERVICES[@]}"; do
      [[ -d "$SVDIR/$s/supervise" ]] || missing=1
    done
    [[ $missing -eq 0 ]] && return
    sleep 1
  done
  log "runsvdir failed to set up supervise dirs in 20s"
  return 1
}

bring_up() {
  local name=$1 port=$2
  log "Bringing up $name"
  sv up "$SVDIR/$name"
  for i in $(seq 1 30); do
    if nc -z 127.0.0.1 "$port" 2>/dev/null; then
      log "  $name ready on $port"
      return
    fi
    sleep 1
  done
  log "  $name did not open port $port; tail of log:"
  case $name in
    postgres)   tail -n 50 /tmp/pg.log 2>&1 || true ;;
    pg_doorman) tail -n 50 /tmp/doorman.log 2>&1 || true ;;
    odyssey)    tail -n 50 /tmp/odyssey.log 2>&1 || true ;;
    pgbouncer)  tail -n 50 /tmp/pgbouncer.log 2>&1 || true ;;
  esac
  return 1
}

cleanup() {
  log "Cleanup: stopping services"
  for s in "${SERVICES[@]}"; do
    sv -w 5 force-stop "$SVDIR/$s" 2>/dev/null || true
  done
  if [[ -n "${RUNSVDIR_PID:-}" ]]; then
    kill "$RUNSVDIR_PID" 2>/dev/null || true
    wait "$RUNSVDIR_PID" 2>/dev/null || true
  fi
  # Always copy service logs and the wrapper output into the results dir so
  # even a failed run gives us something to debug.
  mkdir -p "$RESULTS_DIR"
  for f in /tmp/doorman.log /tmp/odyssey.log /tmp/pgbouncer.log /tmp/pg.log \
           /tmp/bench.out /tmp/bench-wrap.log; do
    [[ -f "$f" ]] && cp -f "$f" "$RESULTS_DIR/" 2>/dev/null || true
  done
  if [[ -d "$RESULTS_DIR" ]]; then
    tar czf "$RESULTS_TARBALL" -C /tmp bench-results 2>/dev/null || true
  fi
}

substitute_pgbench_placeholders() {
  # Replace the ${VAR} markers from bench.feature with concrete values using
  # bash parameter expansion. This used to be 'envsubst' but that depended on
  # exporting every var into the environment; one accidental shadowing left
  # literal '${PGBENCH_FILE}' in pgbench's argv and caused 'env: '\'''\'':
  # No such file or directory' for those rounds.
  local s=$1
  s="${s//\${DOORMAN_PORT\}/$DOORMAN_PORT}"
  s="${s//\${ODYSSEY_PORT\}/$ODYSSEY_PORT}"
  s="${s//\${PGBOUNCER_PORT\}/$PGBOUNCER_PORT}"
  s="${s//\${PGBENCH_FILE\}/$PGBENCH_FILE}"
  s="${s//\${PGBENCH_JOBS_C1\}/$PGBENCH_JOBS_C1}"
  s="${s//\${PGBENCH_JOBS_C40\}/$PGBENCH_JOBS_C40}"
  s="${s//\${PGBENCH_JOBS_C120\}/$PGBENCH_JOBS_C120}"
  s="${s//\${PGBENCH_JOBS_C500\}/$PGBENCH_JOBS_C500}"
  s="${s//\${PGBENCH_JOBS_C10000\}/$PGBENCH_JOBS_C10000}"
  s="${s// -T 30 / -T $BENCH_DURATION }"
  printf '%s' "$s"
}

run_all_pgbench() {
  mkdir -p "$RESULTS_DIR"

  local total=0
  local failed=0
  while IFS=$'\t' read -r name args env_str; do
    [[ -z "$name" ]] && continue
    total=$((total+1))
    local args_subst
    args_subst=$(substitute_pgbench_placeholders "$args")
    # Hard wall-clock cap: BENCH_DURATION plus a generous buffer for the
    # connect/warmup phase (10k-client scenarios spend tens of seconds opening
    # sockets before the timed window even starts).
    local pgbench_timeout=$((BENCH_DURATION + 180))
    local log_prefix="$RESULTS_DIR/${name}_pgbenchlog"
    log "[$total] $name :: $env_str pgbench -l --log-prefix=$log_prefix $args_subst (timeout=${pgbench_timeout}s)"
    if env "$env_str" timeout "$pgbench_timeout" \
        pgbench -l --log-prefix="$log_prefix" $args_subst \
        > "$RESULTS_DIR/$name.log" 2>&1; then
      :
    else
      local rc=$?
      failed=$((failed+1))
      echo "EXIT_CODE=$rc" >> "$RESULTS_DIR/$name.log"
      log "  ! exited rc=$rc"
    fi
  done < <(sed -nE 's/.*pgbench for "([^"]+)" with "([^"]+)" and env "([^"]+)".*/\1\t\2\t\3/p' "$FEATURE_FILE")

  log "pgbench rounds: $total total, $failed failed"
}

write_metadata() {
  cat > "$RESULTS_DIR/metadata.json" <<EOF
{
  "host": "$(hostname)",
  "kernel": "$(uname -srm)",
  "vcpus": $(nproc),
  "memory_kb": $(awk '/MemTotal/ {print $2}' /proc/meminfo),
  "started_at": "$(date -u +%FT%TZ)",
  "doorman_workers": $DOORMAN_WORKERS,
  "odyssey_workers": $ODYSSEY_WORKERS,
  "duration_per_run_sec": $BENCH_DURATION,
  "pgbench_jobs": {
    "c1": $PGBENCH_JOBS_C1,
    "c40": $PGBENCH_JOBS_C40,
    "c120": $PGBENCH_JOBS_C120,
    "c500": $PGBENCH_JOBS_C500,
    "c10000": $PGBENCH_JOBS_C10000
  },
  "git_sha": "$(cd "$WORKSPACE" && git rev-parse HEAD 2>/dev/null || echo unknown)"
}
EOF
}

main() {
  trap cleanup EXIT

  # The pgbench/parent shell only needs enough FDs to open log files and
  # spawn timeout/pgbench; per-service limits are set by chpst -o in the run
  # scripts.
  ulimit -n 1048576 2>/dev/null || ulimit -n 65536 2>/dev/null || true

  install_packages
  install_rust
  build_pg_doorman
  build_odyssey

  init_postgres_datadir
  generate_ssl
  write_pgbench_script
  write_configs
  create_runit_services
  start_runsvdir

  bring_up postgres   "$PG_PORT"
  bring_up pg_doorman "$DOORMAN_PORT"
  bring_up odyssey    "$ODYSSEY_PORT"
  bring_up pgbouncer  "$PGBOUNCER_PORT"

  run_all_pgbench
  write_metadata

  log "Packing $RESULTS_TARBALL"
  tar czf "$RESULTS_TARBALL" -C /tmp bench-results
  ls -lh "$RESULTS_TARBALL"
  log "DONE"
}

main "$@"
