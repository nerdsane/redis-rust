# Redis Rust - Drop-in Replacement for Redis
#
# This Dockerfile creates an image that is compatible with the official
# redis Docker image. You can use it as a direct replacement:
#
#   # Instead of:
#   docker run -p 6379:6379 redis
#
#   # Use:
#   docker run -p 6379:6379 redis-rust
#
# All standard Redis tooling works:
#   redis-cli -h localhost -p 6379
#   redis-benchmark -h localhost -p 6379
#
# Environment Variables:
#   REDIS_PORT          - Server port (default: 6379)
#   REDIS_STORE_TYPE    - Storage: memory, localfs, s3 (default: localfs)
#   REDIS_DATA_PATH     - Data directory (default: /data)
#
# For S3 persistence:
#   REDIS_S3_BUCKET     - S3 bucket name
#   REDIS_S3_ENDPOINT   - S3 endpoint (for MinIO)
#   AWS_ACCESS_KEY_ID   - AWS credentials
#   AWS_SECRET_ACCESS_KEY

# Build stage
FROM rust:1.93-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./

# Dependency caching - build deps with dummy src/benches, then clean for real build
RUN mkdir -p src/bin benches && \
    echo "fn main() {}" > src/main.rs && \
    echo "fn main() {}" > src/bin/server_optimized.rs && \
    echo "fn main() {}" > src/bin/server_persistent.rs && \
    echo "fn main() {}" > benches/hot_paths.rs && \
    echo "" > src/lib.rs && \
    cargo build --release --features "s3,datadog" 2>/dev/null || true && \
    rm -rf src benches target/release/.fingerprint/redis-sim-* \
           target/release/server-persistent target/release/redis-server-optimized \
           target/release/deps/redis_sim-* target/release/deps/libredis_sim* \
           target/release/incremental/redis_sim-*

# Copy source and benches (benches needed to satisfy Cargo.toml manifest)
COPY src ./src
COPY benches ./benches
RUN cargo build --release --features "s3,datadog" \
    --bin redis-server-optimized \
    --bin server-persistent

# Runtime stage - matches official Redis image layout
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    netcat-openbsd \
    && rm -rf /var/lib/apt/lists/*

# Match Redis image: use redis user with UID 999
RUN groupadd -r -g 999 redis && useradd -r -g redis -u 999 redis

# Match Redis image: /data as working directory
RUN mkdir /data && chown redis:redis /data
WORKDIR /data

# Copy binaries
COPY --from=builder /app/target/release/redis-server-optimized /usr/local/bin/
COPY --from=builder /app/target/release/server-persistent /usr/local/bin/

# Create symlinks for Redis compatibility
RUN ln -s /usr/local/bin/server-persistent /usr/local/bin/redis-server

# Default environment - Redis compatible
ENV REDIS_PORT=6379 \
    REDIS_STORE_TYPE=localfs \
    REDIS_DATA_PATH=/data \
    AWS_REGION=us-east-1

USER redis

# Match Redis image: expose 6379
EXPOSE 6379

VOLUME /data

# Health check using Redis PING command (RESP format)
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD printf '*1\r\n$4\r\nPING\r\n' | nc -q1 localhost 6379 | grep -q "PONG" || exit 1

# Default: persistent server (drop-in for Redis with durability)
ENTRYPOINT ["server-persistent"]
