#!/bin/bash
# Simple staging benchmark with metrics feedback
# Usage: ./scripts/bench_staging.sh [duration_seconds]

set -e
DURATION=${1:-30}
SERVICE="redis-rust"
ENV="staging"

echo "=== Staging Benchmark for $SERVICE ==="
echo "Duration: ${DURATION}s"
echo ""

# Start port-forward in background
kubectl port-forward -n rapid-sims svc/redis-rust 6399:3000 &
PF_PID=$!
sleep 2

# Record start time
START_TIME=$(date +%s)

# Run benchmark
echo "Running redis-benchmark..."
redis-benchmark -p 6399 -c 50 -n $((DURATION * 5000)) -t set,get -q

# Record end time
END_TIME=$(date +%s)

# Kill port-forward
kill $PF_PID 2>/dev/null || true

echo ""
echo "=== Metrics from Datadog (last ${DURATION}s) ==="

# Query key metrics via sherlock
cd /home/bits/go/src/github.com/DataDog/sherlock
uv run python3 -c "
import sys
sys.path.insert(0, 'src')
from retriever.client import RetrieverClient

client = RetrieverClient(
    datacenter='us1.staging.dog',
    org_id=2,
    client_id='rapid-xpq'
)

# CPU usage
cpu = client.execute_query('''
SELECT
    avg(value) as avg_cpu,
    max(value) as max_cpu
FROM metrics
WHERE metric = 'container.cpu.usage'
AND service = 'redis-rust'
AND env = 'staging'
AND timestamp > now() - interval 2 minute
''')

# Memory
mem = client.execute_query('''
SELECT
    avg(value)/1024/1024 as avg_mb,
    max(value)/1024/1024 as max_mb
FROM metrics
WHERE metric = 'container.memory.rss'
AND service = 'redis-rust'
AND env = 'staging'
AND timestamp > now() - interval 2 minute
''')

print('CPU Usage:')
for r in cpu:
    print(f'  Avg: {r.get(\"avg_cpu\", 0):.2f}%  Max: {r.get(\"max_cpu\", 0):.2f}%')

print('Memory:')
for r in mem:
    print(f'  Avg: {r.get(\"avg_mb\", 0):.1f} MB  Max: {r.get(\"max_mb\", 0):.1f} MB')

client.close()
"

echo ""
echo "Done! View profiles: https://ddstaging.datadoghq.com/profiling/explorer?query=service%3Aredis-rust"
