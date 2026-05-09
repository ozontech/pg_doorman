#!/usr/bin/env python3
"""Slot-2 ground-truth correlation for grafana/pg_doorman.json.

For every check in scripts/dashboard-ground-truth.checks.yaml the script
takes two snapshots:

  prometheus — instant PromQL query (the value the dashboard would show);
  truth      — an independent source: docker logs of the pg_doorman
               container, psql to Postgres directly, the TOML config,
               or /proc/1/status.

Then it compares: max(prom, truth) / min(prom, truth) <= ratio. The
default ratio is 10 — one order of magnitude. The point is to catch
errors of magnitude (50K qps in the log vs 5K in Prometheus), not the
epsilon-level differences between the two p99 algorithms.

If a check declares exact: true, only equality is accepted (used for
static values such as pool_size).

Exit codes:
    0 — every ratio fits the limit;
    1 — at least one ratio violated.

Run from the repository root with grafana/demo up:
    python3 scripts/dashboard-ground-truth.py
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

# tomllib is stdlib in 3.11+. On 3.10 (current ubuntu-22.04 GitHub
# runner default) fall back to the tomli package, which has the same
# `loads` / `load` surface. PyYAML and tomli are both single-pip
# installs, the rest of the script is stdlib-only.
try:
    import tomllib  # type: ignore[import-not-found]
except ModuleNotFoundError:  # Python <= 3.10
    import tomli as tomllib  # type: ignore[no-redef]

import yaml


LOG_LINE_RE = re.compile(
    # Standard print_all_stats line: `[user@db]`. The pattern is locked
    # to that exact shape so the ANSI escape prefixes (`[32m` ... `[0m`)
    # at the start of the log line do not get swallowed by a greedy
    # `[^\]]+` match.
    r"\[(?P<pool>[a-zA-Z0-9_]+@[a-zA-Z0-9_]+)\]\s+"
    r"qps=(?P<qps>\d+(?:\.\d+)?)\s+"
    r"tps=(?P<tps>\d+(?:\.\d+)?)\s+\|\s+"
    r"clients=(?P<clients_total>\d+)\s+active=(?P<clients_active>\d+)\s+"
    r"idle=(?P<clients_idle>\d+)\s+wait=(?P<clients_wait>\d+)\s+\|\s+"
    r"servers=(?P<servers_total>\d+)\s+active=(?P<servers_active>\d+)\s+"
    r"idle=(?P<servers_idle>\d+)\s+\|\s+"
    r"query_ms\s+p50=(?P<query_p50>\d+(?:\.\d+)?)\s+"
    r"p90=(?P<query_p90>\d+(?:\.\d+)?)\s+"
    r"p95=(?P<query_p95>\d+(?:\.\d+)?)\s+"
    r"p99=(?P<query_p99>\d+(?:\.\d+)?)\s+\|\s+"
    r"xact_ms\s+p50=(?P<xact_p50>\d+(?:\.\d+)?)\s+"
    r"p90=(?P<xact_p90>\d+(?:\.\d+)?)\s+"
    r"p95=(?P<xact_p95>\d+(?:\.\d+)?)\s+"
    r"p99=(?P<xact_p99>\d+(?:\.\d+)?)"
)

LOG_FIELD_KEYS = {
    "qps": "qps",
    "tps": "tps",
    "clients.total": "clients_total",
    "clients.active": "clients_active",
    "clients.idle": "clients_idle",
    "clients.wait": "clients_wait",
    "servers.total": "servers_total",
    "servers.active": "servers_active",
    "servers.idle": "servers_idle",
    "query_ms.p50": "query_p50",
    "query_ms.p90": "query_p90",
    "query_ms.p95": "query_p95",
    "query_ms.p99": "query_p99",
    "xact_ms.p50": "xact_p50",
    "xact_ms.p90": "xact_p90",
    "xact_ms.p95": "xact_p95",
    "xact_ms.p99": "xact_p99",
}


@dataclass
class CheckResult:
    name: str
    prom_value: float | None
    truth_value: float | None
    ok: bool
    reason: str = ""


def prometheus_query(base_url: str, query: str, timeout: float) -> float | None:
    url = f"{base_url.rstrip('/')}/api/v1/query?{urllib.parse.urlencode({'query': query})}"
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        data = json.loads(resp.read().decode("utf-8"))
    if data.get("status") != "success":
        return None
    result = data.get("data", {}).get("result", [])
    if not result:
        return None
    values: list[float] = []
    for series in result:
        v = series.get("value")
        if not v or len(v) < 2:
            continue
        try:
            values.append(float(v[1]))
        except (TypeError, ValueError):
            continue
    if not values:
        return None
    # Multiple series get summed: the ground-truth side is usually
    # already aggregated per pool, so summing makes the two values
    # comparable. Single-series results pass through unchanged.
    return sum(values)


def latest_log_pool_metric(
    container: str, pool: str, key: str, last_n: int = 4
) -> float | None:
    """Mean of the last ``last_n`` print_all_stats values for ``pool``.

    pg_doorman emits a stats line every ``STAT_PERIOD`` seconds (15 s
    in 3.7+). Prometheus rate windows in this PR are ``[1m]``, so a
    single 15-second log sample sits in a much narrower window than
    the Prometheus side and shows ~10–20 % more variance. Averaging
    the last four samples gives the same ~60-second window as the
    Prometheus rate, removing a class of false positives that came
    from the 15 s vs 60 s mismatch alone.
    """
    try:
        proc = subprocess.run(
            ["docker", "logs", "--since", "5m", container],
            check=True,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
        raise RuntimeError(f"docker logs {container}: {e}") from e
    # pg_doorman writes its log to stderr (tracing fmt default). Both
    # streams are concatenated to keep working if logs ever move to
    # stdout in a future release.
    out = proc.stdout + proc.stderr
    if key not in LOG_FIELD_KEYS:
        raise ValueError(f"unknown log field: {key}")
    field = LOG_FIELD_KEYS[key]
    values: list[float] = []
    for line in out.splitlines():
        m = LOG_LINE_RE.search(line)
        if not m or m.group("pool") != pool:
            continue
        values.append(float(m.group(field)))
    if not values:
        return None
    window_size = max(1, last_n)
    window = values[-window_size:]
    return sum(window) / len(window)


def psql_scalar(dsn: dict[str, Any], query: str) -> float | None:
    env = os.environ.copy()
    env["PGPASSWORD"] = str(dsn.get("password", ""))
    cmd = [
        "psql",
        "-h",
        str(dsn.get("host", "localhost")),
        "-p",
        str(dsn.get("port", 5432)),
        "-U",
        str(dsn.get("user", "postgres")),
        "-d",
        str(dsn.get("dbname", "postgres")),
        "-At",
        "-c",
        query,
    ]
    proc = subprocess.run(
        cmd, capture_output=True, text=True, env=env, timeout=10
    )
    if proc.returncode != 0:
        raise RuntimeError(f"psql exit {proc.returncode}: {proc.stderr.strip()}")
    text = proc.stdout.strip()
    if not text:
        return None
    return float(text.splitlines()[0])


PATH_TOKEN_RE = re.compile(r"([^.\[\]]+)|\[(\d+)\]")


def toml_lookup(file: str, path: str) -> Any:
    with open(file, "rb") as f:
        data: Any = tomllib.load(f)
    cur = data
    for token in PATH_TOKEN_RE.finditer(path):
        key, idx = token.group(1), token.group(2)
        if key is not None:
            if not isinstance(cur, dict) or key not in cur:
                raise KeyError(f"key '{key}' missing at {path}")
            cur = cur[key]
        else:
            cur = cur[int(idx)]
    return cur


def toml_array_lookup(
    file: str, path: str, filter_field: str, filter_value: Any, field: str
) -> Any:
    """Pick one entry from a TOML array of tables by predicate.

    Used for things like ``pools.app_db.users[username='app_session'].pool_size``
    so the check does not depend on a fragile array index. A cosmetic
    reordering of ``[[pools.app_db.users]]`` no longer silently breaks
    the assertion.
    """
    array = toml_lookup(file, path)
    if not isinstance(array, list):
        raise KeyError(f"path '{path}' is not an array of tables")
    for item in array:
        if not isinstance(item, dict):
            continue
        if item.get(filter_field) == filter_value:
            if field not in item:
                raise KeyError(
                    f"field '{field}' missing in match for "
                    f"{filter_field}={filter_value!r} at {path}"
                )
            return item[field]
    raise KeyError(
        f"no entry matches {filter_field}={filter_value!r} in {path}"
    )


def psql_delta(
    dsn: dict[str, Any], query: str, interval_seconds: float
) -> float | None:
    """Snapshot, sleep, snapshot. Return per-second delta of a counter.

    Used to compare ``rate(pg_doorman_pools_transactions_total[1m])``
    with ``(xact_commit + xact_rollback) / interval`` from
    ``pg_stat_database`` — the first independent ground truth from
    Postgres itself, separate from any pg_doorman code path.
    """
    first = psql_scalar(dsn, query)
    if first is None:
        return None
    time.sleep(max(1.0, interval_seconds))
    second = psql_scalar(dsn, query)
    if second is None:
        return None
    return max(0.0, (second - first) / max(1.0, interval_seconds))


def proc_status_field(container: str, field: str) -> int:
    """Read a numeric kB field from /proc/1/status inside the container."""
    proc = subprocess.run(
        ["docker", "exec", container, "cat", "/proc/1/status"],
        capture_output=True,
        text=True,
        timeout=10,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"docker exec failed: {proc.stderr.strip()}")
    for line in proc.stdout.splitlines():
        if line.startswith(f"{field}:"):
            tokens = line.split()
            # Format: "VmRSS:    12345 kB"
            return int(tokens[1]) * 1024
    raise KeyError(f"field {field} not found in /proc/1/status")


def resolve_truth(
    truth: dict[str, Any], defaults: dict[str, Any]
) -> float | None:
    kind = truth["kind"]
    if kind == "pg_doorman_log":
        container = truth.get("container") or defaults["pg_doorman_container"]
        last_n = int(truth.get("last_n", 4))
        return latest_log_pool_metric(
            container, truth["pool"], truth["field"], last_n=last_n
        )
    if kind == "psql_query":
        dsn = {**defaults["postgres_dsn"], **(truth.get("dsn") or {})}
        return psql_scalar(dsn, truth["query"])
    if kind == "psql_delta":
        dsn = {**defaults["postgres_dsn"], **(truth.get("dsn") or {})}
        interval = float(truth.get("interval_seconds", 5.0))
        return psql_delta(dsn, truth["query"], interval)
    if kind == "toml_value":
        file = truth.get("file") or defaults["toml_file"]
        value = toml_lookup(file, truth["path"])
        return float(value)
    if kind == "toml_array":
        file = truth.get("file") or defaults["toml_file"]
        value = toml_array_lookup(
            file,
            truth["path"],
            truth["filter_field"],
            truth["filter_value"],
            truth["field"],
        )
        return float(value)
    if kind == "proc_status":
        container = truth.get("container") or defaults["pg_doorman_container"]
        return float(proc_status_field(container, truth["field"]))
    raise ValueError(f"unknown truth.kind: {kind}")


TRUTH_SOURCE_ERRORS: tuple[type[BaseException], ...] = (
    KeyError,
    ValueError,
    RuntimeError,
    OSError,
    subprocess.SubprocessError,
)


def _snapshot(
    check: dict[str, Any],
    defaults: dict[str, Any],
    prom_url: str,
    timeout: float,
) -> tuple[float | None, float | None, str]:
    """Take a single (Prometheus, truth) snapshot. Empty side returns None."""
    try:
        prom_value = prometheus_query(prom_url, check["prometheus"], timeout)
    except (urllib.error.HTTPError, urllib.error.URLError) as e:
        return None, None, f"prometheus error: {e}"
    if prom_value is None:
        return None, None, "prometheus returned no value"
    try:
        truth_value = resolve_truth(check["truth"], defaults)
    except TRUTH_SOURCE_ERRORS as e:
        return prom_value, None, f"truth source error: {type(e).__name__}: {e}"
    if truth_value is None:
        return prom_value, None, "truth source returned no value"
    return prom_value, truth_value, ""


def evaluate(
    check: dict[str, Any],
    defaults: dict[str, Any],
    prom_url: str,
    timeout: float,
    retries: int = 3,
    retry_backoff: float = 30.0,
) -> CheckResult:
    name = check["name"]
    prom_value: float | None = None
    truth_value: float | None = None
    reason = ""
    for attempt in range(max(1, retries)):
        prom_value, truth_value, reason = _snapshot(
            check, defaults, prom_url, timeout
        )
        # Retry only when the demo is still warming up — both sides must
        # be populated before the test is meaningful. A real deviation
        # (PromQL error, missing TOML key) shows up immediately and is
        # not retried because reason is set deterministically.
        if (
            prom_value is not None
            and truth_value is not None
            and prom_value > 0
            and truth_value > 0
        ):
            break
        if attempt + 1 < retries:
            time.sleep(retry_backoff)
    if prom_value is None:
        return CheckResult(
            name=name,
            prom_value=None,
            truth_value=None,
            ok=False,
            reason=reason or "prometheus returned no value",
        )
    if truth_value is None:
        return CheckResult(
            name=name,
            prom_value=prom_value,
            truth_value=None,
            ok=False,
            reason=reason or "truth source returned no value",
        )

    scale = float(check["truth"].get("scale", 1.0))
    truth_scaled = truth_value * scale

    if check.get("exact"):
        # Allow a tiny float tolerance — TOML lookups can return ints or
        # decimals depending on the value, and ``int(10) != float(10.0)``
        # would fail otherwise without any real semantic difference.
        if abs(prom_value - truth_scaled) > 1e-9:
            return CheckResult(
                name=name,
                prom_value=prom_value,
                truth_value=truth_scaled,
                ok=False,
                reason=f"exact mismatch: prom={prom_value:g} truth={truth_scaled:g}",
            )
        return CheckResult(
            name=name, prom_value=prom_value, truth_value=truth_scaled, ok=True
        )

    if prom_value <= 0 or truth_scaled <= 0:
        return CheckResult(
            name=name,
            prom_value=prom_value,
            truth_value=truth_scaled,
            ok=False,
            reason=f"non-positive value: prom={prom_value:g} truth={truth_scaled:g}",
        )

    ratio = float(check.get("ratio", defaults["ratio"]))
    actual_ratio = max(prom_value, truth_scaled) / min(prom_value, truth_scaled)
    if actual_ratio > ratio:
        return CheckResult(
            name=name,
            prom_value=prom_value,
            truth_value=truth_scaled,
            ok=False,
            reason=(
                f"ratio {actual_ratio:.2f}× exceeds limit {ratio:.1f}× "
                f"(prom={prom_value:g}, truth={truth_scaled:g})"
            ),
        )
    return CheckResult(
        name=name, prom_value=prom_value, truth_value=truth_scaled, ok=True
    )


def render_report(results: list[CheckResult]) -> str:
    lines: list[str] = []
    failed = [r for r in results if not r.ok]
    lines.append("=== Ground-truth correlation ===")
    lines.append(f"  total:  {len(results)}")
    lines.append(f"  passed: {len(results) - len(failed)}")
    lines.append(f"  failed: {len(failed)}")
    lines.append("")
    width = max((len(r.name) for r in results), default=0)
    for r in results:
        status = "OK" if r.ok else "FAIL"
        prom = "n/a" if r.prom_value is None else f"{r.prom_value:.4g}"
        truth = "n/a" if r.truth_value is None else f"{r.truth_value:.4g}"
        lines.append(
            f"  [{status:4}] {r.name:<{width}}  prom={prom:>12}  truth={truth:>12}  {r.reason}"
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prometheus-url", default="http://localhost:19090")
    parser.add_argument(
        "--checks", default="scripts/dashboard-ground-truth.checks.yaml"
    )
    parser.add_argument(
        "--container",
        default=None,
        help="Override defaults.pg_doorman_container in the checks YAML.",
    )
    parser.add_argument(
        "--toml-file",
        default=None,
        help="Override defaults.toml_file in the checks YAML.",
    )
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument(
        "--retries",
        type=int,
        default=3,
        help="Snapshot retries when prom or truth is still non-positive.",
    )
    parser.add_argument(
        "--retry-backoff",
        type=float,
        default=30.0,
        help="Sleep between retries in seconds.",
    )
    args = parser.parse_args()

    checks_path = Path(args.checks)
    if not checks_path.is_file():
        print(f"checks YAML not found: {checks_path}", file=sys.stderr)
        return 2
    with checks_path.open(encoding="utf-8") as f:
        config = yaml.safe_load(f) or {}

    defaults = config.get("defaults") or {}
    defaults.setdefault("ratio", 10.0)
    defaults.setdefault("pg_doorman_container", "demo-pg_doorman-1")
    defaults.setdefault("postgres_dsn", {})
    defaults.setdefault("toml_file", "grafana/demo/pg_doorman.toml")
    if args.container:
        defaults["pg_doorman_container"] = args.container
    if args.toml_file:
        defaults["toml_file"] = args.toml_file
    checks = config.get("checks") or []

    results = [
        evaluate(
            check,
            defaults,
            args.prometheus_url,
            args.timeout,
            retries=args.retries,
            retry_backoff=args.retry_backoff,
        )
        for check in checks
    ]
    print(render_report(results))
    return 0 if all(r.ok for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
