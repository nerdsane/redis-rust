# Redis Rust - Docker Deployment

Drop-in replacement for Redis with optional S3 persistence and shadow testing.

## Quick Start (Drop-in Replacement)

```bash
# Build and run - uses port 6379 like official Redis
docker-compose up -d

# Test with standard Redis tools
redis-cli -h localhost -p 6379
redis-benchmark -h localhost -p 6379
```

## Deployment Options

### 1. Simple Drop-in (LocalFs Persistence)

```bash
docker-compose up -d
```

This starts a single Redis-compatible server on port 6379 with local filesystem persistence.

### 2. S3 Persistence (with MinIO)

```bash
docker-compose -f docker-compose.production.yml up -d
```

Services:
- MinIO (S3-compatible) on ports 9000/9001
- Redis Rust on port 6379

### 3. Shadow Testing

Compare Redis Rust against official Redis in real-time:

```bash
docker-compose -f docker-compose.shadow.yml up -d
```

Services:
- Official Redis 7.4 on port 6379 (primary)
- Redis Rust on port 6380 (shadow)
- Shadow proxy on port 6381 (connect here)

```bash
# Connect to shadow proxy
redis-cli -p 6381

# Run benchmark through proxy
redis-benchmark -p 6381 -n 10000 -q

# All commands go to both servers, responses are compared
# Mismatches are logged in the shadow-proxy container
docker logs shadow-proxy
```

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `REDIS_PORT` | 6379 | Server port |
| `REDIS_STORE_TYPE` | localfs | `memory`, `localfs`, or `s3` |
| `REDIS_DATA_PATH` | /data | LocalFs persistence path |
| `REDIS_S3_BUCKET` | - | S3 bucket name |
| `REDIS_S3_ENDPOINT` | - | S3 endpoint (for MinIO) |
| `AWS_ACCESS_KEY_ID` | - | S3 credentials |
| `AWS_SECRET_ACCESS_KEY` | - | S3 credentials |

### Shadow Proxy Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SHADOW_LISTEN_PORT` | 6381 | Proxy port |
| `SHADOW_PRIMARY` | localhost:6379 | Primary Redis |
| `SHADOW_SECONDARY` | localhost:6380 | Shadow Redis |
| `SHADOW_LOG_MISMATCHES` | true | Log differences |
| `SHADOW_FAIL_ON_MISMATCH` | false | Error on mismatch |

## Files

| File | Description |
|------|-------------|
| `Dockerfile` | Drop-in replacement image |
| `Dockerfile.production` | Multi-binary production image |
| `Dockerfile.persistent` | Persistent server only |
| `Dockerfile.shadow` | Shadow proxy image |
| `docker-compose.yml` | Simple drop-in deployment |
| `docker-compose.production.yml` | Full stack with MinIO |
| `docker-compose.localfs.yml` | LocalFs persistence |
| `docker-compose.shadow.yml` | Shadow testing setup |

## Migrating from Redis

1. **Test with shadow proxy first:**
   ```bash
   docker-compose -f docker-compose.shadow.yml up -d
   # Point test traffic to port 6381
   # Monitor for mismatches
   ```

2. **Gradual rollout:**
   ```bash
   # Start Redis Rust alongside existing Redis
   docker run -p 6380:6379 redis-rust

   # Migrate read traffic first
   # Then write traffic
   ```

3. **Full replacement:**
   ```bash
   # Stop old Redis
   docker stop redis

   # Start Redis Rust on same port
   docker-compose up -d
   ```

## Supported Commands

All standard Redis commands are supported:
- String: GET, SET, MGET, MSET, INCR, DECR, APPEND
- Keys: DEL, EXISTS, KEYS, TTL, EXPIRE, EXPIREAT
- Server: PING, INFO, FLUSHDB, FLUSHALL

Not yet implemented:
- Pub/Sub
- Lua scripting
- Cluster mode
