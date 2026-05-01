"""Unit tests for benches/render_charts.py pure helpers.

Run: python3 -m unittest benches.test_render_charts -v
or:  python3 benches/test_render_charts.py

The render functions themselves call matplotlib and are exercised end-to-end
when the bench workflow runs. These tests cover only the pure data-massaging
helpers so the suite stays runnable without a matplotlib install.
"""

import importlib.util
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "render_charts", Path(__file__).parent / "render_charts.py"
)
# render_charts imports matplotlib at module top, so skip the whole module if
# matplotlib is not available. The pure helpers don't need matplotlib at call
# time, but the import does.
try:
    R = importlib.util.module_from_spec(_SPEC)
    _SPEC.loader.exec_module(R)
    HAVE_RENDER = True
except ModuleNotFoundError as exc:
    HAVE_RENDER = False
    SKIP_REASON = f"render_charts unavailable: {exc.name}"


def _cell(p50, p99):
    return {"p50_ms": p50, "p99_ms": p99}


@unittest.skipUnless(HAVE_RENDER, "matplotlib/seaborn not installed")
class SteadyStateSeries(unittest.TestCase):
    def test_empty_groups_returns_empty_lists_per_pooler(self):
        out = R.steady_state_series({}, "simple", "p50_ms")
        self.assertEqual(out, {"pg_doorman": [], "pgbouncer": [], "odyssey": []})

    def test_filters_out_ssl_and_reconnect_rows(self):
        groups = {
            ("simple", False, False, 40):  {"pg_doorman": _cell(0.3, 0.5)},
            ("simple", True,  False, 40):  {"pg_doorman": _cell(9.9, 9.9)},
            ("simple", False, True,  40):  {"pg_doorman": _cell(9.9, 9.9)},
            ("simple", True,  True,  40):  {"pg_doorman": _cell(9.9, 9.9)},
        }
        out = R.steady_state_series(groups, "simple", "p50_ms")
        self.assertEqual(out["pg_doorman"], [(40, 0.3)])

    def test_filters_out_other_protocols(self):
        groups = {
            ("simple",   False, False, 40): {"pg_doorman": _cell(0.3, 0.5)},
            ("extended", False, False, 40): {"pg_doorman": _cell(9.9, 9.9)},
        }
        out = R.steady_state_series(groups, "simple", "p50_ms")
        self.assertEqual(out["pg_doorman"], [(40, 0.3)])

    def test_sorts_by_client_count_ascending(self):
        groups = {
            ("simple", False, False, c): {"pg_doorman": _cell(c * 0.01, c * 0.02)}
            for c in (10000, 1, 500, 40, 120)
        }
        out = R.steady_state_series(groups, "simple", "p50_ms")
        clients = [c for c, _ in out["pg_doorman"]]
        self.assertEqual(clients, [1, 40, 120, 500, 10000])

    def test_skips_pooler_with_missing_metric(self):
        groups = {
            ("simple", False, False, 40): {
                "pg_doorman": _cell(0.3, 0.5),
                "pgbouncer":  {"p99_ms": 1.9},  # no p50
                "odyssey":    None,
            },
        }
        out = R.steady_state_series(groups, "simple", "p50_ms")
        self.assertEqual(out["pg_doorman"], [(40, 0.3)])
        self.assertEqual(out["pgbouncer"], [])
        self.assertEqual(out["odyssey"], [])


@unittest.skipUnless(HAVE_RENDER, "matplotlib/seaborn not installed")
class TailSpread(unittest.TestCase):
    def test_returns_p99_over_p50(self):
        groups = {
            ("simple", False, False, 10000): {
                "pg_doorman": _cell(60.0, 65.0),
                "pgbouncer":  _cell(280.0, 390.0),
            },
        }
        out = R.tail_spread(groups, "simple", False, False, 10000)
        self.assertAlmostEqual(out["pg_doorman"], 65.0 / 60.0)
        self.assertAlmostEqual(out["pgbouncer"],  390.0 / 280.0)

    def test_missing_cell_returns_empty(self):
        self.assertEqual(R.tail_spread({}, "simple", False, False, 10000), {})

    def test_skips_pooler_with_zero_p50(self):
        groups = {
            ("simple", False, False, 1): {
                "pg_doorman": _cell(0.0, 0.1),  # division guard must trigger
                "pgbouncer":  _cell(0.07, 0.10),
            },
        }
        out = R.tail_spread(groups, "simple", False, False, 1)
        self.assertNotIn("pg_doorman", out)
        self.assertIn("pgbouncer", out)

    def test_skips_pooler_missing_either_percentile(self):
        groups = {
            ("simple", False, False, 1): {
                "a": {"p50_ms": 0.07},
                "b": {"p99_ms": 0.10},
                "c": _cell(0.07, 0.10),
            },
        }
        out = R.tail_spread(groups, "simple", False, False, 1)
        self.assertEqual(set(out.keys()), {"c"})


@unittest.skipUnless(HAVE_RENDER, "matplotlib/seaborn not installed")
class FormatSpreadLabel(unittest.TestCase):
    def test_below_ten_one_decimal(self):
        self.assertEqual(R.format_spread_label(1.083), "1.1×")
        self.assertEqual(R.format_spread_label(1.4),   "1.4×")
        self.assertEqual(R.format_spread_label(9.94),  "9.9×")

    def test_at_and_above_ten_no_decimals(self):
        self.assertEqual(R.format_spread_label(10.0),  "10×")
        self.assertEqual(R.format_spread_label(11.39), "11×")
        self.assertEqual(R.format_spread_label(305.0), "305×")

    def test_boundary_just_under_ten_uses_decimal_form(self):
        # 9.95 stays in the decimal branch (`< 10`); float-repr makes
        # `.1f` emit 9.9 rather than 10.0 due to IEEE 754 rounding-down.
        self.assertEqual(R.format_spread_label(9.95), "9.9×")


@unittest.skipUnless(HAVE_RENDER, "matplotlib/seaborn not installed")
class LatencyHeadline(unittest.TestCase):
    def test_returns_none_when_no_pooler_has_10k_data(self):
        p50 = {"pg_doorman": [(40, 0.3)], "pgbouncer": [], "odyssey": []}
        p99 = {"pg_doorman": [(40, 0.5)], "pgbouncer": [], "odyssey": []}
        self.assertIsNone(R._latency_headline(p50, p99))

    def test_renders_only_poolers_present_at_10k(self):
        p50 = {
            "pg_doorman": [(10000, 60.0)],
            "pgbouncer":  [(10000, 280.0)],
            "odyssey":    [(40, 0.3)],          # no 10k row
        }
        p99 = {
            "pg_doorman": [(10000, 65.0)],
            "pgbouncer":  [(10000, 390.0)],
            "odyssey":    [(40, 0.5)],
        }
        out = R._latency_headline(p50, p99)
        self.assertIn("pg_doorman 60/65ms", out)
        self.assertIn("pgbouncer 280/390ms", out)
        self.assertNotIn("odyssey", out)

    def test_sub_one_ms_uses_two_decimal_places(self):
        # Edge case: at 1 client all three poolers are well under 1ms; format
        # must switch to high-precision so the digits don't round to 0.
        p50 = {"pg_doorman": [(10000, 0.07)], "pgbouncer": [], "odyssey": []}
        p99 = {"pg_doorman": [(10000, 0.10)], "pgbouncer": [], "odyssey": []}
        out = R._latency_headline(p50, p99)
        self.assertIn("0.07/0.10ms", out)


@unittest.skipUnless(HAVE_RENDER, "matplotlib/seaborn not installed")
class DecodeGroups(unittest.TestCase):
    def test_round_trip(self):
        raw = {
            "simple|False|False|40":   {"pg_doorman": {"p50_ms": 0.3}},
            "simple|True|False|10000": {"pg_doorman": {"p50_ms": 26.9}},
            "extended|False|True|120": {"pg_doorman": {"p50_ms": 3.8}},
        }
        out = R._decode_groups(raw)
        self.assertEqual(out[("simple",   False, False, 40)],    raw["simple|False|False|40"])
        self.assertEqual(out[("simple",   True,  False, 10000)], raw["simple|True|False|10000"])
        self.assertEqual(out[("extended", False, True,  120)],   raw["extended|False|True|120"])

    def test_only_literal_True_string_decodes_to_true(self):
        # Documented contract: keys come from json.dumps of the bench-host
        # dict, so values are exactly "True" / "False". Anything else is
        # interpreted as False rather than raising; keep the renderer
        # tolerant of mangled metadata.
        raw = {"simple|true|FALSE|40": {"pg_doorman": {"p50_ms": 0.3}}}
        out = R._decode_groups(raw)
        # "true" lowercase is not "True" → ssl=False, "FALSE" → conn=False
        self.assertIn(("simple", False, False, 40), out)


if __name__ == "__main__":
    unittest.main(verbosity=2)
