#!/usr/bin/env bash
# Count scheduler events for one process under load. Requires Linux bpftrace.
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
    echo "Usage: sudo $0 PID [SECONDS]" >&2
    exit 2
fi

pid=$1
seconds=${2:-10}
if [[ ! $pid =~ ^[1-9][0-9]*$ || ! $seconds =~ ^[1-9][0-9]*$ ]]; then
    echo "PID and SECONDS must be positive integers" >&2
    exit 2
fi

# pid is the thread-group ID, so all worker threads are included.
exec bpftrace -e "
tracepoint:syscalls:sys_enter_futex /pid == ${pid}/ { @futex = count(); }
tracepoint:syscalls:sys_enter_nanosleep /pid == ${pid}/ { @nanosleep = count(); }
tracepoint:sched:sched_wakeup /args->pid == ${pid}/ { @wakeups = count(); }
interval:s:${seconds} {
    print(@futex);
    print(@nanosleep);
    print(@wakeups);
    exit();
}
"
