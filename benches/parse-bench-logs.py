#!/usr/bin/env python3
"""Parse bench-results.tar.gz from setup-and-run-bench.sh and emit benchmarks.md.

Same shape as documentation/en/src/benchmarks.md: per-protocol sections with a
relative-throughput table (vs pgbouncer / vs odyssey) and an absolute-latency
table (p50/p95/p99 in one cell per pooler).

Latency percentiles come from pgbench `--log` files (column 3 = µs) — same
convention as tests/bdd/pgbench_helper.rs::compute_latency_percentiles.

stdlib only (Python 3.10+).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tarfile
import tempfile
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

TPS_RE = re.compile(r"^tps = ([\d.]+)", re.MULTILINE)
LAT_AVG_RE = re.compile(r"^latency average = ([\d.]+)", re.MULTILINE)
NAME_RE = re.compile(
    r"^(?P<pooler>pg_doorman|odyssey|pgbouncer)"
    r"(?:_(?P<ssl>ssl))?"
    # bench.feature names SSL+connect cases without an explicit proto segment
    # (they all use --protocol=simple), so proto is optional and we default it
    # to "simple" in parse_test_name.
    r"(?:_(?P<proto>simple|extended|prepared))?"
    r"(?:_(?P<connect>connect))?"
    r"_c(?P<clients>\d+)$"
)
SERVICE_LOG_NAMES = {"doorman", "odyssey", "pgbouncer", "pg"}

PROTO_ORDER = ("simple", "extended", "prepared")
# (ssl, connect) tuples in the same row order as the existing benchmarks.md.
MODE_ORDER = ((False, False), (False, True), (True, False), (True, True))
CLIENT_ORDER = (1, 40, 120, 500, 10000)


# ---------- pure helpers (covered by tests/test_parse_bench_logs.py) ----------


def parse_pgbench_stdout(text: str) -> dict:
    tps = TPS_RE.search(text)
    lat = LAT_AVG_RE.search(text)
    return {
        "tps": float(tps.group(1)) if tps else None,
        "lat_avg_ms": float(lat.group(1)) if lat else None,
    }


def percentile_ms(sorted_us: list[float], frac: float) -> float | None:
    if not sorted_us:
        return None
    n = len(sorted_us)
    # nearest-rank, matches tests/bdd/pgbench_helper.rs::percentile_index
    idx = max(0, min(n - 1, int(n * frac + 0.999999) - 1))
    return sorted_us[idx] / 1000.0


def parse_test_name(name: str) -> dict | None:
    m = NAME_RE.match(name)
    if not m:
        return None
    d = m.groupdict()
    return {
        "pooler": d["pooler"],
        "ssl": bool(d["ssl"]),
        "proto": d["proto"] or "simple",
        "connect": bool(d["connect"]),
        "clients": int(d["clients"]),
    }


def mode_label(ssl: bool, connect: bool) -> str:
    parts = []
    if ssl:
        parts.append("SSL")
    if connect:
        parts.append("Reconnect")
    return " + ".join(parts)


def row_label(clients: int, mode: str) -> str:
    word = "client" if clients == 1 else "clients"
    label = f"{clients:,} {word}"
    if mode:
        label += f" + {mode}"
    return label


def format_throughput(self_tps: float | None, other_tps: float | None) -> str:
    """Relative throughput cell: '+N%', '-N%', '≈0%', 'xN.N', '∞', 'N/A', '-'."""
    if self_tps is None and other_tps is None:
        return "-"
    if self_tps is None or other_tps is None:
        return "N/A"
    if other_tps == 0:
        return "∞" if self_tps > 0 else "-"
    ratio = self_tps / other_tps
    pct = (ratio - 1) * 100
    if abs(pct) < 3:
        return "≈0%"
    if ratio >= 1.5:
        return f"x{ratio:.1f}"
    sign = "+" if pct > 0 else ""
    return f"{sign}{pct:.0f}%"


def format_latency_triplet(rec: dict | None) -> str:
    """`p50 / p95 / p99` (ms, two decimals) or '-' when any percentile missing."""
    if not rec:
        return "-"
    p50 = rec.get("p50_ms")
    p95 = rec.get("p95_ms")
    p99 = rec.get("p99_ms")
    if any(v is None for v in (p50, p95, p99)):
        return "-"
    return f"{p50:.2f} / {p95:.2f} / {p99:.2f}"


# ---------- I/O ----------


def parse_test(name: str, src: Path) -> dict:
    rec = parse_pgbench_stdout((src / f"{name}.log").read_text(errors="replace"))
    latencies_us: list[float] = []
    for f in src.glob(f"{name}_pgbenchlog.*"):
        for line in f.read_text(errors="replace").splitlines():
            parts = line.split()
            if len(parts) < 3:
                continue
            try:
                latencies_us.append(float(parts[2]))
            except ValueError:
                pass
    latencies_us.sort()
    rec["p50_ms"] = percentile_ms(latencies_us, 0.50)
    rec["p95_ms"] = percentile_ms(latencies_us, 0.95)
    rec["p99_ms"] = percentile_ms(latencies_us, 0.99)
    rec["samples"] = len(latencies_us)
    return rec


# ---------- rendering ----------


def emit_metadata(meta: dict | None) -> list[str]:
    if not meta:
        return []
    mem_kb = meta.get("memory_kb")
    mem_gb = f"{mem_kb / 1024 / 1024:.1f}" if mem_kb else "?"
    return [
        "### Environment",
        "",
        f"- **Host**: {meta.get('host', '?')}",
        f"- **Resources**: {meta.get('vcpus', '?')} vCPU / {mem_gb} GB",
        f"- **Workers**: pg_doorman: {meta.get('doorman_workers', '?')}, "
        f"odyssey: {meta.get('odyssey_workers', '?')}",
        f"- **Duration per pgbench run**: {meta.get('duration_per_run_sec', '?')}s",
        f"- **Started**: {meta.get('started_at', '?')}",
        f"- **Commit**: `{meta.get('git_sha', '?')}`",
        "",
    ]


LEGEND = [
    "### Reading the tables",
    "",
    "**Throughput** — pg_doorman TPS relative to each competitor:",
    "",
    "| Value | Meaning |",
    "|-------|---------|",
    "| +N% | Faster by N% |",
    "| -N% | Slower by N% |",
    "| ≈0% | Within 3% |",
    "| xN | N times faster or slower |",
    "| ∞ | Competitor got 0 TPS |",
    "| N/A | Unsupported |",
    "| - | Not tested |",
    "",
    "**Latency** — per-transaction latency in ms. Each cell: `p50 / p95 / p99`. Lower is better.",
    "",
]

NOTES = [
    "### Notes",
    "",
    "- Throughput values are relative ratios — comparable across runs on identical hardware",
    "- Latency values are absolute, measured per-transaction",
]


def _emit_throughput_table(proto_groups: dict[tuple, dict]) -> list[str]:
    out = [
        "### Throughput",
        "",
        "| Test | vs pgbouncer | vs odyssey |",
        "|------|--------------|------------|",
    ]
    for ssl, conn in MODE_ORDER:
        for c in CLIENT_ORDER:
            key = (ssl, conn, c)
            if key not in proto_groups:
                continue
            row = proto_groups[key]
            d = row.get("pg_doorman", {})
            b = row.get("pgbouncer", {})
            o = row.get("odyssey", {})
            label = row_label(c, mode_label(ssl, conn))
            out.append(
                f"| {label} | "
                f"{format_throughput(d.get('tps'), b.get('tps'))} | "
                f"{format_throughput(d.get('tps'), o.get('tps'))} |"
            )
    return out


def _emit_latency_table(proto_groups: dict[tuple, dict]) -> list[str]:
    out = [
        "### Latency — p50 / p95 / p99 (ms)",
        "",
        "| Test | pg_doorman (ms) | pgbouncer (ms) | odyssey (ms) |",
        "|------|----------------|----------------|--------------|",
    ]
    for ssl, conn in MODE_ORDER:
        for c in CLIENT_ORDER:
            key = (ssl, conn, c)
            if key not in proto_groups:
                continue
            row = proto_groups[key]
            label = row_label(c, mode_label(ssl, conn))
            out.append(
                f"| {label} | "
                f"{format_latency_triplet(row.get('pg_doorman'))} | "
                f"{format_latency_triplet(row.get('pgbouncer'))} | "
                f"{format_latency_triplet(row.get('odyssey'))} |"
            )
    return out


def render(results: dict[str, dict], meta: dict | None) -> str:
    groups: dict[tuple, dict[str, dict]] = defaultdict(dict)
    unmatched: list[str] = []
    for name, data in results.items():
        n = parse_test_name(name)
        if n is None:
            unmatched.append(name)
            continue
        key = (n["proto"], n["ssl"], n["connect"], n["clients"])
        groups[key][n["pooler"]] = data

    duration = (meta or {}).get("duration_per_run_sec", "?")
    out: list[str] = [
        "---",
        "title: Benchmarks",
        "---",
        "",
        "# Benchmarks",
        "",
        f"pg_doorman vs pgbouncer vs odyssey. Each test runs `pgbench` for {duration} seconds through a 40-connection pool.",
        "",
        f"Last updated: {datetime.now(timezone.utc):%Y-%m-%d %H:%M UTC}",
        "",
    ]
    out += emit_metadata(meta)
    out += LEGEND

    for proto in PROTO_ORDER:
        proto_groups = {
            (ssl, conn, c): data
            for (p, ssl, conn, c), data in groups.items()
            if p == proto
        }
        if not proto_groups:
            continue
        out += [
            "---",
            "",
            f"## {proto.capitalize()} protocol",
            "",
        ]
        out += _emit_throughput_table(proto_groups)
        out.append("")
        out += _emit_latency_table(proto_groups)
        out.append("")

    out += ["---", ""]
    out += NOTES

    if unmatched:
        out += ["", "### Unparsed test names", ""]
        out += [f"- {n}" for n in sorted(unmatched)]

    out.append("")
    return "\n".join(out)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("tarball", help="bench-results.tar.gz")
    ap.add_argument("output", help="output markdown file")
    args = ap.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        with tarfile.open(args.tarball, "r:gz") as tf:
            # filter="data" rejects path-traversal entries; tarball is local so
            # this is mostly future-proofing for Python 3.14's default change.
            tf.extractall(tmp, filter="data")
        src = Path(tmp) / "bench-results"
        if not src.is_dir():
            sys.exit("tarball missing 'bench-results/' dir")

        meta = None
        meta_file = src / "metadata.json"
        if meta_file.exists():
            meta = json.loads(meta_file.read_text())

        results: dict[str, dict] = {}
        for log_file in sorted(src.glob("*.log")):
            name = log_file.stem
            if name in SERVICE_LOG_NAMES:
                continue
            results[name] = parse_test(name, src)

        Path(args.output).write_text(render(results, meta))
        print(f"wrote {args.output} with {len(results)} tests")


if __name__ == "__main__":
    main()
