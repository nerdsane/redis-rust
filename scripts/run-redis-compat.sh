#!/bin/bash
# Run the official Redis Tcl test suite against our implementation.
#
# Usage:
#   ./scripts/run-redis-compat.sh                          # default test files
#   ./scripts/run-redis-compat.sh unit/type/string          # specific test file
#   ./scripts/run-redis-compat.sh unit/type/incr unit/expire # multiple
#
# Test paths are relative to the redis tests/ directory (e.g. unit/type/string, unit/expire).
#
# Modes:
#   - External mode (default): We start the server, Tcl connects to it
#   - Internal mode (for acl): Tcl harness starts/stops the server via wrapper script
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REDIS_TESTS_DIR="$PROJECT_DIR/tests/redis-tests"

# Tests that require internal mode (the Tcl harness spawns the server itself)
INTERNAL_MODE_TESTS=("unit/acl")

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

# Check if a test needs internal mode
needs_internal_mode() {
    local test="$1"
    for internal_test in "${INTERNAL_MODE_TESTS[@]}"; do
        if [ "$test" = "$internal_test" ]; then
            return 0
        fi
    done
    return 1
}

# Find a free port
find_free_port() {
    python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()'
}

echo "=== Redis Compatibility Test Suite ==="

# Build release binary (always with acl feature)
echo "Building redis-server-optimized (release, features=acl)..."
cargo build --bin redis-server-optimized --release --features acl --manifest-path "$PROJECT_DIR/Cargo.toml" 2>&1 | tail -3
BINARY="$PROJECT_DIR/target/release/redis-server-optimized"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Build failed - binary not found at $BINARY"
    exit 1
fi

# Check if any tests need internal mode — set up wrapper if so
NEEDS_INTERNAL=false
for TEST in "${TESTS[@]}"; do
    if needs_internal_mode "$TEST"; then
        NEEDS_INTERNAL=true
        break
    fi
done

if [ "$NEEDS_INTERNAL" = true ]; then
    echo "Setting up internal mode (wrapper script)..."
    # Install our wrapper as src/redis-server in the redis-tests directory
    cp "$SCRIPT_DIR/redis-server-wrapper.sh" "$REDIS_TESTS_DIR/src/redis-server"
    chmod +x "$REDIS_TESTS_DIR/src/redis-server"
    # Also place the binary where the wrapper can find it
    cp "$BINARY" "$REDIS_TESTS_DIR/src/redis-server-optimized"
fi

# Check submodule
if [ ! -f "$REDIS_TESTS_DIR/runtest" ]; then
    echo "ERROR: Redis test suite not found at $REDIS_TESTS_DIR"
    echo "Run: git submodule update --init"
    exit 1
fi

# For external mode tests, start our server
SERVER_PID=""
PORT=""

start_external_server() {
    PORT=$(find_free_port)
    echo "Starting server on port $PORT..."
    REDIS_PORT="$PORT" "$BINARY" > /tmp/redis-rust-compat-server.log 2>&1 &
    SERVER_PID=$!

    # Wait for server to be ready
    echo "Waiting for server..."
    for i in $(seq 1 30); do
        if (echo -e '*1\r\n$4\r\nPING\r\n'; sleep 0.3) | nc -w 1 127.0.0.1 "$PORT" 2>/dev/null | grep -q '+PONG'; then
            echo "Server ready on port $PORT."
            return 0
        fi
        if [ "$i" -eq 30 ]; then
            echo "ERROR: Server failed to start within 30 seconds"
            cat /tmp/redis-rust-compat-server.log
            return 1
        fi
        sleep 1
    done
}

stop_external_server() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
        SERVER_PID=""
    fi
}

# Ensure cleanup on exit
cleanup() {
    stop_external_server
    # Clean up wrapper files from submodule
    rm -f "$REDIS_TESTS_DIR/src/redis-server" "$REDIS_TESTS_DIR/src/redis-server-optimized"
}
trap cleanup EXIT

# Run tests
PASSED=0
FAILED=0
ERRORS=()

for TEST in "${TESTS[@]}"; do
    echo ""
    echo "--- Running: $TEST ---"

    if needs_internal_mode "$TEST"; then
        # Internal mode: Tcl harness spawns server via wrapper
        echo "(internal mode — Tcl harness manages server lifecycle)"
        if cd "$REDIS_TESTS_DIR" && \
           ./runtest \
             --single "$TEST" \
             --singledb \
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
    else
        # External mode: we manage the server
        if [ -z "$SERVER_PID" ] || ! kill -0 "$SERVER_PID" 2>/dev/null; then
            start_external_server || exit 1
        fi

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
