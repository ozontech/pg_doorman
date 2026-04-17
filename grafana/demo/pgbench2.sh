#!/bin/bash
set -e

# Wait for pg_doorman to be ready
until pg_isready -h pg_doorman -p 6432 -U app_user_2 -d app_db 2>/dev/null; do
  sleep 1
done

# Run continuous load as second user with prepared statements
exec pgbench -h pg_doorman -p 6432 -U app_user_2 -c 10 -j 2 -T 999999 -P 10 \
  -M prepared app_db
