#!/usr/bin/env python3
"""Compute pgbench latency percentiles on the bench host before tarring results.

For every <test>.log produced by setup-and-run-bench.sh, walk the matching
<test>_pgbenchlog.* per-transaction files, sort the third column (latency in
microseconds — see pgbench --log docs), pick nearest-rank p50/p95/p99 and
write <test>_percentiles.json next to the .log file.

Doing this on the VM keeps the raw pgbench --log files (multi-gigabyte ASCII,
one row per transaction) off the wire: parse-bench-logs.py only needs the
percentiles, so the post-bench tarball drops from gigabytes to megabytes.

Nearest-rank semantics match parse-bench-logs.py::percentile_ms and
tests/bdd/pgbench_helper.rs::percentile_index — keep them in sync.

stdlib only (Python 3.10+).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# Stems setup-and-run-bench.sh leaves in the results dir for debugging.
# Mirrors parse-bench-logs.py::SERVICE_LOG_NAMES.
SERVICE_LOG_NAMES = {
    "doorman", "odyssey", "pgbouncer", "pg",
    "bench-wrap",
}


def percentile_ms(sorted_us: list[float], frac: float) -> float | None:
    """Nearest-rank percentile, microseconds → milliseconds."""
    if not sorted_us:
        return None
    n = len(sorted_us)
    idx = max(0, min(n - 1, int(n * frac + 0.999999) - 1))
    return sorted_us[idx] / 1000.0


def collect_latencies_us(results_dir: Path, name: str) -> list[float]:
    """Return latency samples (µs) from every <name>_pgbenchlog.* file."""
    samples: list[float] = []
    for f in sorted(results_dir.glob(f"{name}_pgbenchlog.*")):
        for line in f.read_text(errors="replace").splitlines():
            parts = line.split()
            if len(parts) < 3:
                continue
            try:
                samples.append(float(parts[2]))
            except ValueError:
                pass
    return samples


def summarize(latencies_us: list[float]) -> dict:
    """Sort in place and return the {samples, p50_ms, p95_ms, p99_ms} payload."""
    latencies_us.sort()
    return {
        "samples": len(latencies_us),
        "p50_ms": percentile_ms(latencies_us, 0.50),
        "p95_ms": percentile_ms(latencies_us, 0.95),
        "p99_ms": percentile_ms(latencies_us, 0.99),
    }


def write_percentiles_for_results_dir(results_dir: Path) -> int:
    """Emit <name>_percentiles.json for every pgbench test log. Returns count."""
    count = 0
    for log_file in sorted(results_dir.glob("*.log")):
        name = log_file.stem
        if name in SERVICE_LOG_NAMES:
            continue
        payload = summarize(collect_latencies_us(results_dir, name))
        (results_dir / f"{name}_percentiles.json").write_text(json.dumps(payload))
        count += 1
    return count


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("results_dir", help="path to bench-results directory")
    args = ap.parse_args()
    results_dir = Path(args.results_dir)
    if not results_dir.is_dir():
        sys.exit(f"not a directory: {results_dir}")
    count = write_percentiles_for_results_dir(results_dir)
    print(f"wrote percentiles for {count} test(s)")


if __name__ == "__main__":
    main()
