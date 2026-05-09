#!/usr/bin/env python3
"""Slot-1 smoke test for grafana/pg_doorman.json.

Walks the dashboard, substitutes the dashboard's template variables
($instance / $user / $database / $__rate_interval) into every panel
target, queries Prometheus, and checks that the result vector is not
empty. In parallel runs the targeted-bounds checks from
scripts/dashboard-smoke.expected.yaml.

Exit codes:
    0 — every panel returned data and every bound was within [min, max];
    1 — at least one panel returned an empty vector, or a bound
        violated the configured range.

Run from the repository root:
    python3 scripts/dashboard-smoke.py
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml


VAR_PATTERN = re.compile(r"\$\{?([_a-zA-Z][_a-zA-Z0-9]*)\}?")


@dataclass
class Variables:
    """Concrete values for the dashboard's template variables."""

    instance: str
    user: str
    database: str
    rate_interval: str
    interval: str
    time_range: str

    def as_dict(self) -> dict[str, str]:
        return {
            "instance": self.instance,
            "user": self.user,
            "database": self.database,
            "__rate_interval": self.rate_interval,
            "__interval": self.interval,
            "__range": self.time_range,
        }


@dataclass
class Bound:
    """A targeted bounds check from expected.yaml."""

    name: str
    query: str
    min_value: float
    max_value: float


@dataclass
class TargetCheck:
    """Result of one panel target check."""

    panel: str
    expr_raw: str
    expr_resolved: str
    ok: bool
    reason: str = ""
    allowed_empty: bool = False


@dataclass
class BoundCheck:
    """Result of one targeted bounds check."""

    name: str
    query: str
    ok: bool
    reason: str = ""
    actual: float | None = None


@dataclass
class Report:
    targets: list[TargetCheck] = field(default_factory=list)
    bounds: list[BoundCheck] = field(default_factory=list)

    @property
    def failed(self) -> bool:
        return any(not t.ok for t in self.targets) or any(
            not b.ok for b in self.bounds
        )


def substitute_variables(expr: str, vars_: Variables) -> str:
    """Replace $name / ${name} in a PromQL expression with concrete values.

    Unknown variables are left as-is — that is not a smoke-test failure
    on its own, but a hint that expected.yaml does not yet describe a
    new variable. Prometheus will then return an error and the per-target
    check will fail with a clear reason.
    """
    mapping = vars_.as_dict()
    return VAR_PATTERN.sub(
        lambda m: mapping.get(m.group(1), m.group(0)), expr
    )


def iter_panels(panels: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Flatten Grafana row nesting into a single panel list."""
    out: list[dict[str, Any]] = []
    for panel in panels:
        if panel.get("type") == "row":
            nested = panel.get("panels", [])
            if nested:
                out.extend(iter_panels(nested))
            continue
        out.append(panel)
    return out


def collect_panel_targets(
    dashboard: dict[str, Any],
) -> list[tuple[str, str]]:
    """Collect (panel_title, expr) pairs for every target in the dashboard.

    Titles are not unique in pg_doorman.json — at minimum "Waiting Clients"
    appears twice. The smoke test still keys on the title because that
    is what the operator reads in Grafana, but ``validate_allow_empty``
    refuses to start when allow_empty references an ambiguous title,
    so a future duplicate cannot silently widen the amnesty list.
    """
    pairs: list[tuple[str, str]] = []
    for panel in iter_panels(dashboard.get("panels", [])):
        title = panel.get("title", "<untitled>")
        for target in panel.get("targets", []):
            expr = target.get("expr")
            if not expr:
                continue
            pairs.append((title, expr))
    return pairs


def validate_allow_empty(
    dashboard: dict[str, Any], allow_empty: set[str]
) -> list[str]:
    """Check every allow_empty entry resolves to a single panel.

    Returns a list of error strings: missing titles or duplicated ones.
    Run before the actual checks so the operator sees the configuration
    error at the top of the report instead of after 60+ panel results.
    """
    title_counts: dict[str, int] = {}
    for panel in iter_panels(dashboard.get("panels", [])):
        title = panel.get("title", "<untitled>")
        title_counts[title] = title_counts.get(title, 0) + 1
    errors: list[str] = []
    for title in allow_empty:
        count = title_counts.get(title, 0)
        if count == 0:
            errors.append(f"allow_empty: title not found in dashboard: {title!r}")
        elif count > 1:
            errors.append(
                f"allow_empty: title is ambiguous "
                f"({count} panels): {title!r}"
            )
    return errors


def prometheus_query(base_url: str, query: str, timeout: float) -> dict[str, Any]:
    """Run an instant PromQL query and return the parsed Prometheus response."""
    url = f"{base_url.rstrip('/')}/api/v1/query?{urllib.parse.urlencode({'query': query})}"
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def first_value(result: list[dict[str, Any]]) -> float | None:
    """Pull the first scalar value out of a Prometheus vector response."""
    if not result:
        return None
    value = result[0].get("value")
    if not value or len(value) < 2:
        return None
    try:
        return float(value[1])
    except (TypeError, ValueError):
        return None


def check_target(
    panel: str,
    expr: str,
    vars_: Variables,
    prom_url: str,
    timeout: float,
    allow_empty: set[str],
) -> TargetCheck:
    resolved = substitute_variables(expr, vars_)
    allowed_empty = panel in allow_empty
    try:
        response = prometheus_query(prom_url, resolved, timeout)
    except urllib.error.HTTPError as e:
        return TargetCheck(
            panel=panel,
            expr_raw=expr,
            expr_resolved=resolved,
            ok=False,
            reason=f"http {e.code}: {e.reason}",
        )
    except urllib.error.URLError as e:
        return TargetCheck(
            panel=panel,
            expr_raw=expr,
            expr_resolved=resolved,
            ok=False,
            reason=f"url error: {e.reason}",
        )
    if response.get("status") != "success":
        error = response.get("error") or response.get("errorType") or "unknown"
        return TargetCheck(
            panel=panel,
            expr_raw=expr,
            expr_resolved=resolved,
            ok=False,
            reason=f"prom error: {error}",
        )
    result = response.get("data", {}).get("result", [])
    if not result:
        return TargetCheck(
            panel=panel,
            expr_raw=expr,
            expr_resolved=resolved,
            ok=allowed_empty,
            allowed_empty=allowed_empty,
            reason="empty vector (allowed)" if allowed_empty else "empty vector",
        )
    return TargetCheck(panel=panel, expr_raw=expr, expr_resolved=resolved, ok=True)


def check_bound(bound: Bound, prom_url: str, timeout: float) -> BoundCheck:
    try:
        response = prometheus_query(prom_url, bound.query, timeout)
    except (urllib.error.HTTPError, urllib.error.URLError) as e:
        return BoundCheck(
            name=bound.name, query=bound.query, ok=False, reason=f"network: {e}"
        )
    if response.get("status") != "success":
        error = response.get("error") or response.get("errorType") or "unknown"
        return BoundCheck(
            name=bound.name, query=bound.query, ok=False, reason=f"prom error: {error}"
        )
    actual = first_value(response.get("data", {}).get("result", []))
    if actual is None:
        return BoundCheck(
            name=bound.name, query=bound.query, ok=False, reason="empty vector"
        )
    if actual < bound.min_value or actual > bound.max_value:
        return BoundCheck(
            name=bound.name,
            query=bound.query,
            ok=False,
            actual=actual,
            reason=f"value {actual:g} out of [{bound.min_value:g}, {bound.max_value:g}]",
        )
    return BoundCheck(name=bound.name, query=bound.query, ok=True, actual=actual)


def parse_expected(
    path: Path,
) -> tuple[Variables, list[Bound], set[str]]:
    with path.open(encoding="utf-8") as f:
        data = yaml.safe_load(f) or {}
    raw_vars = data.get("variables", {})
    vars_ = Variables(
        instance=str(raw_vars.get("instance", "pg_doorman:9127")),
        user=str(raw_vars.get("user", ".+")),
        database=str(raw_vars.get("database", ".+")),
        rate_interval=str(raw_vars.get("__rate_interval", "1m")),
        interval=str(raw_vars.get("__interval", "30s")),
        time_range=str(raw_vars.get("__range", "30m")),
    )
    bounds: list[Bound] = []
    for entry in data.get("bounds", []) or []:
        bounds.append(
            Bound(
                name=str(entry["name"]),
                query=str(entry["query"]),
                min_value=float(entry["min"]),
                max_value=float(entry["max"]),
            )
        )
    allow_empty = {str(p) for p in data.get("allow_empty", []) or []}
    return vars_, bounds, allow_empty


def render_report(report: Report) -> str:
    lines: list[str] = []
    failed_targets = [t for t in report.targets if not t.ok]
    allowed_empty_targets = [t for t in report.targets if t.ok and t.allowed_empty]
    failed_bounds = [b for b in report.bounds if not b.ok]
    lines.append("=== Panel target results ===")
    lines.append(f"  total:        {len(report.targets)}")
    passed_with_data = (
        len(report.targets) - len(failed_targets) - len(allowed_empty_targets)
    )
    lines.append(f"  passed:       {passed_with_data}")
    lines.append(f"  allow_empty:  {len(allowed_empty_targets)}")
    lines.append(f"  failed:       {len(failed_targets)}")
    if failed_targets:
        lines.append("")
        lines.append("Failed targets:")
        for t in failed_targets:
            lines.append(f"  [{t.panel}] {t.reason}")
            lines.append(f"    raw:      {t.expr_raw}")
            lines.append(f"    resolved: {t.expr_resolved}")
    lines.append("")
    lines.append("=== Targeted bounds ===")
    lines.append(f"  total:  {len(report.bounds)}")
    lines.append(f"  passed: {len(report.bounds) - len(failed_bounds)}")
    lines.append(f"  failed: {len(failed_bounds)}")
    if failed_bounds:
        lines.append("")
        lines.append("Failed bounds:")
        for b in failed_bounds:
            lines.append(f"  [{b.name}] {b.reason}")
            lines.append(f"    query: {b.query}")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prometheus-url",
        default="http://localhost:19090",
        help="Prometheus base URL (default %(default)s — Grafana demo).",
    )
    parser.add_argument(
        "--dashboard-json",
        default="grafana/pg_doorman.json",
        help="Path to the Grafana dashboard JSON.",
    )
    parser.add_argument(
        "--expected",
        default="scripts/dashboard-smoke.expected.yaml",
        help="Path to the YAML with variables, bounds, and allow_empty.",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=10.0,
        help="HTTP timeout for each Prometheus query.",
    )
    args = parser.parse_args()

    dashboard_path = Path(args.dashboard_json)
    if not dashboard_path.is_file():
        print(f"dashboard JSON not found: {dashboard_path}", file=sys.stderr)
        return 2
    expected_path = Path(args.expected)
    if not expected_path.is_file():
        print(f"expected YAML not found: {expected_path}", file=sys.stderr)
        return 2

    with dashboard_path.open(encoding="utf-8") as f:
        dashboard = json.load(f)
    vars_, bounds, allow_empty = parse_expected(expected_path)

    config_errors = validate_allow_empty(dashboard, allow_empty)
    if config_errors:
        for err in config_errors:
            print(err, file=sys.stderr)
        return 2

    report = Report()
    for panel, expr in collect_panel_targets(dashboard):
        report.targets.append(
            check_target(
                panel, expr, vars_, args.prometheus_url, args.timeout, allow_empty
            )
        )
    for bound in bounds:
        report.bounds.append(check_bound(bound, args.prometheus_url, args.timeout))

    print(render_report(report))
    return 1 if report.failed else 0


if __name__ == "__main__":
    sys.exit(main())
