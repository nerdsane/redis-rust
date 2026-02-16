#!/bin/bash
# Wrapper script that translates Redis config file + startup protocol
# for the Tcl test harness's internal (start_server) mode.
#
# The Tcl harness runs:   src/redis-server <config-file> [--extra-args]
# It redirects our stdout to a log file and watches it for:
#   1) A line matching: " PID: <pid>.*Server initialized"
#   2) A line containing: "Ready to accept"
#
# This wrapper parses the config file, starts our binary with the right
# env vars, and prints the expected startup messages.

set -uo pipefail

CONFIG_FILE="${1:-}"

# Parse config file for settings we care about
PORT=6379
BIND="127.0.0.1"
REQUIREPASS=""
LOGLEVEL="notice"
DIR=""

if [ -n "$CONFIG_FILE" ] && [ -f "$CONFIG_FILE" ]; then
    while IFS= read -r line; do
        # Skip comments and empty lines
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        [[ -z "$line" ]] && continue

        key=$(echo "$line" | awk '{print $1}')
        value=$(echo "$line" | awk '{$1=""; print $0}' | sed 's/^ *//' | sed 's/"//g')

        case "$key" in
            port)         PORT="$value" ;;
            bind)         BIND="$value" ;;
            requirepass)  REQUIREPASS="$value" ;;
            loglevel)     LOGLEVEL="$value" ;;
            dir)          DIR="$value" ;;
        esac
    done < "$CONFIG_FILE"
fi

# Resolve binary path
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY=""
for candidate in \
    "$SCRIPT_DIR/redis-server-optimized" \
    "$SCRIPT_DIR/../../../target/release/redis-server-optimized" \
    "$SCRIPT_DIR/../../target/release/redis-server-optimized"; do
    if [ -f "$candidate" ] && [ -x "$candidate" ]; then
        BINARY="$(cd "$(dirname "$candidate")" && pwd)/$(basename "$candidate")"
        break
    fi
done

if [ -z "$BINARY" ]; then
    echo "ERROR: Cannot find redis-server-optimized binary" >&2
    exit 1
fi

PID=$$
TIMESTAMP=$(date "+%d %b %Y %H:%M:%S.000")

# Print Phase 1: PID + Server initialized (checked by wait_server_started)
echo "${PID}:M ${TIMESTAMP} * oO0OoO0OoO0Oo Redis is starting oO0OoO0OoO0Oo"
echo "${PID}:M ${TIMESTAMP} * PID: ${PID}, Server initialized"

# Determine log directory — use DIR from config, or create temp
LOG_DIR="${DIR:-/tmp}"
SERVER_LOG="$LOG_DIR/redis-server-wrapper-$PID.log"

# Build environment variables for our binary
ENV_VARS="REDIS_PORT=$PORT"
if [ -n "$REQUIREPASS" ]; then
    ENV_VARS="$ENV_VARS REDIS_REQUIRE_PASS=$REQUIREPASS"
fi

# Start the actual server in background, redirect its output away from our stdout
env REDIS_PORT="$PORT" ${REQUIREPASS:+REDIS_REQUIRE_PASS="$REQUIREPASS"} \
    "$BINARY" > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

# Forward signals to the server process
cleanup() {
    if kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT TERM INT

# Wait for server to accept connections
TIMEOUT=30
for i in $(seq 1 $TIMEOUT); do
    if (printf '*1\r\n$4\r\nPING\r\n'; sleep 0.3) | nc -w 1 127.0.0.1 "$PORT" 2>/dev/null | grep -q '+PONG'; then
        break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "ERROR: Server process died. Log:" >&2
        cat "$SERVER_LOG" >&2
        exit 1
    fi
    if [ "$i" -eq "$TIMEOUT" ]; then
        echo "ERROR: Server failed to start within ${TIMEOUT}s. Log:" >&2
        cat "$SERVER_LOG" >&2
        exit 1
    fi
    sleep 1
done

# Print Phase 2: Ready to accept (checked by start_server loop)
echo "${PID}:M ${TIMESTAMP} * Ready to accept connections tcp"

# Wait for server to exit (the Tcl harness will kill us when the test is done)
wait "$SERVER_PID" 2>/dev/null || true
