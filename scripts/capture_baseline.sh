#!/bin/bash
# capture_baseline.sh - Capture performance baseline for optimization comparison
#
# Usage:
#   ./scripts/capture_baseline.sh          # Run Criterion + Docker benchmarks
#   ./scripts/capture_baseline.sh --quick  # Run Criterion only (fast)
#   ./scripts/capture_baseline.sh --docker # Run Docker only (fair comparison)

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BASELINES_DIR="$PROJECT_ROOT/baselines"

mkdir -p "$BASELINES_DIR"

echo "=============================================="
echo "  Performance Baseline Capture"
echo "  Timestamp: $TIMESTAMP"
echo "=============================================="
echo ""

run_criterion() {
    echo "Running Criterion benchmarks..."
    echo "  This provides sub-second hot path measurements"
    echo ""

    cd "$PROJECT_ROOT"

    # Save baseline for future comparison
    cargo bench --bench hot_paths -- --save-baseline "baseline_$TIMESTAMP" 2>&1 | tee "$BASELINES_DIR/criterion_$TIMESTAMP.txt"

    echo ""
    echo "Criterion baseline saved to: $BASELINES_DIR/criterion_$TIMESTAMP.txt"
    echo "Compare with: cargo bench --bench hot_paths -- --baseline baseline_$TIMESTAMP"
}

run_docker() {
    echo "Running Docker benchmarks (Redis 8.0 comparison)..."
    echo "  This provides fair end-to-end comparison"
    echo ""

    if [ ! -d "$PROJECT_ROOT/docker-benchmark" ]; then
        echo "ERROR: docker-benchmark directory not found"
        exit 1
    fi

    cd "$PROJECT_ROOT/docker-benchmark"

    # Run the comparison script
    ./run-redis8-comparison.sh

    # Copy results to baselines
    LATEST_RESULT=$(ls -t results/redis8_comparison*.md 2>/dev/null | head -1)
    if [ -n "$LATEST_RESULT" ]; then
        cp "$LATEST_RESULT" "$BASELINES_DIR/docker_baseline_$TIMESTAMP.md"
        echo ""
        echo "Docker baseline saved to: $BASELINES_DIR/docker_baseline_$TIMESTAMP.md"
    fi
}

show_summary() {
    echo ""
    echo "=============================================="
    echo "  Baseline Summary"
    echo "=============================================="

    if [ -f "$BASELINES_DIR/criterion_$TIMESTAMP.txt" ]; then
        echo ""
        echo "Criterion Hot Path Results:"
        grep -E "set_direct|get_direct" "$BASELINES_DIR/criterion_$TIMESTAMP.txt" | head -10
    fi

    if [ -f "$BASELINES_DIR/docker_baseline_$TIMESTAMP.md" ]; then
        echo ""
        echo "Docker Benchmark Results (vs Redis 8.0):"
        grep -E "^\| SET|^\| GET" "$BASELINES_DIR/docker_baseline_$TIMESTAMP.md" | head -10
    fi

    echo ""
    echo "Baselines saved to: $BASELINES_DIR/"
    ls -la "$BASELINES_DIR"/*"$TIMESTAMP"* 2>/dev/null || true
}

# Parse arguments
MODE="both"
if [ "$1" == "--quick" ]; then
    MODE="criterion"
elif [ "$1" == "--docker" ]; then
    MODE="docker"
elif [ "$1" == "--help" ]; then
    echo "Usage: $0 [--quick|--docker]"
    echo ""
    echo "Options:"
    echo "  --quick   Run Criterion benchmarks only (fast, ~30s)"
    echo "  --docker  Run Docker benchmarks only (fair comparison, ~5min)"
    echo "  (default) Run both Criterion and Docker benchmarks"
    exit 0
fi

case "$MODE" in
    criterion)
        run_criterion
        ;;
    docker)
        run_docker
        ;;
    both)
        run_criterion
        echo ""
        echo "----------------------------------------------"
        echo ""
        run_docker
        ;;
esac

show_summary

echo ""
echo "=============================================="
echo "  Baseline capture complete!"
echo "=============================================="
