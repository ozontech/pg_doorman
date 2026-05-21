#!/bin/bash
# pg_doorman_diag.sh — capture a forensic snapshot of a running pg_doorman.
#
# Run on a host whose pg_doorman is misbehaving (EMFILE storm, accept-loop
# spin, runaway fd usage). Bundles fd type breakdown, admin SHOW output,
# kernel socket state, journal, config, and a 3-second strace into a
# tarball under /tmp.
#
# Designed to be safe on a non-storming host as well: every step has a
# short timeout so the script never blocks on a stuck daemon.
#
# Requirements: bash, sudo. The host must have pg_doorman, psql, ss, and
# ideally strace installed. Missing tools degrade individual steps; the
# rest of the snapshot still ships.

set -u

# pgrep -ox: exact match on the comm field (16-char Linux process name),
# `-o` returns the oldest match. We need exact because the host typically
# also runs `pg_doorman_pam` (a PAM helper) whose comm starts with the
# same prefix. Substring-style `pgrep pg_doorman` matches both; the
# oldest is usually the PAM helper, not the actual pooler.
PID=$(pgrep -ox pg_doorman)
if [ -z "$PID" ]; then
    echo "ERROR: no pg_doorman process (exact match) found"
    echo "ps with pg_doorman in name, for reference:"
    pgrep -af pg_doorman || true
    exit 1
fi

USER_OF_PID=$(stat -c %U "/proc/$PID" 2>/dev/null || echo "unknown")
TS=$(date -u +%Y%m%dT%H%M%SZ)
H=$(hostname -s)
D=/tmp/doorman-diag-$H-$TS
mkdir -p "$D"

# 0. Process identity
date -u                                                     > "$D/00-date.txt"
{
    echo "PID=$PID"
    echo "owner=$USER_OF_PID"
    echo
    ps -o pid,ppid,user,nice,rss,vsz,etime,pcpu,cmd -p "$PID"
}                                                           > "$D/01-pid.txt"
cat "/proc/$PID/limits"                                     > "$D/02-limits.txt"
cat "/proc/$PID/status"                                     > "$D/03-status.txt"
tr '\0' ' ' < "/proc/$PID/cmdline"                          > "$D/04-cmdline.txt"
echo                                                       >> "$D/04-cmdline.txt"

# 1. fd table — readlink under the process owner so ptrace_scope=1
# does not block us. `sudo -u $USER_OF_PID` is the cheap way to drop
# from root into the daemon's uid for one command.
fd_under_uid() {
    sudo -u "$USER_OF_PID" bash -c "readlink /proc/$PID/fd/* 2>/dev/null"
}
sudo -u "$USER_OF_PID" ls -la "/proc/$PID/fd"               > "$D/10-fd-listing.txt" 2>&1 || true
fd_under_uid \
    | sed -E 's/:\[[0-9]+\]/:[N]/' \
    | sort | uniq -c | sort -rn                             > "$D/11-fd-by-type.txt"
fd_under_uid | grep -c '^socket:'                           > "$D/12-fd-socket-count.txt"
fd_under_uid | grep -c '^pipe:'                             > "$D/13-fd-pipe-count.txt"
fd_under_uid | grep -c '^anon_inode:'                       > "$D/14-fd-anon-count.txt"
ls "/proc/$PID/fd" 2>/dev/null | wc -l                      > "$D/15-fd-total.txt"

# 2. Kernel-visible socket state — these are scoped to the netns
# /proc/$PID is in. ss is faster and richer than /proc/net/tcp.
ss -tan '( sport = :6432 or dport = :6432 )'                > "$D/20-ss-all.txt"
ss -tan '( sport = :6432 )' | awk 'NR>1{print $1}' | sort | uniq -c \
                                                            > "$D/21-ss-client-state.txt"
ss -tan '( dport = :6432 )' | awk 'NR>1{print $1}' | sort | uniq -c \
                                                            > "$D/22-ss-backend-state.txt"
sudo -u "$USER_OF_PID" wc -l "/proc/$PID/net/tcp" \
                            "/proc/$PID/net/tcp6" \
                            "/proc/$PID/net/unix"           > "$D/23-net-counts.txt" 2>&1

# 3. /metrics — Prometheus endpoint, may be off or on a custom port.
timeout 3 curl -sf http://127.0.0.1:7777/metrics            > "$D/30-metrics.txt" 2>&1 \
    || echo "metrics unreachable on :7777" >> "$D/30-metrics.txt"

# 4. Admin SHOW commands — go through TCP because the pooler may not
# listen on a Unix socket. Credentials come from the config; we feed
# them to psql via PGPASSWORD so the password never lands in argv.
CFG=/etc/pg_doorman/pg_doorman.toml
ADMIN_USER=$(awk -F'"' '/^[[:space:]]*admin_username[[:space:]]*=/{print $2; exit}' "$CFG" 2>/dev/null)
ADMIN_PASS=$(awk -F'"' '/^[[:space:]]*admin_password[[:space:]]*=/{print $2; exit}' "$CFG" 2>/dev/null)
admin_show() {
    local q="$1" out="$2"
    if [ -z "$ADMIN_USER" ] || [ -z "$ADMIN_PASS" ]; then
        echo "admin credentials not parsed from $CFG" > "$out"
        return
    fi
    PGPASSWORD="$ADMIN_PASS" timeout 3 \
        psql -h 127.0.0.1 -p 6432 -U "$ADMIN_USER" -d pgbouncer \
             -c "$q" > "$out" 2>&1
}
admin_show "SHOW POOLS"     "$D/31-show-pools.txt"
admin_show "SHOW SERVERS"   "$D/32-show-servers.txt"
admin_show "SHOW CLIENTS"   "$D/33-show-clients.txt"
admin_show "SHOW DATABASES" "$D/34-show-dbs.txt"
admin_show "SHOW STATS"     "$D/35-show-stats.txt"
admin_show "SHOW SOCKETS"   "$D/36-show-sockets.txt"

# 5. Syscall profile — 3 seconds of strace -c reveals whether the
# accept-loop is spinning on the same epoll/accept syscall.
sudo -u "$USER_OF_PID" timeout 3 strace -p "$PID" -c -f 2>&1 \
    | tail -50                                              > "$D/40-strace-count.txt"

# 6. Logs — try journalctl first; some runr setups bundle a wrapper
# that does not accept --since but does accept -n.
if command -v journalctl >/dev/null 2>&1 \
   && journalctl -u pg_doorman -n 5 --no-pager >/dev/null 2>&1; then
    journalctl -u pg_doorman -n 5000 --no-pager             > "$D/50-journal.txt" 2>&1
else
    runr logs pg_doorman -n 5000                             > "$D/50-journal.txt" 2>&1 \
        || echo "no journal source available" > "$D/50-journal.txt"
fi

# 7. Config + service unit. Useful to correlate LimitNOFILE vs
# max_connections vs pool_size sums.
cat "$CFG"                                                   > "$D/60-config.toml" 2>&1
for f in /etc/runr/pg_doorman.service \
         /etc/systemd/system/pg_doorman.service \
         /lib/systemd/system/pg_doorman.service; do
    [ -f "$f" ] && cat "$f" > "$D/61-runr-service.txt" && break
done

# Bundle
tar czf "$D.tar.gz" -C /tmp "$(basename "$D")"
ls -la "$D.tar.gz"
echo "READY: $D.tar.gz"
