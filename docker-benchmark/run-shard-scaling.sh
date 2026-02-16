#!/bin/bash
# Test how Rust Redis performance scales with shard count.
# Runs redis-benchmark at P=1 and P=16 for each shard count.
set -euo pipefail

cd "$(dirname "$0")"

REQUESTS=100000
CLIENTS=50
DATA_SIZE=64

# Port for official Redis (reference, run once)
OFFICIAL_PORT=6399
RUST_PORT=3000

SHARD_COUNTS=(1 2 4 8 16 32)

echo "=== Shard Scaling Test ==="
echo "Requests: $REQUESTS | Clients: $CLIENTS | Data: ${DATA_SIZE}B"
echo ""

# --- Reference: Official Redis 7.4 (single run) ---
echo "--- Starting Redis 7.4 reference ---"
docker rm -f redis-ref 2>/dev/null || true
docker run -d --name redis-ref -p ${OFFICIAL_PORT}:6379 \
  --cpus=2 --memory=1g redis:7.4-alpine redis-server --save "" --appendonly no >/dev/null
sleep 2

REF_SET_P1=$(docker run --rm --network host redis:7.4-alpine \
  redis-benchmark -p $OFFICIAL_PORT -n $REQUESTS -c $CLIENTS -P 1 -d $DATA_SIZE -r 10000 \
  -t set -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')
REF_GET_P1=$(docker run --rm --network host redis:7.4-alpine \
  redis-benchmark -p $OFFICIAL_PORT -n $REQUESTS -c $CLIENTS -P 1 -r 10000 \
  -t get -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')
REF_SET_P16=$(docker run --rm --network host redis:7.4-alpine \
  redis-benchmark -p $OFFICIAL_PORT -n $REQUESTS -c $CLIENTS -P 16 -d $DATA_SIZE -r 10000 \
  -t set -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')
REF_GET_P16=$(docker run --rm --network host redis:7.4-alpine \
  redis-benchmark -p $OFFICIAL_PORT -n $REQUESTS -c $CLIENTS -P 16 -r 10000 \
  -t get -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')

docker rm -f redis-ref >/dev/null 2>&1

echo "Redis 7.4 reference:  SET P=1: ${REF_SET_P1}  GET P=1: ${REF_GET_P1}  SET P=16: ${REF_SET_P16}  GET P=16: ${REF_GET_P16}"
echo ""

# --- Scaling test ---
printf "%-8s  %12s  %12s  %12s  %12s\n" "Shards" "SET P=1" "GET P=1" "SET P=16" "GET P=16"
printf "%-8s  %12s  %12s  %12s  %12s\n" "------" "-----------" "-----------" "-----------" "-----------"

for SHARDS in "${SHARD_COUNTS[@]}"; do
  # Write config
  cat > perf_config_test.toml <<EOF
num_shards = ${SHARDS}

[response_pool]
capacity = 576
prewarm = 96

[buffers]
read_size = 8192
max_size = 536870912

[batching]
min_pipeline_buffer = 70
batch_threshold = 6
EOF

  # Start Rust server with this config
  docker rm -f redis-rust-test 2>/dev/null || true
  docker run -d --name redis-rust-test -p ${RUST_PORT}:6379 \
    --cpus=2 --memory=1g \
    -e RUST_LOG=warn \
    -e PERF_CONFIG_PATH=/etc/redis-rust/perf_config.toml \
    -v "$(pwd)/perf_config_test.toml:/etc/redis-rust/perf_config.toml:ro" \
    docker-benchmark-redis-rust >/dev/null
  sleep 2

  # Verify it's up
  if ! docker run --rm --network host redis:7.4-alpine redis-cli -p $RUST_PORT PING 2>/dev/null | grep -q PONG; then
    echo "Shards=$SHARDS: server failed to start"
    docker rm -f redis-rust-test 2>/dev/null || true
    continue
  fi

  # Benchmark
  S1=$(docker run --rm --network host redis:7.4-alpine \
    redis-benchmark -p $RUST_PORT -n $REQUESTS -c $CLIENTS -P 1 -d $DATA_SIZE -r 10000 \
    -t set -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')
  G1=$(docker run --rm --network host redis:7.4-alpine \
    redis-benchmark -p $RUST_PORT -n $REQUESTS -c $CLIENTS -P 1 -r 10000 \
    -t get -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')
  S16=$(docker run --rm --network host redis:7.4-alpine \
    redis-benchmark -p $RUST_PORT -n $REQUESTS -c $CLIENTS -P 16 -d $DATA_SIZE -r 10000 \
    -t set -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')
  G16=$(docker run --rm --network host redis:7.4-alpine \
    redis-benchmark -p $RUST_PORT -n $REQUESTS -c $CLIENTS -P 16 -r 10000 \
    -t get -q 2>/dev/null | sed -n 's/.*: \([0-9.]*\) requests.*/\1/p')

  printf "%-8s  %12s  %12s  %12s  %12s\n" "$SHARDS" "$S1" "$G1" "$S16" "$G16"

  docker rm -f redis-rust-test >/dev/null 2>&1
done

# Cleanup
rm -f perf_config_test.toml
echo ""
echo "Reference (Redis 7.4):"
printf "%-8s  %12s  %12s  %12s  %12s\n" "R7.4" "$REF_SET_P1" "$REF_GET_P1" "$REF_SET_P16" "$REF_GET_P16"
