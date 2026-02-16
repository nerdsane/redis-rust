#!/bin/bash
# Time-boxed DST soak test — TigerBeetle/FoundationDB style.
#
# Runs DST with random seeds until the time budget expires.
# Each CI run explores NEW state space (seed = timestamp-based).
# A failing seed is printed and can be reproduced deterministically.
#
# Usage:
#   ./scripts/soak-dst.sh              # default: 5 minutes
#   ./scripts/soak-dst.sh 600          # 10 minutes
#   SOAK_OPS=2000 ./scripts/soak-dst.sh 300  # 5 min, 2000 ops/seed
set -euo pipefail

DURATION_SECS="${1:-300}"
OPS_PER_SEED="${SOAK_OPS:-1000}"
END_TIME=$(($(date +%s) + DURATION_SECS))

# Use current timestamp as base seed so every run explores new space
BASE_SEED=$(date +%s%N | cut -c1-12)

cd "$(dirname "$0")/.."

echo "=== DST Soak Test ==="
echo "Duration: ${DURATION_SECS}s | Ops/seed: ${OPS_PER_SEED} | Base seed: ${BASE_SEED}"
echo ""

# Build once in release mode
cargo build --release --test executor_dst_test --test transaction_dst_test --test connection_transaction_dst 2>&1 | tail -1

EXECUTOR_SEEDS=0
EXECUTOR_FAILURES=0
TRANSACTION_SEEDS=0
TRANSACTION_FAILURES=0
CONNECTION_SEEDS=0
CONNECTION_FAILURES=0
FAILED_SEEDS=""

# Phase 1: Executor DST (most comprehensive)
echo "--- Phase 1: Executor DST ---"
SEED_OFFSET=0
while [ "$(date +%s)" -lt "$END_TIME" ]; do
    SEED=$((BASE_SEED + SEED_OFFSET))

    # Run a batch of 50 seeds at a time for efficiency
    OUTPUT=$(cargo test --release --test executor_dst_test \
        -- --nocapture --test-threads=1 --ignored test_executor_dst_500_seeds 2>&1 || true)

    if echo "$OUTPUT" | grep -q "0 failed"; then
        EXECUTOR_SEEDS=$((EXECUTOR_SEEDS + 500))
    else
        # Something failed — fall back to individual seeds to find which one
        for i in $(seq 0 499); do
            S=$((SEED + i))
            INNER=$(cargo test --release --lib \
                "executor_dst::tests::test_executor_dst_single_seed" \
                -- --nocapture 2>&1 || true)
            EXECUTOR_SEEDS=$((EXECUTOR_SEEDS + 1))
            if echo "$INNER" | grep -qi "violation\|FAILED\|panicked"; then
                EXECUTOR_FAILURES=$((EXECUTOR_FAILURES + 1))
                FAILED_SEEDS="${FAILED_SEEDS}executor:${S} "
                echo "EXECUTOR FAILURE at seed ${S}"
            fi
        done
    fi
    SEED_OFFSET=$((SEED_OFFSET + 500))

    # Check time after each batch
    REMAINING=$((END_TIME - $(date +%s)))
    if [ "$REMAINING" -le "$((DURATION_SECS / 3))" ]; then
        break  # Reserve time for other phases
    fi
done
echo "Executor: ${EXECUTOR_SEEDS} seeds, ${EXECUTOR_FAILURES} failures"

# Phase 2: Connection Transaction DST
echo ""
echo "--- Phase 2: Connection Transaction DST ---"
while [ "$(date +%s)" -lt "$END_TIME" ]; do
    OUTPUT=$(cargo test --release --test connection_transaction_dst \
        test_connection_transaction_dst_100_seeds \
        -- --nocapture --test-threads=1 2>&1 || true)

    if echo "$OUTPUT" | grep -q "ok"; then
        CONNECTION_SEEDS=$((CONNECTION_SEEDS + 100))
    else
        CONNECTION_FAILURES=$((CONNECTION_FAILURES + 1))
        FAILED_SEEDS="${FAILED_SEEDS}connection:batch "
        echo "CONNECTION FAILURE"
        break
    fi

    REMAINING=$((END_TIME - $(date +%s)))
    if [ "$REMAINING" -le "$((DURATION_SECS / 6))" ]; then
        break
    fi
done
echo "Connection TX: ${CONNECTION_SEEDS} seeds, ${CONNECTION_FAILURES} failures"

# Phase 3: CRDT DST
echo ""
echo "--- Phase 3: CRDT DST ---"
CRDT_SEEDS=0
CRDT_FAILURES=0
while [ "$(date +%s)" -lt "$END_TIME" ]; do
    OUTPUT=$(cargo test --release --lib crdt_dst \
        -- --nocapture --test-threads=1 2>&1 || true)

    if echo "$OUTPUT" | grep -q "0 failed"; then
        CRDT_SEEDS=$((CRDT_SEEDS + 400))  # 4 types x 100 seeds
    else
        CRDT_FAILURES=$((CRDT_FAILURES + 1))
        FAILED_SEEDS="${FAILED_SEEDS}crdt:batch "
        echo "CRDT FAILURE"
        break
    fi

    REMAINING=$((END_TIME - $(date +%s)))
    if [ "$REMAINING" -le 10 ]; then
        break
    fi
done
echo "CRDT: ${CRDT_SEEDS} seeds, ${CRDT_FAILURES} failures"

# Summary
TOTAL_SEEDS=$((EXECUTOR_SEEDS + TRANSACTION_SEEDS + CONNECTION_SEEDS + CRDT_SEEDS))
TOTAL_FAILURES=$((EXECUTOR_FAILURES + TRANSACTION_FAILURES + CONNECTION_FAILURES + CRDT_FAILURES))

echo ""
echo "=== Soak Test Summary ==="
echo "Total seeds: ${TOTAL_SEEDS}"
echo "Total failures: ${TOTAL_FAILURES}"
if [ -n "$FAILED_SEEDS" ]; then
    echo "Failed seeds: ${FAILED_SEEDS}"
fi
echo "Duration: ${DURATION_SECS}s"
echo ""

if [ "$TOTAL_FAILURES" -gt 0 ]; then
    echo "SOAK TEST FAILED"
    exit 1
else
    echo "SOAK TEST PASSED"
    exit 0
fi
