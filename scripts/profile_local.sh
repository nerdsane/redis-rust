#!/bin/bash
# Local profiling script - replicates staging workload for CPU profiling
#
# Usage:
#   ./scripts/profile_local.sh           # Run benchmark and show perf report
#   ./scripts/profile_local.sh flamegraph # Generate flamegraph SVG
#
# Prerequisites:
#   - perf (linux-tools-generic)
#   - cargo-flamegraph (cargo install flamegraph)
#
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

MODE=${1:-perf}
PORT=3000
DATA_DIR="/tmp/redis-rust-profile-$$"
mkdir -p "$DATA_DIR"

# Server environment
export REDIS_PORT=$PORT
export REDIS_DATA_PATH="$DATA_DIR"
export REDIS_STORE_TYPE="memory"  # Use memory store for profiling (no disk I/O)
export RUST_LOG="warn"            # Reduce log noise

echo "=== Local Profiling (Staging Workload) ==="
echo "Mode: $MODE"
echo ""

# Build release binaries
echo "Building release binaries..."
cargo build --release --bin server-persistent --bin staging_benchmark 2>/dev/null

# Kill any existing server
pkill -f "server-persistent" 2>/dev/null || true
sleep 1

case $MODE in
    perf)
        echo "Starting server with perf recording..."
        perf record -F 99 -g --call-graph dwarf -o /tmp/perf.data \
            ./target/release/server-persistent &
        SERVER_PID=$!
        sleep 2

        echo "Running staging benchmark..."
        ./target/release/staging_benchmark 127.0.0.1:$PORT

        echo ""
        echo "Stopping server..."
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true

        echo ""
        echo "=== Perf Report (top functions) ==="
        perf report -i /tmp/perf.data --stdio --no-children | head -50

        echo ""
        echo "For interactive report: perf report -i /tmp/perf.data"
        ;;

    flamegraph)
        echo "Starting server for flamegraph..."
        ./target/release/server-persistent &
        SERVER_PID=$!
        sleep 2

        echo "Recording with perf..."
        perf record -F 99 -g --call-graph dwarf -p $SERVER_PID -o /tmp/perf.data &
        PERF_PID=$!
        sleep 1

        echo "Running staging benchmark..."
        ./target/release/staging_benchmark 127.0.0.1:$PORT

        echo ""
        echo "Stopping recording..."
        kill $PERF_PID 2>/dev/null || true
        wait $PERF_PID 2>/dev/null || true

        echo "Stopping server..."
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true

        echo ""
        echo "Generating flamegraph..."
        perf script -i /tmp/perf.data | inferno-collapse-perf | inferno-flamegraph > /tmp/flamegraph.svg

        echo "Flamegraph saved to: /tmp/flamegraph.svg"
        ;;

    simple)
        # Simple mode - just run benchmark against existing or new server
        echo "Starting server..."
        ./target/release/server-persistent &
        SERVER_PID=$!
        sleep 2

        echo "Running staging benchmark..."
        ./target/release/staging_benchmark 127.0.0.1:$PORT

        echo ""
        echo "Stopping server..."
        kill $SERVER_PID 2>/dev/null || true
        ;;

    *)
        echo "Unknown mode: $MODE"
        echo "Usage: $0 [perf|flamegraph|simple]"
        exit 1
        ;;
esac

# Cleanup
rm -rf "$DATA_DIR" 2>/dev/null || true

echo ""
echo "Done!"
