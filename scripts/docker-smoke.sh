#!/usr/bin/env bash
# End-to-end smoke for a pg_doorman docker image:
#   1. pg_doorman --version inside image matches Cargo.toml
#   2. patroni_proxy --version inside image runs
#   3. pg_doorman generate produces a config against a real postgres
#   4. pg_doorman starts on the generated config
#   5. psql SELECT 1 routed through pg_doorman returns 1
#
# Both CI (build-packages.yaml docker-image-smoke job) and developers
# (make docker-smoke) call this script, so CI and local stay in sync.
#
# Usage:
#   scripts/docker-smoke.sh <image-ref>
#
# Requires: docker, internet access to pull postgres:17 once.
set -euo pipefail

IMAGE="${1:?usage: $0 <image-ref>}"

# Repo root so the Cargo.toml lookup works regardless of cwd.
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Unique resource names so parallel runs (or stale containers) don't collide.
RUN_ID="docker-smoke-$$-$RANDOM"
NET_NAME="net-${RUN_ID}"
PG_NAME="pg-${RUN_ID}"
DOORMAN_NAME="doorman-${RUN_ID}"
CONFIG_DIR="$(mktemp -d -t pg-doorman-smoke.XXXXXX)"

# Postgres credentials used both for the sidecar and for pg_doorman generate.
PG_USER="postgres"
PG_PASSWORD="postgres"
PG_DB="postgres"

cleanup() {
  local rc=$?
  set +e
  if [ "$rc" -ne 0 ]; then
    echo
    echo "=== smoke failed, dumping recent logs ==="
    echo "--- postgres ($PG_NAME) ---"
    docker logs --tail 60 "$PG_NAME" 2>&1 || true
    echo "--- pg_doorman ($DOORMAN_NAME) ---"
    docker logs --tail 100 "$DOORMAN_NAME" 2>&1 || true
    echo "--- generated config ---"
    cat "$CONFIG_DIR/pg_doorman.toml" 2>/dev/null || echo "(no config generated)"
  fi
  docker rm -f "$DOORMAN_NAME" "$PG_NAME" >/dev/null 2>&1 || true
  docker network rm "$NET_NAME" >/dev/null 2>&1 || true
  rm -rf "$CONFIG_DIR"
  return "$rc"
}
trap cleanup EXIT

echo "=== smoke: image = $IMAGE ==="
echo

# 1. Version inside the image matches Cargo.toml.
echo "--- pg_doorman --version vs Cargo.toml ---"
EXPECTED_VERSION=$(grep -m1 '^version = ' "$REPO_ROOT/Cargo.toml" | cut -d'"' -f2)
# `pg_doorman --version` emits "pg_doorman <version>".
ACTUAL_VERSION=$(docker run --rm "$IMAGE" pg_doorman --version | awk '{print $NF}')
echo "Cargo.toml: $EXPECTED_VERSION"
echo "image:      $ACTUAL_VERSION"
if [ "$ACTUAL_VERSION" != "$EXPECTED_VERSION" ]; then
  echo "::error::pg_doorman version in image ($ACTUAL_VERSION) does not match Cargo.toml ($EXPECTED_VERSION)"
  exit 1
fi

echo
echo "--- patroni_proxy --version ---"
docker run --rm "$IMAGE" patroni_proxy --version

# 2. Bring up an isolated network and a postgres:17 sidecar.
echo
echo "--- starting postgres:17 ---"
docker network create "$NET_NAME" >/dev/null
docker run -d --name "$PG_NAME" --network "$NET_NAME" \
  -e "POSTGRES_USER=$PG_USER" \
  -e "POSTGRES_PASSWORD=$PG_PASSWORD" \
  -e "POSTGRES_DB=$PG_DB" \
  postgres:17 >/dev/null

# Wait for postgres to accept connections. 60s ceiling: a pulled image
# from cache is typically ready in <5s; the ceiling covers a cold cache.
echo "waiting for postgres readiness..."
for i in $(seq 1 60); do
  if docker exec "$PG_NAME" pg_isready -U "$PG_USER" >/dev/null 2>&1; then
    echo "postgres ready after ${i}s"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "::error::postgres did not become ready in 60s"
    exit 1
  fi
  sleep 1
done

# 3. Generate a pg_doorman config against the real postgres.
echo
echo "--- pg_doorman generate ---"
docker run --rm --network "$NET_NAME" \
  -e "PGHOST=$PG_NAME" -e PGPORT=5432 \
  -e "PGUSER=$PG_USER" -e "PGPASSWORD=$PG_PASSWORD" \
  -e "PGDATABASE=$PG_DB" \
  -v "$CONFIG_DIR:/work" \
  "$IMAGE" \
  pg_doorman generate --output /work/pg_doorman.toml --server-host "$PG_NAME"

if [ ! -s "$CONFIG_DIR/pg_doorman.toml" ]; then
  echo "::error::pg_doorman generate produced no output"
  exit 1
fi
echo "generated $(wc -l < "$CONFIG_DIR/pg_doorman.toml") lines"

# 4. Start pg_doorman on the generated config.
echo
echo "--- starting pg_doorman ---"
docker run -d --name "$DOORMAN_NAME" --network "$NET_NAME" \
  -v "$CONFIG_DIR/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml" \
  "$IMAGE" \
  pg_doorman /etc/pg_doorman/pg_doorman.toml >/dev/null

# Same 60s ceiling as postgres above. pg_doorman binds the listener
# before opening backend connections, so on a working image it answers
# pg_isready within ~1s. The ceiling covers QEMU emulation on arm64
# and a slow first run on a cold runner.
echo "waiting for pg_doorman listener on 6432..."
for i in $(seq 1 60); do
  if docker run --rm --network "$NET_NAME" postgres:17 \
       pg_isready -h "$DOORMAN_NAME" -p 6432 -U "$PG_USER" >/dev/null 2>&1; then
    echo "pg_doorman ready after ${i}s"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "::error::pg_doorman did not start listening on 6432 in 60s"
    exit 1
  fi
  sleep 1
done

# 5. Route SELECT 1 through pg_doorman.
echo
echo "--- psql SELECT 1 via pg_doorman ---"
RESULT=$(docker run --rm --network "$NET_NAME" \
  -e "PGPASSWORD=$PG_PASSWORD" \
  postgres:17 \
  psql -h "$DOORMAN_NAME" -p 6432 -U "$PG_USER" -d "$PG_DB" \
       -At -c "SELECT 1")
echo "result: '$RESULT'"
if [ "$RESULT" != "1" ]; then
  echo "::error::SELECT 1 through pg_doorman returned '$RESULT', expected '1'"
  exit 1
fi

echo
echo "=== smoke passed: $IMAGE ==="
