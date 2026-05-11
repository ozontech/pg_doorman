#!/usr/bin/env bash
# End-to-end smoke for a pg_doorman docker image:
#   1. pg_doorman --version inside image matches Cargo.toml
#      (skipped when CHECK_VERSION=0)
#   2. patroni_proxy --version inside image runs
#   3. pg_doorman generate produces a config against a real postgres
#      (with a non-superuser role + custom database created beforehand)
#   4. pg_doorman starts on the generated config
#   5. psql SELECT 1 routed through pg_doorman returns 1
#   6. psql via a non-superuser role into a non-default database returns
#      the expected current_user/current_database — exercises routing
#      and non-superuser auth, not just the postgres/postgres happy path
#
# Both CI (build-packages.yaml docker-image-smoke job + the dashboard
# validation workflow) and developers (`make docker-smoke`) call this
# script, so CI and local stay in sync.
#
# Usage:
#   scripts/docker-smoke.sh <image-ref>
#
# Environment overrides:
#   POSTGRES_IMAGE    sidecar/psql client image (default postgres:17)
#   CHECK_VERSION     1 to enforce Cargo.toml version match, 0 to skip
#                     (useful when smoking an already-published image
#                     against an unrelated checkout)
#
# Requires: docker, internet access to pull $POSTGRES_IMAGE once.
set -euo pipefail

IMAGE="${1:?usage: $0 <image-ref>}"

POSTGRES_IMAGE="${POSTGRES_IMAGE:-postgres:17}"
CHECK_VERSION="${CHECK_VERSION:-1}"

# Repo root so the Cargo.toml lookup works regardless of cwd.
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Unique resource names so parallel runs (or stale containers) don't collide.
RUN_ID="docker-smoke-$$-$RANDOM"
NET_NAME="net-${RUN_ID}"
PG_NAME="pg-${RUN_ID}"
DOORMAN_NAME="doorman-${RUN_ID}"
CONFIG_DIR="$(mktemp -d -t pg-doorman-smoke.XXXXXX)"

# Superuser credentials for the sidecar and for pg_doorman generate.
PG_USER="postgres"
PG_PASSWORD="postgres"
PG_DB="postgres"

# Non-superuser fixture created before generate so the generated config
# contains a pool for it, and the routing/auth round-trip below has a
# realistic non-postgres user to exercise.
SMOKE_USER="smoke_user"
SMOKE_PASSWORD="smoke_pw"
SMOKE_DB="smoke_db"

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
# EXIT covers normal exit and `set -e` failures. INT/TERM cover ^C
# and `docker stop` of the parent container or a CI runner shutdown.
# SIGKILL can't be trapped by design; the residual cleanup that
# leaves behind is taken care of by GHA's ephemeral runner.
trap cleanup EXIT INT TERM

echo "=== smoke: image = $IMAGE ==="
echo "=== sidecar image = $POSTGRES_IMAGE ==="
echo

# 1. Version inside the image matches Cargo.toml (unless explicitly skipped).
if [ "$CHECK_VERSION" = "1" ]; then
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
else
  echo "--- pg_doorman --version (Cargo.toml comparison skipped via CHECK_VERSION=0) ---"
  docker run --rm "$IMAGE" pg_doorman --version
fi

echo
echo "--- patroni_proxy --version ---"
docker run --rm "$IMAGE" patroni_proxy --version

# 2. Bring up an isolated network and a postgres sidecar.
echo
echo "--- starting $POSTGRES_IMAGE ---"
docker network create "$NET_NAME" >/dev/null
docker run -d --name "$PG_NAME" --network "$NET_NAME" \
  -e "POSTGRES_USER=$PG_USER" \
  -e "POSTGRES_PASSWORD=$PG_PASSWORD" \
  -e "POSTGRES_DB=$PG_DB" \
  "$POSTGRES_IMAGE" >/dev/null

# Wait for postgres to accept connections. 90s ceiling: typical fresh
# postgres:17 boot is <5s; the headroom covers cold-cache image pulls
# and slow runners (Ubicloud cold VM, congested GHA pool).
echo "waiting for postgres readiness..."
for i in $(seq 1 90); do
  if docker exec "$PG_NAME" pg_isready -U "$PG_USER" >/dev/null 2>&1; then
    echo "postgres ready after ${i}s"
    break
  fi
  if [ "$i" -eq 90 ]; then
    echo "::error::postgres did not become ready in 90s"
    exit 1
  fi
  sleep 1
done

# 3. Create non-superuser fixture before generate so pg_doorman generate
#    enumerates both users in pg_shadow and both databases when emitting
#    pools/auth_query stanzas.
echo
echo "--- creating non-superuser fixture: ${SMOKE_USER}/${SMOKE_DB} ---"
# Heredoc is single-quoted (<<'SQL'), so no shell expansion happens
# inside the SQL block. psql does the substitution at parse time
# via `-v name=value`, with `:"ident"` for safely quoted identifiers
# and `:'literal'` for safely quoted string literals. That way the
# SMOKE_USER / SMOKE_PASSWORD / SMOKE_DB names cannot be turned into
# SQL injection by an unexpected shell-meaningful character, today
# or after future edits.
docker exec -i -e "PGPASSWORD=$PG_PASSWORD" "$PG_NAME" \
  psql -U "$PG_USER" -d "$PG_DB" -v ON_ERROR_STOP=1 \
       -v "smoke_user=${SMOKE_USER}" \
       -v "smoke_password=${SMOKE_PASSWORD}" \
       -v "smoke_db=${SMOKE_DB}" <<'SQL'
CREATE ROLE :"smoke_user" LOGIN PASSWORD :'smoke_password';
CREATE DATABASE :"smoke_db" OWNER :"smoke_user";
SQL

# 4. Generate a pg_doorman config against the real postgres.
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

# 5. Start pg_doorman on the generated config.
echo
echo "--- starting pg_doorman ---"
docker run -d --name "$DOORMAN_NAME" --network "$NET_NAME" \
  -v "$CONFIG_DIR/pg_doorman.toml:/etc/pg_doorman/pg_doorman.toml" \
  "$IMAGE" \
  pg_doorman /etc/pg_doorman/pg_doorman.toml >/dev/null

# Same 90s ceiling as postgres above. pg_doorman binds the listener
# before opening backend connections, so on a working image it answers
# pg_isready within ~1s. The ceiling covers QEMU emulation on arm64
# and a slow first run on a cold runner.
echo "waiting for pg_doorman listener on 6432..."
for i in $(seq 1 90); do
  if docker run --rm --network "$NET_NAME" "$POSTGRES_IMAGE" \
       pg_isready -h "$DOORMAN_NAME" -p 6432 -U "$PG_USER" >/dev/null 2>&1; then
    echo "pg_doorman ready after ${i}s"
    break
  fi
  if [ "$i" -eq 90 ]; then
    echo "::error::pg_doorman did not start listening on 6432 in 90s"
    exit 1
  fi
  sleep 1
done

# 6. Route SELECT 1 through pg_doorman as the superuser/default database.
echo
echo "--- psql SELECT 1 via pg_doorman (superuser path) ---"
RESULT=$(docker run --rm --network "$NET_NAME" \
  -e "PGPASSWORD=$PG_PASSWORD" \
  "$POSTGRES_IMAGE" \
  psql -h "$DOORMAN_NAME" -p 6432 -U "$PG_USER" -d "$PG_DB" \
       -At -c "SELECT 1")
echo "result: '$RESULT'"
if [ "$RESULT" != "1" ]; then
  echo "::error::SELECT 1 through pg_doorman returned '$RESULT', expected '1'"
  exit 1
fi

# 7. Round-trip via the non-superuser role into the dedicated database.
#    Asserts current_user and current_database() so a regression in
#    pool routing or password forwarding gets caught, not just the
#    superuser SELECT 1 happy path.
echo
echo "--- psql current_user/current_database via pg_doorman (non-superuser path) ---"
RESULT=$(docker run --rm --network "$NET_NAME" \
  -e "PGPASSWORD=$SMOKE_PASSWORD" \
  "$POSTGRES_IMAGE" \
  psql -h "$DOORMAN_NAME" -p 6432 -U "$SMOKE_USER" -d "$SMOKE_DB" \
       -At -c "SELECT current_user || '|' || current_database()")
EXPECTED="${SMOKE_USER}|${SMOKE_DB}"
echo "result:   '$RESULT'"
echo "expected: '$EXPECTED'"
if [ "$RESULT" != "$EXPECTED" ]; then
  echo "::error::non-superuser routing returned '$RESULT', expected '$EXPECTED'"
  exit 1
fi

# 8. Prepared-statement round-trip in transaction pool mode (the
#    default that `pg_doorman generate` emits). All three statements
#    arrive in one psql `-c` invocation so they land in a single
#    transaction on a single backend; in transaction pooling each
#    psql command-line statement would otherwise be free to land on
#    a different backend, and EXECUTE would not find the PREPAREd
#    smoke_p. A regression that breaks pg_doorman routing of
#    PREPARE/EXECUTE drops the `42` line below.
#
#    `-At` keeps the output unaligned and tuples-only; psql still
#    prints command tags (PREPARE / DEALLOCATE) on their own lines
#    in this mode, so we grep for the single numeric line.
echo
echo "--- psql PREPARE / EXECUTE / DEALLOCATE via pg_doorman ---"
RESULT=$(docker run --rm --network "$NET_NAME" \
  -e "PGPASSWORD=$SMOKE_PASSWORD" \
  "$POSTGRES_IMAGE" \
  psql -h "$DOORMAN_NAME" -p 6432 -U "$SMOKE_USER" -d "$SMOKE_DB" \
       -At -c 'PREPARE smoke_p (int) AS SELECT $1 + 1; EXECUTE smoke_p(41); DEALLOCATE smoke_p;' \
  | grep -E '^[0-9]+$' | head -1)
echo "result: '$RESULT'"
if [ "$RESULT" != "42" ]; then
  echo "::error::PREPARE/EXECUTE through pg_doorman returned '$RESULT', expected '42'"
  exit 1
fi

echo
echo "=== smoke passed: $IMAGE ==="
