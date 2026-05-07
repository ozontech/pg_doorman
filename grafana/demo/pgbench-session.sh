#!/bin/bash
# Session-mode pgbench: keeps 4 long-lived clients pinned to backends through
# the app_session pool. Lower concurrency than the transaction-mode pgbench
# scripts because each client holds a backend for the full session lifetime —
# pool_size = 5, leave 1 slot free for the listener producer.
set -e

until pg_isready -h pg_doorman -p 6432 -U app_session -d app_db 2>/dev/null; do
    sleep 1
done

# pgbench tables already initialised by pgbench.sh on the postgres backend;
# the session pool reuses them.
exec pgbench \
    -h pg_doorman -p 6432 -U app_session \
    -c 4 -j 1 -T 999999 -P 30 \
    -M simple \
    --select-only \
    --no-vacuum \
    app_db
