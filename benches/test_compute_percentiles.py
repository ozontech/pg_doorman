"""Unit tests for benches/compute_percentiles.py.

Run: python3 -m unittest benches.test_compute_percentiles -v
or:  python3 benches/test_compute_percentiles.py
"""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "compute_percentiles", Path(__file__).parent / "compute_percentiles.py"
)
C = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(C)


class PercentileMs(unittest.TestCase):
    def test_empty_returns_none(self):
        self.assertIsNone(C.percentile_ms([], 0.5))

    def test_full_range_matches_parse_bench_logs(self):
        # 1..100 µs → 0.001..0.100 ms. Same indices as
        # parse-bench-logs.py and pgbench_helper.rs::percentile_index.
        data = [float(i) for i in range(1, 101)]
        self.assertAlmostEqual(C.percentile_ms(data, 0.50), 0.050)
        self.assertAlmostEqual(C.percentile_ms(data, 0.95), 0.095)
        self.assertAlmostEqual(C.percentile_ms(data, 0.99), 0.099)

    def test_index_clamped_at_top(self):
        self.assertEqual(C.percentile_ms([10.0], 0.999), 0.010)


class CollectLatenciesUs(unittest.TestCase):
    def test_no_files_returns_empty(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(C.collect_latencies_us(Path(tmp), "missing"), [])

    def test_extracts_third_column(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "test_pgbenchlog.1").write_text(
                "0 1 1500 0 1700000000 0\n"
                "0 2 2500 0 1700000001 0\n"
            )
            self.assertEqual(
                sorted(C.collect_latencies_us(d, "test")),
                [1500.0, 2500.0],
            )

    def test_concatenates_across_per_thread_files(self):
        # pgbench writes one log per thread (-l --log-prefix=foo → foo.<pid>).
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "test_pgbenchlog.1").write_text("0 1 1000\n")
            (d / "test_pgbenchlog.2").write_text("0 1 2000\n")
            self.assertEqual(
                sorted(C.collect_latencies_us(d, "test")),
                [1000.0, 2000.0],
            )

    def test_skips_malformed_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "test_pgbenchlog.1").write_text(
                "\n"                       # blank line
                "0 1\n"                    # too few columns
                "0 1 not_a_number\n"       # column 3 unparseable
                "0 1 500\n"                # valid
            )
            self.assertEqual(C.collect_latencies_us(d, "test"), [500.0])

    def test_does_not_match_other_test_prefix(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "alpha_pgbenchlog.1").write_text("0 1 100\n")
            (d / "beta_pgbenchlog.1").write_text("0 1 200\n")
            self.assertEqual(C.collect_latencies_us(d, "alpha"), [100.0])


class Summarize(unittest.TestCase):
    def test_three_samples(self):
        # Unsorted input, summarize should sort in place.
        result = C.summarize([2000.0, 1000.0, 3000.0])
        self.assertEqual(result["samples"], 3)
        # n=3, p50: idx = ceil(3*0.5)-1 = 1 → 2000 µs → 2.0 ms
        self.assertEqual(result["p50_ms"], 2.0)
        self.assertEqual(result["p99_ms"], 3.0)

    def test_empty_yields_none_percentiles(self):
        self.assertEqual(
            C.summarize([]),
            {"samples": 0, "p50_ms": None, "p95_ms": None, "p99_ms": None},
        )


class WritePercentilesForResultsDir(unittest.TestCase):
    def test_writes_one_json_per_test_log(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "alpha.log").write_text("tps = 100\nlatency average = 5.0\n")
            (d / "alpha_pgbenchlog.1").write_text("0 1 1000\n0 2 2000\n")
            (d / "beta.log").write_text("tps = 200\n")
            (d / "beta_pgbenchlog.1").write_text("0 1 500\n")
            count = C.write_percentiles_for_results_dir(d)
            self.assertEqual(count, 2)
            alpha = json.loads((d / "alpha_percentiles.json").read_text())
            self.assertEqual(alpha["samples"], 2)
            self.assertEqual(alpha["p99_ms"], 2.0)
            beta = json.loads((d / "beta_percentiles.json").read_text())
            self.assertEqual(beta["samples"], 1)

    def test_skips_service_logs(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            for stem in ("doorman", "odyssey", "pgbouncer", "pg", "bench-wrap"):
                (d / f"{stem}.log").write_text("noise")
            count = C.write_percentiles_for_results_dir(d)
            self.assertEqual(count, 0)
            self.assertFalse((d / "doorman_percentiles.json").exists())
            self.assertFalse((d / "bench-wrap_percentiles.json").exists())

    def test_test_with_no_pgbenchlog_emits_zero_sample_summary(self):
        # A pgbench round that timed out/exited still has its .log; record an
        # empty summary so the parser sees samples=0 instead of falling back
        # to scanning raw files that won't exist on the runner.
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / "failed_test.log").write_text("EXIT_CODE=124\n")
            self.assertEqual(C.write_percentiles_for_results_dir(d), 1)
            payload = json.loads((d / "failed_test_percentiles.json").read_text())
            self.assertEqual(
                payload,
                {"samples": 0, "p50_ms": None, "p95_ms": None, "p99_ms": None},
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
