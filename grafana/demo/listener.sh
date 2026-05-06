#!/usr/bin/env bash
# Session-mode demo workload. Holds three long-lived sessions LISTENing on
# `app_events` (only possible in session pool mode) and a fourth session
# inserting one row into notify_queue every five seconds. The trigger on
# notify_queue raises NOTIFY, which the LISTEN sessions receive — keeping
# `pg_doorman_pools_clients{user="app_session"}` non-zero on idle for the
# Web UI and Grafana to surface.
set -e

until pg_isready -h pg_doorman -p 6432 -U app_session -d app_db; do
    echo "waiting for pg_doorman ..."
    sleep 2
done

trap 'kill 0' EXIT INT TERM

for i in 1 2 3; do
    PGPASSWORD=session_pass psql \
        -h pg_doorman -p 6432 -U app_session -d app_db \
        -c "LISTEN app_events;" \
        -c "SELECT pg_sleep(86400);" \
        >"/tmp/listener-$i.log" 2>&1 &
done

while true; do
    PGPASSWORD=session_pass psql \
        -h pg_doorman -p 6432 -U app_session -d app_db \
        -c "INSERT INTO notify_queue(payload) VALUES ('event-' || now()::TEXT);" \
        >/dev/null 2>&1 || true
    sleep 5
done
