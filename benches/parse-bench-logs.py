#!/usr/bin/env python3
"""Parse bench-results.tar.gz from setup-and-run-bench.sh and emit benchmarks.md.

Layout follows documentation/en/src/benchmarks.md: TL;DR + environment +
methodology + per-protocol sections (relative-throughput table and absolute
p50/p95/p99 latency table) + caveats.

Latency percentiles come from pgbench `--log` files (column 3 = µs) — same
nearest-rank convention as tests/bdd/pgbench_helper.rs::compute_latency_percentiles.

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
    # (they all use --protocol=simple), so proto is optional and defaults to
    # "simple" in parse_test_name.
    r"(?:_(?P<proto>simple|extended|prepared))?"
    r"(?:_(?P<connect>connect))?"
    r"_c(?P<clients>\d+)$"
)
# Stems that the script puts in bench-results/ for debugging — never tests.
SERVICE_LOG_NAMES = {
    "doorman", "odyssey", "pgbouncer", "pg",
    "bench-wrap",  # added by the workflow's nohup wrapper
}

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


def parse_iso8601_z(value: str | None) -> datetime | None:
    """Accept '2026-04-27T05:14:44Z' or with offset; return tz-aware datetime."""
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def format_duration(seconds: float) -> str:
    seconds = int(seconds)
    if seconds < 60:
        return f"{seconds}s"
    h, rem = divmod(seconds, 3600)
    m, s = divmod(rem, 60)
    if h:
        return f"{h}h {m:02d}m {s:02d}s"
    return f"{m}m {s:02d}s"


def compute_tldr(groups: dict[tuple, dict[str, dict]]) -> list[str]:
    """Pick three headline numbers worth showing above the fold.

    Strategy: find the highest pg_doorman speedup vs each competitor at >=40
    clients in the simple/extended/prepared protocols (no Reconnect, no SSL),
    plus the worst p99 spread on the steady-state simple-protocol curve.
    Returns markdown bullet lines. Empty list if data is too sparse.
    """
    bullets: list[str] = []

    def best_speedup(other: str) -> tuple[str, float] | None:
        best: tuple[str, float] | None = None
        for (proto, ssl, conn, clients), poolers in groups.items():
            if ssl or conn or clients < 40:
                continue
            d_tps = (poolers.get("pg_doorman") or {}).get("tps")
            o_tps = (poolers.get(other) or {}).get("tps")
            if not d_tps or not o_tps:
                continue
            ratio = d_tps / o_tps
            if best is None or ratio > best[1]:
                label = row_label(clients, "")
                best = (f"{proto} protocol, {label}", ratio)
        return best

    for other in ("pgbouncer", "odyssey"):
        b = best_speedup(other)
        if b is None:
            continue
        scenario, ratio = b
        if ratio >= 1.5:
            bullets.append(
                f"- **vs {other}** — pg_doorman peaks at **x{ratio:.1f}** TPS "
                f"on {scenario}."
            )
        elif ratio >= 1.03:
            bullets.append(
                f"- **vs {other}** — pg_doorman wins by "
                f"**+{(ratio - 1) * 100:.0f}%** at most ({scenario})."
            )
        else:
            bullets.append(
                f"- **vs {other}** — within ±3% on steady-state simple/extended/prepared workloads."
            )

    # Latency under load: simple protocol, 10k clients, no SSL/Reconnect.
    key = ("simple", False, False, 10000)
    if key in groups:
        row = groups[key]
        rows = []
        for pooler in ("pg_doorman", "pgbouncer", "odyssey"):
            p99 = (row.get(pooler) or {}).get("p99_ms")
            if p99 is not None:
                rows.append((pooler, p99))
        if rows:
            rows.sort(key=lambda x: x[1])
            best_pooler, best_p99 = rows[0]
            tail = ", ".join(f"{name} {p99:.0f}ms" for name, p99 in rows[1:])
            bullets.append(
                f"- **Tail latency at 10 000 simple-protocol clients** — "
                f"{best_pooler} **p99 {best_p99:.0f}ms** (others: {tail})."
            )

    return bullets


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
    versions = meta.get("versions") or {}
    pg_v = versions.get("postgres", "?")
    pgb_v = versions.get("pgbouncer", "?")
    od_v = versions.get("odyssey", "?")
    door_v = versions.get("pg_doorman", "?")
    vm_size = meta.get("vm_size") or "unknown"
    sha = meta.get("git_sha") or ""
    started = meta.get("started_at")
    finished = meta.get("finished_at")
    started_dt = parse_iso8601_z(started)
    finished_dt = parse_iso8601_z(finished)
    duration = (
        format_duration((finished_dt - started_dt).total_seconds())
        if started_dt and finished_dt
        else "?"
    )
    started_pretty = started_dt.strftime("%Y-%m-%d %H:%M UTC") if started_dt else "?"
    finished_pretty = finished_dt.strftime("%Y-%m-%d %H:%M UTC") if finished_dt else "?"

    lines = [
        "### Environment",
        "",
        f"- **Provider**: Ubicloud `{vm_size}` (eu-central-h1)",
        f"- **Resources**: {meta.get('vcpus', '?')} vCPU / {mem_gb} GB",
        f"- **Kernel**: `{meta.get('kernel', '?')}`",
        f"- **Versions**: PostgreSQL {pg_v}, pg_doorman {door_v}, "
        f"pgbouncer {pgb_v}, odyssey {od_v}",
        f"- **Workers**: pg_doorman: {meta.get('doorman_workers', '?')}, "
        f"odyssey: {meta.get('odyssey_workers', '?')}",
        f"- **Duration per pgbench run**: {meta.get('duration_per_run_sec', '?')}s",
        f"- **Started**: {started_pretty}",
        f"- **Finished**: {finished_pretty}",
        f"- **Total wall-clock**: {duration}",
    ]
    if sha and sha != "unknown":
        lines.append(f"- **Commit**: [`{sha[:8]}`](https://github.com/ozontech/pg_doorman/commit/{sha})")
    lines.append("")
    return lines


METHODOLOGY = [
    "### Methodology",
    "",
    "Each scenario runs `pgbench -T <duration>` against a 40-connection",
    "server-side pool (`pool_mode = transaction`). The workload is a single",
    "`SELECT :aid` (`\\set aid random(1, 100000)`) — pure pooler overhead, no",
    "real working set. Three poolers, one PostgreSQL backend, identical",
    "hardware.",
    "",
    "- **Reconnect** rows use `pgbench --connect`: a fresh TCP+startup per",
    "  transaction (worst case for login latency).",
    "- **SSL** rows set `PGSSLMODE=require` and a self-signed cert.",
    "- Latency is collected with `pgbench --log` (per-transaction file);",
    "  percentiles come from those samples, not from `pgbench` summary stats.",
    "- Scenarios run sequentially with the same data dir and warm OS caches.",
    "",
    "Source: [`tests/bdd/features/bench.feature`](https://github.com/ozontech/pg_doorman/blob/master/tests/bdd/features/bench.feature),",
    "driver: [`benches/setup-and-run-bench.sh`](https://github.com/ozontech/pg_doorman/blob/master/benches/setup-and-run-bench.sh).",
    "",
]


LEGEND = [
    "### Reading the tables",
    "",
    "**Throughput** — `pg_doorman_TPS / competitor_TPS`, rendered:",
    "",
    "| Value | Meaning |",
    "|-------|---------|",
    "| +N% / -N% | Faster / slower by N percent |",
    "| ≈0% | Within 3% — call it a tie |",
    "| xN.N | N times faster (when ratio ≥ 1.5) |",
    "| ∞ | Competitor returned 0 TPS |",
    "| N/A | Competitor was not measured for this row |",
    "| - | Not measured for either pooler |",
    "",
    "**Latency** — per-transaction in ms, `p50 / p95 / p99` per cell. Lower is",
    "better. Compare the same column across rows for one pooler, or across",
    "columns at one row for head-to-head.",
    "",
]


CAVEATS = [
    "### Caveats",
    "",
    "- 30 s per run is short by `pgbench` standards (the docs recommend",
    "  minutes); expect ±5% variance between runs. Re-run for production",
    "  decisions.",
    "- Single PostgreSQL backend, no replicas, no real working set — these",
    "  numbers measure pooler overhead, not full-system throughput.",
    "- All three poolers use vendor defaults plus `pool_size = 40`.",
    "  Tuning specific knobs (`pgbouncer so_reuseport`, `odyssey workers`)",
    "  will move the curves.",
    "- `Reconnect` is the worst-case login-latency scenario; the headline",
    "  numbers in production rarely look like the Reconnect rows.",
    "- Workload is a 1-row `SELECT`. Read-heavy OLTP, OLAP, or `LISTEN`/",
    "  `NOTIFY` paths are not represented.",
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

    if unmatched:
        print(f"warning: {len(unmatched)} unmatched test name(s) ignored: "
              f"{', '.join(sorted(unmatched))}", file=sys.stderr)

    out: list[str] = [
        "---",
        "title: Benchmarks",
        "---",
        "",
        "# Benchmarks",
        "",
        "Three connection poolers — pg_doorman, pgbouncer, odyssey — driven",
        "by `pgbench` against the same PostgreSQL backend on identical",
        "hardware. Numbers below are relative throughput against each",
        "competitor and absolute per-transaction latency.",
        "",
        f"_Last updated: {datetime.now(timezone.utc):%Y-%m-%d %H:%M UTC}._",
        "",
    ]

    tldr = compute_tldr(groups)
    if tldr:
        out += ["## TL;DR", ""] + tldr + [""]

    out += emit_metadata(meta)
    out += METHODOLOGY
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
    out += CAVEATS
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
