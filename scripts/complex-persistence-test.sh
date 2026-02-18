#!/usr/bin/env bash
set -euo pipefail

# Complex WAL persistence test — multiple data types, overwrites, multiple crashes
# Requires: docker compose with wal-integration already running

PASS=0; FAIL=0

rcli() { docker exec wal-redis-official redis-cli -h wal-node-a -p 6379 "$@"; }

check() {
  local actual="$1" expected="$2" label="$3"
  if [ "$actual" = "$expected" ]; then
    PASS=$((PASS+1)); echo "  PASS: $label"
  else
    FAIL=$((FAIL+1)); echo "  FAIL: $label (expected='$expected', got='$actual')"
  fi
}

echo "============================================"
echo "  Complex WAL Persistence Test"
echo "============================================"
echo ""

# ── Phase 1: Multiple data types ─────────────────────────
echo "=== Phase 1: Write multiple data types ==="

# Strings
for i in $(seq 1 50); do rcli SET "str:$i" "value-$i" > /dev/null; done
echo "  50 strings written"

# Counters (INCR)
rcli SET counter:a 0 > /dev/null
for i in $(seq 1 100); do rcli INCR counter:a > /dev/null; done
echo "  counter:a incremented 100 times"

# Lists (LPUSH)
for i in $(seq 1 20); do rcli LPUSH mylist "item-$i" > /dev/null; done
echo "  20 items pushed to mylist"

# Hashes (HSET)
for i in $(seq 1 30); do rcli HSET myhash "field-$i" "hashval-$i" > /dev/null; done
echo "  30 fields set on myhash"

# Sets (SADD)
for i in $(seq 1 25); do rcli SADD myset "member-$i" > /dev/null; done
echo "  25 members added to myset"

# Sorted sets (ZADD)
for i in $(seq 1 15); do rcli ZADD myzset "$i" "zitem-$i" > /dev/null; done
echo "  15 members added to myzset"

# Overwrites — same key written multiple times
rcli SET overwrite:key "version-1" > /dev/null
rcli SET overwrite:key "version-2" > /dev/null
rcli SET overwrite:key "version-3" > /dev/null
echo "  overwrite:key set 3 times (final=version-3)"

# Large value (10KB)
LARGE=$(python3 -c "print('X' * 10000)")
rcli SET large:key "$LARGE" > /dev/null
echo "  1 large value (10KB)"

echo ""
echo "=== Phase 1 verification ==="
check "$(rcli GET str:25)" "value-25" "str:25"
check "$(rcli GET counter:a)" "100" "counter:a = 100"
check "$(rcli LLEN mylist)" "20" "mylist length = 20"
check "$(rcli HLEN myhash)" "30" "myhash length = 30"
check "$(rcli SCARD myset)" "25" "myset cardinality = 25"
check "$(rcli ZCARD myzset)" "15" "myzset cardinality = 15"
check "$(rcli GET overwrite:key)" "version-3" "overwrite:key = version-3"
check "$(rcli STRLEN large:key)" "10000" "large:key length = 10000"

echo ""
echo "=== Phase 2: SIGKILL crash #1 ==="
docker kill wal-node-a > /dev/null
echo "  node-a killed"
docker start wal-node-a > /dev/null
sleep 8
echo "  node-a restarted"

echo ""
echo "=== Phase 2 verification (all types survive crash) ==="
check "$(rcli GET str:1)" "value-1" "str:1 survived"
check "$(rcli GET str:50)" "value-50" "str:50 survived"
check "$(rcli GET counter:a)" "100" "counter:a = 100 survived"
check "$(rcli LLEN mylist)" "20" "mylist length survived"
check "$(rcli HLEN myhash)" "30" "myhash length survived"
check "$(rcli SCARD myset)" "25" "myset cardinality survived"
check "$(rcli ZCARD myzset)" "15" "myzset cardinality survived"
check "$(rcli GET overwrite:key)" "version-3" "overwrite final value survived"
check "$(rcli STRLEN large:key)" "10000" "large value survived"
check "$(rcli HGET myhash field-15)" "hashval-15" "hash field-15 survived"
check "$(rcli LINDEX mylist 0)" "item-20" "list head survived"
check "$(rcli ZSCORE myzset zitem-10)" "10" "sorted set score survived"
check "$(rcli SISMEMBER myset member-13)" "1" "set member survived"

echo ""
echo "=== Phase 3: Modify data after recovery, then crash again ==="

# Update existing keys
rcli SET counter:a 0 > /dev/null
for i in $(seq 1 200); do rcli INCR counter:a > /dev/null; done
echo "  counter:a reset and incremented to 200"

# Delete some keys
rcli DEL str:1 str:2 str:3 > /dev/null
echo "  3 string keys deleted"

# Add to existing collections
for i in $(seq 21 40); do rcli LPUSH mylist "item-$i" > /dev/null; done
echo "  20 more items pushed to mylist (now 40)"

# Overwrite again
rcli SET overwrite:key "version-4-post-recovery" > /dev/null
echo "  overwrite:key updated to version-4-post-recovery"

# New keys
for i in $(seq 1 30); do rcli SET "new:$i" "post-crash-$i" > /dev/null; done
echo "  30 new keys written"

echo ""
echo "=== Phase 3: SIGKILL crash #2 ==="
docker kill wal-node-a > /dev/null
echo "  node-a killed"
docker start wal-node-a > /dev/null
sleep 8
echo "  node-a restarted"

echo ""
echo "=== Phase 3 verification (mutations + deletions survive) ==="
check "$(rcli GET counter:a)" "200" "counter:a = 200"
check "$(rcli GET str:1)" "" "str:1 deleted (empty)"
check "$(rcli GET str:4)" "value-4" "str:4 still exists"
check "$(rcli LLEN mylist)" "40" "mylist length = 40"
check "$(rcli GET overwrite:key)" "version-4-post-recovery" "overwrite = version-4"
check "$(rcli GET new:15)" "post-crash-15" "new:15 survived"
check "$(rcli GET new:30)" "post-crash-30" "new:30 survived"
check "$(rcli HGET myhash field-30)" "hashval-30" "hash field-30 survived 2 crashes"
check "$(rcli ZSCORE myzset zitem-5)" "5" "sorted set survived 2 crashes"

echo ""
echo "=== Phase 4: Rapid-fire writes + immediate crash ==="
for i in $(seq 1 500); do rcli SET "rapid:$i" "fast-$i" > /dev/null; done
echo "  500 rapid-fire keys written"

docker kill wal-node-a > /dev/null
echo "  node-a killed IMMEDIATELY after writes"
docker start wal-node-a > /dev/null
sleep 8
echo "  node-a restarted"

echo ""
echo "=== Phase 4 verification (rapid-fire survive) ==="
check "$(rcli GET rapid:1)" "fast-1" "rapid:1 survived"
check "$(rcli GET rapid:250)" "fast-250" "rapid:250 survived"
check "$(rcli GET rapid:500)" "fast-500" "rapid:500 survived"

echo ""
echo "=== Phase 5: Third crash (cumulative state integrity) ==="
docker kill wal-node-a > /dev/null
docker start wal-node-a > /dev/null
sleep 8

check "$(rcli GET counter:a)" "200" "counter still 200 after 3rd crash"
check "$(rcli GET rapid:500)" "fast-500" "rapid:500 after 3rd crash"
check "$(rcli HLEN myhash)" "30" "myhash still 30 after 3rd crash"
check "$(rcli ZCARD myzset)" "15" "myzset still 15 after 3rd crash"
check "$(rcli SCARD myset)" "25" "myset still 25 after 3rd crash"
check "$(rcli LLEN mylist)" "40" "mylist still 40 after 3rd crash"
check "$(rcli STRLEN large:key)" "10000" "large value after 3rd crash"

echo ""
echo "============================================"
echo "  Results: $PASS passed, $FAIL failed"
echo "============================================"

if [ $FAIL -gt 0 ]; then exit 1; fi
