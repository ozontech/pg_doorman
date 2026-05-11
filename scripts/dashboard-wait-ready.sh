#!/usr/bin/env bash
# Block until grafana/demo has accumulated enough Prometheus scrape
# points for rate()-based panels and ground-truth checks to be
# meaningful. Used by `make dashboard-up`.
#
# Why polling instead of `sleep 90`:
#   - On a fresh CI runner the docker pull alone can take 60 seconds,
#     so a flat 90 s sleep risks the demo not being fully up yet.
#   - On a developer laptop with warm caches the demo is ready in 30 s
#     and a flat sleep is wasted time.
#
# Defaults can be overridden by the operator:
#   PROM_URL    Prometheus base URL (default localhost:19090).
#   MIN_POINTS  How many scrape points must be present.
#   MAX_WAIT    Hard timeout in seconds.

set -euo pipefail

PROM_URL=${PROM_URL:-http://localhost:19090}
MIN_POINTS=${MIN_POINTS:-12}
MAX_WAIT=${MAX_WAIT:-300}
POLL_INTERVAL=${POLL_INTERVAL:-3}

probe_query="count_over_time(pg_doorman_pools_queries_total%5B1m%5D)"
deadline=$(( $(date +%s) + MAX_WAIT ))

echo "dashboard-wait-ready: polling ${PROM_URL} until rate() has ${MIN_POINTS}+ points"

while [[ $(date +%s) -lt ${deadline} ]]; do
    body=$(curl --silent --max-time 5 "${PROM_URL}/api/v1/query?query=${probe_query}" || true)
    if [[ -n "${body}" ]]; then
        # Pull the largest count value across all returned series. Any
        # series at MIN_POINTS or above means "Prometheus has scraped
        # pg_doorman that many times under sustained traffic".
        max_points=$(printf '%s' "${body}" | python3 -c '
import json
import sys

try:
    data = json.load(sys.stdin)
except json.JSONDecodeError:
    print(0)
    sys.exit(0)

series = data.get("data", {}).get("result", [])
best = 0
for s in series:
    value = s.get("value")
    if not value or len(value) < 2:
        continue
    try:
        best = max(best, float(value[1]))
    except (TypeError, ValueError):
        continue
print(int(best))
' || echo 0)
        if (( max_points >= MIN_POINTS )); then
            echo "dashboard-wait-ready: ${max_points} points — ready"
            exit 0
        fi
        echo "dashboard-wait-ready: ${max_points}/${MIN_POINTS} points, sleeping ${POLL_INTERVAL}s"
    fi
    sleep "${POLL_INTERVAL}"
done

echo "dashboard-wait-ready: timed out after ${MAX_WAIT}s without enough scrape points" >&2
exit 1
