#!/usr/bin/env bash
set -e

echo "=========================================="
echo "Redis 7.4 vs Redis 8.0 vs Rust Comparison"
echo "=========================================="
echo ""

# Configuration
REQUESTS=100000
CLIENTS=50
DATA_SIZE=64

# Commands to benchmark (16 tests)
# Misc: ping_mbulk (latency baseline)
# Core: set, get, incr, mset
# List: lpush, rpush, lpop, rpop, lrange_100, lrange_300, lrange_500
# Set: sadd
# Hash: hset
# Sorted Set: zadd
COMMANDS="ping_mbulk set get mset incr lpush rpush lpop rpop lrange_100 lrange_300 lrange_500 sadd hset zadd"

cd "$(dirname "$0")"

# Output files
RESULTS_DIR="./results"
mkdir -p "$RESULTS_DIR"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

echo "Configuration:"
echo "  Requests: $REQUESTS"
echo "  Clients: $CLIENTS"
echo "  Data size: $DATA_SIZE bytes"
echo "  CPU limit: 2 cores per container"
echo "  Memory limit: 1GB per container"
echo "  Commands: $COMMANDS"
echo "  Results dir: $RESULTS_DIR"
echo ""

# Start containers using the Redis 8 compose file
echo "Starting containers..."
docker compose -f docker-compose.redis8.yml down 2>/dev/null || true
docker compose -f docker-compose.redis8.yml up -d --build

# Wait for servers to be ready
echo "Waiting for servers to start..."
sleep 8

# Check connectivity
echo "Checking server connectivity..."
echo -n "  Redis 7.4 (port 6379): "
docker run --rm --network host redis:7.4-alpine redis-cli -p 6379 PING || { echo "FAILED"; exit 1; }
echo -n "  Redis 8.0 (port 6380): "
docker run --rm --network host redis:8.0 redis-cli -p 6380 PING || { echo "FAILED"; exit 1; }
echo -n "  Rust impl (port 3000): "
docker run --rm --network host redis:7.4-alpine redis-cli -p 3000 PING || { echo "FAILED"; exit 1; }

# Function to run benchmark and extract throughput
run_bench() {
    local port=$1
    local cmd=$2
    local pipeline=${3:-1}

    local csv_output=$(docker run --rm --network host redis:8.0 \
        redis-benchmark -p $port -n $REQUESTS -c $CLIENTS -P $pipeline -d $DATA_SIZE -r 10000 \
        -t $cmd --csv 2>/dev/null)

    echo "$csv_output" | grep -iE "^\"$cmd" | head -1 | cut -d',' -f2 | tr -d '"'
}

# Calculate percentage
calc_pct() {
    if [ -n "$1" ] && [ -n "$2" ] && [ "$2" != "0" ]; then
        echo "scale=1; $1 * 100 / $2" | bc 2>/dev/null || echo "N/A"
    else
        echo "N/A"
    fi
}

# Create results file
RESULTS_FILE="$RESULTS_DIR/redis8_comparison_${TIMESTAMP}.md"

cat > "$RESULTS_FILE" << 'HEADER'
# Redis 7.4 vs Redis 8.0 vs Rust Implementation

## Test Configuration
- **Method**: Docker benchmarks (docker-compose.redis8.yml)
- **CPU Limit**: 2 cores per container
- **Memory Limit**: 1GB per container
- **Requests**: 100,000
- **Clients**: 50 concurrent
- **Data Size**: 64 bytes

## Results

HEADER

# Temp files for results
P1_FILE=$(mktemp)
P16_FILE=$(mktemp)
trap "rm -f $P1_FILE $P16_FILE" EXIT

echo ""
echo "=========================================="
echo "Running Benchmarks - Non-Pipelined (P=1)"
echo "=========================================="

echo "### Non-Pipelined Performance (P=1)" >> "$RESULTS_FILE"
echo "" >> "$RESULTS_FILE"
echo "| Command | Redis 7.4 | Redis 8.0 | Rust | Rust vs R8 |" >> "$RESULTS_FILE"
echo "|---------|-----------|-----------|------|------------|" >> "$RESULTS_FILE"

for cmd in $COMMANDS; do
    CMD_UPPER=$(echo $cmd | tr '[:lower:]' '[:upper:]')
    echo ""
    echo "--- $CMD_UPPER ---"

    echo "  Redis 7.4..."
    R7=$(run_bench 6379 "$cmd" 1)
    echo "    $R7 req/s"

    echo "  Redis 8.0..."
    R8=$(run_bench 6380 "$cmd" 1)
    echo "    $R8 req/s"

    echo "  Rust..."
    RUST=$(run_bench 3000 "$cmd" 1)
    echo "    $RUST req/s"

    PCT=$(calc_pct "$RUST" "$R8")
    echo "| $CMD_UPPER | $R7 | $R8 | $RUST | ${PCT}% |" >> "$RESULTS_FILE"
    echo "$CMD_UPPER $PCT" >> "$P1_FILE"
done

echo ""
echo "=========================================="
echo "Running Benchmarks - Pipelined (P=16)"
echo "=========================================="

echo "" >> "$RESULTS_FILE"
echo "### Pipelined Performance (P=16)" >> "$RESULTS_FILE"
echo "" >> "$RESULTS_FILE"
echo "| Command | Redis 7.4 | Redis 8.0 | Rust | Rust vs R8 |" >> "$RESULTS_FILE"
echo "|---------|-----------|-----------|------|------------|" >> "$RESULTS_FILE"

for cmd in $COMMANDS; do
    CMD_UPPER=$(echo $cmd | tr '[:lower:]' '[:upper:]')
    echo ""
    echo "--- $CMD_UPPER ---"

    echo "  Redis 7.4..."
    R7=$(run_bench 6379 "$cmd" 16)
    echo "    $R7 req/s"

    echo "  Redis 8.0..."
    R8=$(run_bench 6380 "$cmd" 16)
    echo "    $R8 req/s"

    echo "  Rust..."
    RUST=$(run_bench 3000 "$cmd" 16)
    echo "    $RUST req/s"

    PCT=$(calc_pct "$RUST" "$R8")
    echo "| $CMD_UPPER | $R7 | $R8 | $RUST | ${PCT}% |" >> "$RESULTS_FILE"
    echo "$CMD_UPPER $PCT" >> "$P16_FILE"
done

# Write summary
echo "" >> "$RESULTS_FILE"
echo "## Summary" >> "$RESULTS_FILE"
echo "" >> "$RESULTS_FILE"
echo "### Rust vs Redis 8.0 (P=1)" >> "$RESULTS_FILE"
while read -r line; do
    CMD=$(echo "$line" | cut -d' ' -f1)
    PCT=$(echo "$line" | cut -d' ' -f2)
    echo "- $CMD: ${PCT}% of Redis 8.0" >> "$RESULTS_FILE"
done < "$P1_FILE"

echo "" >> "$RESULTS_FILE"
echo "### Rust vs Redis 8.0 (P=16)" >> "$RESULTS_FILE"
while read -r line; do
    CMD=$(echo "$line" | cut -d' ' -f1)
    PCT=$(echo "$line" | cut -d' ' -f2)
    echo "- $CMD: ${PCT}% of Redis 8.0" >> "$RESULTS_FILE"
done < "$P16_FILE"

echo "" >> "$RESULTS_FILE"
echo "---" >> "$RESULTS_FILE"
echo "Generated: $(date)" >> "$RESULTS_FILE"

# Print summary to console
echo ""
echo "=========================================="
echo "Results Summary (Rust vs Redis 8.0)"
echo "=========================================="
echo ""
echo "Non-Pipelined (P=1):"
while read -r line; do
    CMD=$(echo "$line" | cut -d' ' -f1)
    PCT=$(echo "$line" | cut -d' ' -f2)
    printf "  %-6s: %s%%\n" "$CMD" "$PCT"
done < "$P1_FILE"

echo ""
echo "Pipelined (P=16):"
while read -r line; do
    CMD=$(echo "$line" | cut -d' ' -f1)
    PCT=$(echo "$line" | cut -d' ' -f2)
    printf "  %-6s: %s%%\n" "$CMD" "$PCT"
done < "$P16_FILE"

echo ""
echo "Full results saved to: $RESULTS_FILE"

# Cleanup
echo ""
echo "Cleaning up..."
docker compose -f docker-compose.redis8.yml down

echo ""
echo "Benchmark complete!"
