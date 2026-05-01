#!/usr/bin/env python3
"""Render benchmark visualisations as SVG.

Produces 4 charts for the steady-state (no SSL, no Reconnect) workloads:
  * tldr_tail_spread.svg   - p99 / p50 at 10k simple-protocol clients
  * <proto>_latency.svg    - p50 (solid) and p99 (dashed) vs concurrency

Style follows tvhahn/matplotlib-skill conventions: whitegrid + despined,
DejaVu Sans, dimgrey ticks/annotations, lightgrey legend frames. Categorical
palette taken from the diverging-colorblind ColorBrewer set so the three
poolers stay distinguishable both in colour and in marker shape.

Inputs/outputs live in plain dicts so unit tests can drive the pure helpers
without touching matplotlib.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import matplotlib.pyplot as plt
import seaborn as sns

POOLER_ORDER = ("pg_doorman", "pgbouncer", "odyssey")
COLOR = {
    "pg_doorman": "#4575b4",
    "pgbouncer":  "#d73027",
    "odyssey":    "#fc8d59",
}
MARKER = {"pg_doorman": "o", "pgbouncer": "s", "odyssey": "^"}
ACCENT_RED = "#bd0c0c"
CLIENT_TICKS = (1, 40, 120, 500, 10_000)
PROTOCOLS = ("simple", "extended", "prepared")

LEGEND_KW = dict(
    frameon=True, facecolor="white", framealpha=0.8,
    edgecolor="lightgrey", labelcolor="dimgrey",
)


# ---------- pure helpers (covered by tests) -------------------------------


def steady_state_series(groups: dict, proto: str, key: str) -> dict[str, list[tuple[int, float]]]:
    """For each pooler return [(clients, value)] for ssl=False, conn=False."""
    out: dict[str, list[tuple[int, float]]] = {p: [] for p in POOLER_ORDER}
    for (p, ssl, conn, clients), poolers in groups.items():
        if p != proto or ssl or conn:
            continue
        for pooler in POOLER_ORDER:
            v = (poolers.get(pooler) or {}).get(key)
            if v is not None:
                out[pooler].append((clients, v))
    for pooler in POOLER_ORDER:
        out[pooler].sort()
    return out


def tail_spread(groups: dict, proto: str, ssl: bool, conn: bool, clients: int) -> dict[str, float]:
    """Return {pooler: p99/p50} ratio at the requested cell."""
    cell = groups.get((proto, ssl, conn, clients)) or {}
    out: dict[str, float] = {}
    for pooler, vals in cell.items():
        p50 = (vals or {}).get("p50_ms")
        p99 = (vals or {}).get("p99_ms")
        if p50 and p99 and p50 > 0:
            out[pooler] = p99 / p50
    return out


def format_spread_label(ratio: float) -> str:
    return f"{ratio:.1f}×" if ratio < 10 else f"{ratio:.0f}×"


# ---------- shared style setup --------------------------------------------


def _setup_style() -> None:
    sns.set_theme(font_scale=1.0, style="whitegrid", font="DejaVu Sans")
    plt.rcParams["svg.fonttype"] = "none"
    plt.rcParams["pdf.fonttype"] = 42


def _finalise_axes(ax) -> None:
    sns.despine(ax=ax, left=True, bottom=True)
    ax.tick_params(axis="both", which="both", length=0, labelcolor="dimgrey")


def _save(fig, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(path, format="svg", dpi=150, bbox_inches="tight")
    plt.close(fig)


# ---------- chart functions -----------------------------------------------


def chart_tldr_tail_spread(groups: dict, out_path: Path) -> None:
    """Bar chart: p99/p50 at 10 000 simple-protocol clients."""
    _setup_style()
    spreads = tail_spread(groups, "simple", ssl=False, conn=False, clients=10_000)
    poolers = [p for p in POOLER_ORDER if p in spreads]
    values = [spreads[p] for p in poolers]
    if not poolers:
        return

    winner = min(poolers, key=lambda p: spreads[p])
    colors = [ACCENT_RED if p == winner else "lightgrey" for p in poolers]

    fig, ax = plt.subplots(figsize=(7, 4.5), dpi=150)
    bars = ax.bar(poolers, values, color=colors,
                  edgecolor="dimgrey", linewidth=0.6, width=0.55)
    ax.axhline(1.0, color="lightgrey", linestyle=":", linewidth=1, zorder=0)

    ymax = max(values)
    for bar, v in zip(bars, values):
        ax.text(bar.get_x() + bar.get_width() / 2, v + ymax * 0.025,
                format_spread_label(v),
                ha="center", va="bottom",
                color="dimgrey", weight="semibold", fontsize=12)

    ax.set_ylim(0, ymax * 1.18)
    ax.set_ylabel("p99 / p50  (lower = more predictable)",
                  fontsize=11, labelpad=8, color="dimgrey")
    ax.set_xlabel("")
    ax.set_title("Tail spread at 10,000 simple-protocol clients",
                 loc="left", pad=8, fontsize=14, color="dimgrey")

    others = [p for p in poolers if p != winner]
    if others:
        gaps = ", ".join(
            f"{p} {format_spread_label(spreads[p])}" for p in others
        )
        ax.text(0.98, 0.98,
                f"{winner} holds the median ({format_spread_label(spreads[winner])}); "
                f"competitors drift to {gaps}",
                transform=ax.transAxes, ha="right", va="top",
                fontsize=9, color="dimgrey", style="italic")

    ax.grid(False, axis="x")
    ax.grid(axis="y", alpha=0.3, linewidth=0.6)
    _finalise_axes(ax)
    _save(fig, out_path)


def chart_latency_per_protocol(groups: dict, proto: str, out_path: Path) -> None:
    """Log-log line chart: p50 (solid) and p99 (dashed) per pooler."""
    _setup_style()
    p50 = steady_state_series(groups, proto, "p50_ms")
    p99 = steady_state_series(groups, proto, "p99_ms")

    if not any(p50.values()):
        return

    fig, ax = plt.subplots(figsize=(9, 5.2), dpi=150)

    for pooler in POOLER_ORDER:
        pts50 = p50.get(pooler) or []
        pts99 = p99.get(pooler) or []
        if pts50:
            xs, ys = zip(*pts50)
            ax.plot(xs, ys, color=COLOR[pooler], marker=MARKER[pooler],
                    linestyle="-", linewidth=2.2, markersize=7,
                    label=f"{pooler} p50")
        if pts99:
            xs, ys = zip(*pts99)
            ax.plot(xs, ys, color=COLOR[pooler], marker=MARKER[pooler],
                    linestyle="--", linewidth=1.6, markersize=5,
                    alpha=0.75, label=f"{pooler} p99")
        if pts50 and pts99 and [x for x, _ in pts50] == [x for x, _ in pts99]:
            xs = [x for x, _ in pts50]
            ax.fill_between(xs,
                            [y for _, y in pts50],
                            [y for _, y in pts99],
                            color=COLOR[pooler], alpha=0.08, linewidth=0)

    ax.set_xscale("log")
    ax.set_yscale("log")
    ax.set_xticks(CLIENT_TICKS)
    ax.set_xticklabels([f"{c:,}" for c in CLIENT_TICKS])
    ax.set_xlabel("Concurrent pgbench clients (log scale)",
                  fontsize=11, labelpad=8, color="dimgrey")
    ax.set_ylabel("Per-transaction latency, ms (log scale)",
                  fontsize=11, labelpad=8, color="dimgrey")
    ax.set_title(f"{proto.capitalize()} protocol: latency p50 (solid) and p99 (dashed)",
                 loc="left", pad=8, fontsize=14, color="dimgrey")

    headline = _latency_headline(p50, p99)
    if headline:
        ax.text(0.98, 0.02, headline,
                transform=ax.transAxes, ha="right", va="bottom",
                fontsize=9, color="dimgrey", style="italic")

    ax.legend(loc="upper left", ncol=2, fontsize=9, **LEGEND_KW)
    ax.grid(True, which="major", alpha=0.35, linewidth=0.6)
    ax.grid(True, which="minor", alpha=0.15, linewidth=0.4)
    _finalise_axes(ax)
    _save(fig, out_path)


def _latency_headline(p50_series: dict, p99_series: dict) -> str | None:
    """One-line takeaway built from the 10k-client column when available."""
    target = 10_000
    cells: list[tuple[str, float, float]] = []
    for pooler in POOLER_ORDER:
        s50 = dict(p50_series.get(pooler) or [])
        s99 = dict(p99_series.get(pooler) or [])
        if target in s50 and target in s99:
            cells.append((pooler, s50[target], s99[target]))
    if not cells:
        return None
    pieces = [
        f"{pooler} {p50:.0f}/{p99:.0f}ms"
        if p50 >= 1 else f"{pooler} {p50:.2f}/{p99:.2f}ms"
        for pooler, p50, p99 in cells
    ]
    return f"At {target:,} clients (p50/p99): " + ", ".join(pieces)


# ---------- top-level orchestration ---------------------------------------


def render_all(groups: dict, images_dir: Path) -> list[str]:
    """Generate every chart we currently know how to draw. Returns the list of
    relative filenames produced (so the markdown step can build ![]() links)."""
    rendered: list[str] = []

    name = "tldr_tail_spread.svg"
    chart_tldr_tail_spread(groups, images_dir / name)
    if (images_dir / name).exists():
        rendered.append(name)

    for proto in PROTOCOLS:
        name = f"{proto}_latency.svg"
        chart_latency_per_protocol(groups, proto, images_dir / name)
        if (images_dir / name).exists():
            rendered.append(name)

    return rendered


def _decode_groups(raw: dict) -> dict:
    """JSON keys arrive as strings ('simple|False|False|10000'); rebuild tuples."""
    out: dict = {}
    for k, v in raw.items():
        proto, ssl, conn, clients = k.split("|")
        out[(proto, ssl == "True", conn == "True", int(clients))] = v
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("groups_json",
                    help="JSON file with {'<proto>|<ssl>|<conn>|<clients>': {<pooler>: {p50_ms, p99_ms, ...}}}")
    ap.add_argument("images_dir", help="Output directory for SVG files")
    args = ap.parse_args()

    groups = _decode_groups(json.loads(Path(args.groups_json).read_text()))
    rendered = render_all(groups, Path(args.images_dir))
    for name in rendered:
        print(f"wrote {name}")


if __name__ == "__main__":
    main()
