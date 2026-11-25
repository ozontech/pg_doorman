#!/usr/bin/env bash

# Sourced by run-tests.sh. The official image contains compiled Greengage and
# its demo-cluster scripts; no database compilation is needed at startup.
GREENGAGE_LIBRARY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GREENGAGE_IMAGE="${GREENGAGE_IMAGE:-greengagedb/ggdb7_ubuntu:7.5.0@sha256:5176ca76baf6f33092665454b3669310157c42af11ccf4f539f3252aee8ea7c3}"

greengage_bootstrap() {
    set -eo pipefail
    cd /home/gpadmin
    # Use the installation and account setup shipped in the pinned image.
    # shellcheck source=/dev/null
    source gpdb_src/concourse/scripts/common.bash
    install_gpdb
    gpdb_src/concourse/scripts/setup_gpadmin_user.bash
    su gpadmin -c 'source /usr/local/greengage-db-devel/greengage_path.sh; LANG=en_US.UTF-8 make -C /home/gpadmin/gpdb_src/gpAux/gpdemo create-demo-cluster PORT_BASE=5432 NUM_PRIMARY_MIRROR_PAIRS=2 WITH_MIRRORS=false WITH_STANDBY=false'

    # These credentials belong only to this disposable test cluster. Explicit
    # MD5 authentication exercises the backend auth path supported by doorman.
    su gpadmin -c 'source /usr/local/greengage-db-devel/greengage_path.sh; psql -X -v ON_ERROR_STOP=1 -p 5432 -d postgres' <<'SQL'
SET password_encryption = 'md5';
ALTER ROLE gpadmin PASSWORD 'greengage-test';
SQL
    local coordinator=/home/gpadmin/gpdb_src/gpAux/gpdemo/datadirs/qddir/demoDataDir-1
    {
        printf '%s\n' 'host all all 127.0.0.1/32 md5' 'host all all ::1/128 md5'
        cat "${coordinator}/pg_hba.conf"
    } > "${coordinator}/pg_hba.conf.test"
    mv "${coordinator}/pg_hba.conf.test" "${coordinator}/pg_hba.conf"
    chown gpadmin:gpadmin "${coordinator}/pg_hba.conf"
    su gpadmin -c 'source /usr/local/greengage-db-devel/greengage_path.sh; psql -X -v ON_ERROR_STOP=1 -p 5432 -d postgres -c "SELECT pg_reload_conf()"'
    touch /tmp/greengage-bootstrap-ready
    exec sleep infinity
}

stop_greengage() {
    if [ -z "${GREENGAGE_CONTAINER:-}" ] || [ -z "${GREENGAGE_OWNER_TOKEN:-}" ]; then
        return 0
    fi
    local owner
    owner=$(docker inspect --format '{{ index .Config.Labels "org.pg-doorman.greengage-test-owner" }}' "${GREENGAGE_CONTAINER}" 2>/dev/null) || return 0
    if [ "${owner}" = "${GREENGAGE_OWNER_TOKEN}" ]; then
        docker rm --force --volumes "${GREENGAGE_CONTAINER}" >/dev/null
    fi
    unset GREENGAGE_CONTAINER GREENGAGE_OWNER_TOKEN
}

greengage_start_failure() {
    printf '[ERROR] Greengage startup failed: %s\n' "$1" >&2
    docker logs --tail 200 "${GREENGAGE_CONTAINER}" >&2 || true
    stop_greengage
    return 1
}

start_greengage() {
    if [ -n "${GREENGAGE_CONTAINER:-}" ]; then
        printf '[ERROR] Greengage is already allocated by this runner\n' >&2
        return 1
    fi
    local startup_timeout="${GREENGAGE_STARTUP_TIMEOUT:-600}"
    case "${startup_timeout}" in
        ''|*[!0-9]*|0)
            printf '[ERROR] GREENGAGE_STARTUP_TIMEOUT must be a positive number of seconds\n' >&2
            return 1
            ;;
    esac
    export GREENGAGE_HOST=127.0.0.1 GREENGAGE_PORT=5432
    export GREENGAGE_USER=gpadmin GREENGAGE_PASSWORD=greengage-test
    # Set ownership before creating the container so EXIT cleanup also covers
    # interrupted startup. The label prevents deleting an unrelated container.
    GREENGAGE_OWNER_TOKEN="$$-${RANDOM}-${RANDOM}"
    GREENGAGE_CONTAINER="pg-doorman-greengage-${GREENGAGE_OWNER_TOKEN}"
    export GREENGAGE_CONTAINER
    printf '[INFO] Starting Greengage: %s\n' "${GREENGAGE_IMAGE}"
    if ! docker create \
        --name "${GREENGAGE_CONTAINER}" \
        --label "org.pg-doorman.greengage-test-owner=${GREENGAGE_OWNER_TOKEN}" \
        --platform linux/amd64 \
        --init \
        --shm-size 512m \
        --ulimit nofile=65536:65536 \
        --sysctl 'kernel.sem=500 1024000 200 4096' \
        -v "${GREENGAGE_LIBRARY_DIR}/greengage.sh:/tmp/pg-doorman-greengage.sh:ro" \
        "${GREENGAGE_IMAGE}" bash /tmp/pg-doorman-greengage.sh --bootstrap >/dev/null; then
        greengage_start_failure 'container creation failed'
        return 1
    fi
    if ! docker start "${GREENGAGE_CONTAINER}" >/dev/null; then
        greengage_start_failure 'container start failed'
        return 1
    fi

    local deadline=$((SECONDS + startup_timeout))
    local ready running
    while [ "${SECONDS}" -lt "${deadline}" ]; do
        running=$(docker inspect --format '{{.State.Running}}' "${GREENGAGE_CONTAINER}") || running=false
        if [ "${running}" != true ]; then
            greengage_start_failure 'bootstrap exited before the cluster became ready'
            return 1
        fi
        # Check authentication, catalog health, and a query actually dispatched
        # to both primary segments, rather than accepting a listening socket.
        ready=$(docker exec \
            -e PGPASSWORD="${GREENGAGE_PASSWORD}" \
            -e PGCONNECT_TIMEOUT=2 \
            -e 'PGOPTIONS=-c statement_timeout=5000' \
            "${GREENGAGE_CONTAINER}" timeout 8 bash -c '
                test -f /tmp/greengage-bootstrap-ready || exit 1
                source /usr/local/greengage-db-devel/greengage_path.sh
                psql -X -A -t -v ON_ERROR_STOP=1 -h 127.0.0.1 -p 5432 -U gpadmin -d postgres -c "SELECT version() LIKE '\''%Greengage Database 7.%'\'' AND (SELECT count(*) >= 2 AND bool_and(status = '\''u'\'') FROM gp_segment_configuration WHERE content >= 0 AND role = '\''p'\'') AND (SELECT count(*) >= 2 FROM gp_dist_random('\''gp_id'\''));"
            ' 2>/dev/null) || ready=
        if [ "${ready}" = t ]; then
            printf '[INFO] Greengage coordinator and two primary segments are ready\n'
            return 0
        fi
        sleep 2
    done
    greengage_start_failure "cluster did not become ready within ${startup_timeout}s"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    if [ "${1:-}" = --bootstrap ]; then
        greengage_bootstrap
    else
        printf 'This file is a library for run-tests.sh.\n' >&2
        exit 1
    fi
fi
