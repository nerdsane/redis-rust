#!/bin/bash
# Run the official Redis Tcl test suite against our implementation.
#
# Usage:
#   ./scripts/run-redis-compat.sh                          # default test files
#   ./scripts/run-redis-compat.sh unit/type/string          # specific test file
#   ./scripts/run-redis-compat.sh unit/type/incr unit/expire # multiple
#
# Test paths are relative to the redis tests/ directory (e.g. unit/type/string, unit/expire).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REDIS_TESTS_DIR="$PROJECT_DIR/tests/redis-tests"

# Default test files (ordered by expected compatibility)
DEFAULT_TESTS=(
    "unit/type/string"
    "unit/type/incr"
    "unit/expire"
    "unit/multi"
    "unit/type/set"
    "unit/type/hash"
    "unit/type/list"
    "unit/type/zset"
)

# Use provided test files or defaults
if [ $# -gt 0 ]; then
    TESTS=("$@")
else
    TESTS=("${DEFAULT_TESTS[@]}")
fi

# Find a free port
find_free_port() {
    python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()'
}

PORT=$(find_free_port)
echo "=== Redis Compatibility Test Suite ==="
echo "Port: $PORT"

# Build release binary
echo "Building redis-server-optimized (release)..."
cargo build --bin redis-server-optimized --release --manifest-path "$PROJECT_DIR/Cargo.toml" 2>&1 | tail -3
BINARY="$PROJECT_DIR/target/release/redis-server-optimized"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Build failed - binary not found at $BINARY"
    exit 1
fi

# Start server (uses REDIS_PORT env var)
echo "Starting server on port $PORT..."
REDIS_PORT="$PORT" "$BINARY" > /tmp/redis-rust-compat-server.log 2>&1 &
SERVER_PID=$!

# Ensure cleanup on exit
cleanup() {
    if kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Wait for server to be ready (using raw TCP since redis-cli may not be installed)
echo "Waiting for server..."
for i in $(seq 1 30); do
    if (echo -e '*1\r\n$4\r\nPING\r\n'; sleep 0.3) | nc -w 1 127.0.0.1 "$PORT" 2>/dev/null | grep -q '+PONG'; then
        echo "Server ready."
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "ERROR: Server failed to start within 30 seconds"
        cat /tmp/redis-rust-compat-server.log
        exit 1
    fi
    sleep 1
done

# Check submodule
if [ ! -f "$REDIS_TESTS_DIR/runtest" ]; then
    echo "ERROR: Redis test suite not found at $REDIS_TESTS_DIR"
    echo "Run: git submodule update --init"
    exit 1
fi

# Run tests
PASSED=0
FAILED=0
ERRORS=()

for TEST in "${TESTS[@]}"; do
    echo ""
    echo "--- Running: $TEST ---"

    if cd "$REDIS_TESTS_DIR" && \
       ./runtest \
         --host 127.0.0.1 \
         --port "$PORT" \
         --single "$TEST" \
         --tags "-needs:debug -needs:repl -needs:save -needs:config-maxmemory -needs:reset" \
         --ignore-encoding \
         --ignore-digest \
         2>&1; then
        PASSED=$((PASSED + 1))
        echo "--- PASSED: $TEST ---"
    else
        FAILED=$((FAILED + 1))
        ERRORS+=("$TEST")
        echo "--- FAILED: $TEST ---"
    fi
done

# Summary
echo ""
echo "=== Summary ==="
echo "Passed: $PASSED"
echo "Failed: $FAILED"
if [ ${#ERRORS[@]} -gt 0 ]; then
    echo "Failed tests:"
    for t in "${ERRORS[@]}"; do
        echo "  - $t"
    done
fi

exit "$FAILED"
