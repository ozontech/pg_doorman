"""Unit tests for benches/parse-bench-logs.py.

Run: python3 -m unittest benches.test_parse_bench_logs -v
or: python3 benches/test_parse_bench_logs.py
"""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

# parse-bench-logs.py uses a hyphen so we can't `import` it directly; load by path.
_SPEC = importlib.util.spec_from_file_location(
    "parse_bench_logs", Path(__file__).parent / "parse-bench-logs.py"
)
P = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(P)


class PercentileMs(unittest.TestCase):
    def test_empty_returns_none(self):
        self.assertIsNone(P.percentile_ms([], 0.5))

    def test_single_value(self):
        self.assertEqual(P.percentile_ms([1000.0], 0.5), 1.0)
        self.assertEqual(P.percentile_ms([1000.0], 0.99), 1.0)

    def test_two_values(self):
        # n=2, p50: idx = ceil(2*0.5)-1 = 0 → first
        self.assertEqual(P.percentile_ms([1000.0, 2000.0], 0.5), 1.0)
        # n=2, p95: idx = ceil(2*0.95)-1 = 1 → second
        self.assertEqual(P.percentile_ms([1000.0, 2000.0], 0.95), 2.0)

    def test_full_range(self):
        # 1..100 µs → 0.001..0.100 ms
        data = [float(i) for i in range(1, 101)]
        # p50: idx = ceil(100*0.5)-1 = 49 → 50 µs → 0.050 ms
        self.assertAlmostEqual(P.percentile_ms(data, 0.50), 0.050)
        # p99: idx = ceil(100*0.99)-1 = 98 → 99 µs → 0.099 ms
        self.assertAlmostEqual(P.percentile_ms(data, 0.99), 0.099)
        # p95: idx = ceil(100*0.95)-1 = 94 → 95 µs → 0.095 ms
        self.assertAlmostEqual(P.percentile_ms(data, 0.95), 0.095)

    def test_index_clamped_at_top(self):
        # very small input, very high frac — idx may overshoot before clamp.
        self.assertEqual(P.percentile_ms([10.0], 0.999), 0.010)


class ParseTestName(unittest.TestCase):
    def test_simple_no_modifiers(self):
        self.assertEqual(
            P.parse_test_name("pg_doorman_simple_c1"),
            {"pooler": "pg_doorman", "ssl": False, "proto": "simple",
             "connect": False, "clients": 1},
        )

    def test_simple_with_connect(self):
        self.assertEqual(
            P.parse_test_name("odyssey_simple_connect_c40"),
            {"pooler": "odyssey", "ssl": False, "proto": "simple",
             "connect": True, "clients": 40},
        )

    def test_ssl_with_proto(self):
        self.assertEqual(
            P.parse_test_name("pgbouncer_ssl_extended_c500"),
            {"pooler": "pgbouncer", "ssl": True, "proto": "extended",
             "connect": False, "clients": 500},
        )

    def test_ssl_connect_no_explicit_proto_defaults_to_simple(self):
        # bench.feature emits these for SSL+--connect (always --protocol=simple).
        self.assertEqual(
            P.parse_test_name("pg_doorman_ssl_connect_c1"),
            {"pooler": "pg_doorman", "ssl": True, "proto": "simple",
             "connect": True, "clients": 1},
        )

    def test_prepared(self):
        self.assertEqual(
            P.parse_test_name("pg_doorman_prepared_c10000")["proto"],
            "prepared",
        )

    def test_garbage_returns_none(self):
        self.assertIsNone(P.parse_test_name("garbage"))
        self.assertIsNone(P.parse_test_name("doorman"))  # service log stem
        self.assertIsNone(P.parse_test_name("pg_doorman_unknown_c1"))


class ParsePgbenchStdout(unittest.TestCase):
    def test_full_output(self):
        text = (
            "transaction type: /tmp/pgbench.sql\n"
            "scaling factor: 1\n"
            "tps = 1234.567 (without initial connection time)\n"
            "latency average = 5.6 ms\n"
        )
        self.assertEqual(
            P.parse_pgbench_stdout(text),
            {"tps": 1234.567, "lat_avg_ms": 5.6},
        )

    def test_empty(self):
        self.assertEqual(
            P.parse_pgbench_stdout(""),
            {"tps": None, "lat_avg_ms": None},
        )

    def test_only_tps(self):
        self.assertEqual(
            P.parse_pgbench_stdout("tps = 100"),
            {"tps": 100.0, "lat_avg_ms": None},
        )


class FormatThroughput(unittest.TestCase):
    def test_within_three_percent(self):
        self.assertEqual(P.format_throughput(102, 100), "≈0%")
        self.assertEqual(P.format_throughput(98, 100), "≈0%")

    def test_positive_percent(self):
        self.assertEqual(P.format_throughput(110, 100), "+10%")
        self.assertEqual(P.format_throughput(149, 100), "+49%")

    def test_negative_percent(self):
        self.assertEqual(P.format_throughput(90, 100), "-10%")

    def test_ratio_at_threshold(self):
        # ratio == 1.5 switches to xN.N form
        self.assertEqual(P.format_throughput(150, 100), "x1.5")
        self.assertEqual(P.format_throughput(260, 100), "x2.6")
        self.assertEqual(P.format_throughput(900, 100), "x9.0")

    def test_competitor_zero_tps(self):
        self.assertEqual(P.format_throughput(100, 0), "∞")
        self.assertEqual(P.format_throughput(0, 0), "-")

    def test_missing_data(self):
        self.assertEqual(P.format_throughput(None, None), "-")
        self.assertEqual(P.format_throughput(100, None), "N/A")
        self.assertEqual(P.format_throughput(None, 100), "N/A")


class FormatLatencyTriplet(unittest.TestCase):
    def test_full(self):
        rec = {"p50_ms": 0.07, "p95_ms": 0.07, "p99_ms": 0.08}
        self.assertEqual(P.format_latency_triplet(rec), "0.07 / 0.07 / 0.08")

    def test_two_decimal_rounding(self):
        rec = {"p50_ms": 1.234, "p95_ms": 5.678, "p99_ms": 10.0}
        self.assertEqual(P.format_latency_triplet(rec), "1.23 / 5.68 / 10.00")

    def test_missing_percentile_returns_dash(self):
        self.assertEqual(
            P.format_latency_triplet({"p50_ms": 1.0, "p95_ms": None, "p99_ms": 3.0}),
            "-",
        )

    def test_empty_dict(self):
        self.assertEqual(P.format_latency_triplet({}), "-")
        self.assertEqual(P.format_latency_triplet(None), "-")


class FormatMs(unittest.TestCase):
    def test_adaptive_precision(self):
        # <10 → 2 decimals, <100 → 1, else 0
        self.assertEqual(P.format_ms({"p99_ms": 0.08}, "p99_ms"), "0.08")
        self.assertEqual(P.format_ms({"p99_ms": 9.99}, "p99_ms"), "9.99")
        self.assertEqual(P.format_ms({"p99_ms": 44.3}, "p99_ms"), "44.3")
        self.assertEqual(P.format_ms({"p99_ms": 99.4}, "p99_ms"), "99.4")
        self.assertEqual(P.format_ms({"p99_ms": 286.4}, "p99_ms"), "286")
        self.assertEqual(P.format_ms({"p99_ms": 1500.7}, "p99_ms"), "1501")

    def test_missing(self):
        self.assertEqual(P.format_ms(None, "p99_ms"), "-")
        self.assertEqual(P.format_ms({}, "p99_ms"), "-")
        self.assertEqual(P.format_ms({"p99_ms": None}, "p99_ms"), "-")
        self.assertEqual(P.format_ms({"p50_ms": 1.0}, "p99_ms"), "-")


class FormatP50P99(unittest.TestCase):
    def test_pair(self):
        rec = {"p50_ms": 3.54, "p95_ms": 4.68, "p99_ms": 6.46}
        self.assertEqual(P.format_p50_p99(rec), "3.54 / 6.46")

    def test_mixed_precision(self):
        rec = {"p50_ms": 69.42, "p99_ms": 75.83}
        self.assertEqual(P.format_p50_p99(rec), "69.4 / 75.8")

    def test_missing(self):
        self.assertEqual(P.format_p50_p99(None), "-")
        self.assertEqual(P.format_p50_p99({"p50_ms": 1.0}), "-")
        self.assertEqual(P.format_p50_p99({"p99_ms": 1.0}), "-")


class FormatSpread(unittest.TestCase):
    def test_steady(self):
        # pg_doorman at 10k simple: p50=128, p99=150 → 1.17 → "1.2×"
        self.assertEqual(P.format_spread({"p50_ms": 128, "p99_ms": 150}), "1.2×")

    def test_long_tail(self):
        # odyssey at 10k simple: p50=2.7, p99=823 → 304.8 → "305×"
        self.assertEqual(P.format_spread({"p50_ms": 2.7, "p99_ms": 823}), "305×")

    def test_under_10_uses_one_decimal(self):
        self.assertEqual(P.format_spread({"p50_ms": 1.0, "p99_ms": 1.8}), "1.8×")

    def test_zero_or_missing_p50(self):
        self.assertEqual(P.format_spread({"p50_ms": 0, "p99_ms": 1.0}), "-")
        self.assertEqual(P.format_spread({"p99_ms": 1.0}), "-")
        self.assertEqual(P.format_spread(None), "-")


class ModeAndRowLabels(unittest.TestCase):
    def test_mode_label(self):
        self.assertEqual(P.mode_label(False, False), "")
        self.assertEqual(P.mode_label(True, False), "SSL")
        self.assertEqual(P.mode_label(False, True), "Reconnect")
        self.assertEqual(P.mode_label(True, True), "SSL + Reconnect")

    def test_row_label_singular_and_plural(self):
        self.assertEqual(P.row_label(1, ""), "1 client")
        self.assertEqual(P.row_label(40, ""), "40 clients")

    def test_row_label_thousands_grouping(self):
        self.assertEqual(P.row_label(10000, ""), "10,000 clients")

    def test_row_label_with_mode(self):
        self.assertEqual(P.row_label(120, "Reconnect"), "120 clients + Reconnect")
        self.assertEqual(
            P.row_label(500, "SSL + Reconnect"),
            "500 clients + SSL + Reconnect",
        )


class FormatDuration(unittest.TestCase):
    def test_seconds(self):
        self.assertEqual(P.format_duration(45), "45s")

    def test_minutes(self):
        self.assertEqual(P.format_duration(90), "1m 30s")

    def test_hours(self):
        self.assertEqual(P.format_duration(3661), "1h 01m 01s")


class ParseIso8601Z(unittest.TestCase):
    def test_zulu(self):
        dt = P.parse_iso8601_z("2026-04-27T05:14:44Z")
        self.assertIsNotNone(dt)
        self.assertEqual(dt.year, 2026)

    def test_none(self):
        self.assertIsNone(P.parse_iso8601_z(None))
        self.assertIsNone(P.parse_iso8601_z("not-a-date"))


class ComputeTldr(unittest.TestCase):
    def test_speedup_picks_largest_ratio(self):
        # pg_doorman 5x pgbouncer at 500 clients simple, 2x at 40
        groups = {
            ("simple", False, False, 40): {
                "pg_doorman": {"tps": 200, "p50_ms": 1, "p95_ms": 2, "p99_ms": 3},
                "pgbouncer": {"tps": 100, "p50_ms": 2, "p95_ms": 4, "p99_ms": 6},
                "odyssey": {"tps": 180, "p50_ms": 1, "p95_ms": 2, "p99_ms": 4},
            },
            ("simple", False, False, 500): {
                "pg_doorman": {"tps": 500, "p50_ms": 3, "p95_ms": 5, "p99_ms": 8},
                "pgbouncer": {"tps": 100, "p50_ms": 8, "p95_ms": 15, "p99_ms": 25},
                "odyssey": {"tps": 480, "p50_ms": 3, "p95_ms": 5, "p99_ms": 9},
            },
        }
        bullets = P.compute_tldr(groups)
        self.assertTrue(any("x5.0" in b and "vs pgbouncer" in b for b in bullets))

    def test_empty_groups_returns_empty(self):
        self.assertEqual(P.compute_tldr({}), [])

    def test_skips_ssl_and_connect(self):
        # Only SSL+Reconnect data — should produce no speedup bullets.
        groups = {
            ("simple", True, True, 40): {
                "pg_doorman": {"tps": 100, "p50_ms": 1, "p95_ms": 2, "p99_ms": 3},
                "pgbouncer": {"tps": 50, "p50_ms": 2, "p95_ms": 4, "p99_ms": 6},
            },
        }
        bullets = P.compute_tldr(groups)
        # No vs-pgbouncer headline because steady-state filter excludes ssl/connect.
        self.assertFalse(any("vs pgbouncer" in b for b in bullets))


class ServiceLogFiltering(unittest.TestCase):
    def test_bench_wrap_in_blocklist(self):
        self.assertIn("bench-wrap", P.SERVICE_LOG_NAMES)

    def test_pooler_logs_in_blocklist(self):
        for name in ("doorman", "odyssey", "pgbouncer", "pg"):
            self.assertIn(name, P.SERVICE_LOG_NAMES)


class ParseTestPercentilesSource(unittest.TestCase):
    """parse_test prefers <name>_percentiles.json (written on the bench host
    by compute_percentiles.py) over raw pgbench --log files."""

    def test_uses_percentiles_json_when_present(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "alpha.log").write_text(
                "tps = 1000\nlatency average = 5.0\n"
            )
            (d / "alpha_percentiles.json").write_text(json.dumps({
                "samples": 42, "p50_ms": 1.5, "p95_ms": 4.0, "p99_ms": 8.0,
            }))
            # A stale raw log next to the JSON must not influence the result.
            (d / "alpha_pgbenchlog.1").write_text("0 1 999999\n")
            rec = P.parse_test("alpha", d)
            self.assertEqual(rec["tps"], 1000.0)
            self.assertEqual(rec["lat_avg_ms"], 5.0)
            self.assertEqual(rec["samples"], 42)
            self.assertEqual(rec["p50_ms"], 1.5)
            self.assertEqual(rec["p95_ms"], 4.0)
            self.assertEqual(rec["p99_ms"], 8.0)

    def test_falls_back_to_raw_logs_when_json_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "alpha.log").write_text("tps = 100\n")
            (d / "alpha_pgbenchlog.1").write_text("0 1 1000\n0 2 2000\n")
            rec = P.parse_test("alpha", d)
            self.assertEqual(rec["samples"], 2)
            # n=2, p99: idx = ceil(2*0.99)-1 = 1 → 2000 µs → 2.0 ms
            self.assertEqual(rec["p99_ms"], 2.0)

    def test_no_data_at_all_yields_none_percentiles(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "alpha.log").write_text("tps = 0\n")
            rec = P.parse_test("alpha", d)
            self.assertIsNone(rec["p50_ms"])
            self.assertEqual(rec["samples"], 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
