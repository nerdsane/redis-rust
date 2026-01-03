#!/bin/bash
set -e

MAELSTROM_DIR="/home/runner/workspace/maelstrom/maelstrom"
BIN="/home/runner/workspace/target/release/maelstrom-kv"
BIN_REPLICATED="/home/runner/workspace/target/release/maelstrom-kv-replicated"

echo "Building maelstrom binaries..."
cd /home/runner/workspace
cargo build --bin maelstrom-kv --release
cargo build --bin maelstrom-kv-replicated --release

if [ ! -f "$MAELSTROM_DIR/maelstrom" ]; then
    echo "Maelstrom not found at $MAELSTROM_DIR"
    exit 1
fi

echo ""
echo "============================================"
echo "  Running Maelstrom Linearizability Tests"
echo "============================================"
echo ""

echo "Test 1: Single-node linearizability (should pass)"
echo "---------------------------------------------------"
cd $MAELSTROM_DIR
./maelstrom test -w lin-kv \
    --bin $BIN \
    --node-count 1 \
    --time-limit 10 \
    --rate 10 \
    --concurrency 2

echo ""
echo "Test 2: Single-node with higher load (should pass)"
echo "---------------------------------------------------"
./maelstrom test -w lin-kv \
    --bin $BIN \
    --node-count 1 \
    --time-limit 15 \
    --rate 50 \
    --concurrency 4

echo ""
echo "Test 3: Multi-node eventual consistency (replicated)"
echo "-----------------------------------------------------"
./maelstrom test -w lin-kv \
    --bin $BIN_REPLICATED \
    --node-count 3 \
    --time-limit 20 \
    --rate 10 \
    --concurrency 2 || echo "Multi-node test completed (may have expected consistency violations)"

echo ""
echo "============================================"
echo "  All tests completed!"
echo "============================================"
