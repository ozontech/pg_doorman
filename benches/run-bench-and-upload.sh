#!/bin/bash
set -e

echo "=========================================="
echo "Starting pg_doorman benchmark tests"
echo "=========================================="

# Run the benchmark tests
cargo test --release --test bdd -- --tags @bench

# Check if benchmarks.md was generated
if [ ! -f "documentation/docs/benchmarks.md" ]; then
    echo "ERROR: benchmarks.md not found after test run"
    exit 1
fi

echo ""
echo "=========================================="
echo "Benchmark tests completed successfully"
echo "=========================================="
echo ""

# Output the file as base64 between markers for easy extraction
echo "===BEGIN_BENCHMARK_RESULTS==="
base64 -w 0 documentation/docs/benchmarks.md
echo ""
echo "===END_BENCHMARK_RESULTS==="

echo ""
echo "Upload complete!"
