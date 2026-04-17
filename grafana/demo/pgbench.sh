#!/bin/bash
set -e

# Wait for pg_doorman to be ready
until pg_isready -h pg_doorman -p 6432 -U app_user -d app_db 2>/dev/null; do
  sleep 1
done

# Initialize pgbench tables directly on postgres (not through the pooler)
PGPASSWORD=app_pass pgbench -i -h postgres -p 5432 -U app_user app_db 2>/dev/null || true

# Run continuous load through pg_doorman with prepared statements
exec pgbench -h pg_doorman -p 6432 -U app_user -c 20 -j 4 -T 999999 -P 10 \
  -M prepared app_db
