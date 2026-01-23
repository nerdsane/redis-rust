FROM rust:1.83-slim AS builder

RUN apt-get update && apt-get install -y build-essential && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY benches ./benches

# Build the server-persistent binary for Kubernetes deployment
RUN cargo build --release --bin server-persistent

FROM debian:bookworm-slim

# Create non-root user for security
RUN useradd -r -u 1000 -g root redis && \
    mkdir -p /data && \
    chown redis:root /data

COPY --from=builder /app/target/release/server-persistent /usr/local/bin/redis-server

# Run as non-root
USER 1000

EXPOSE 3000 3001 7000 9090

# Health check via Kubernetes probes (no curl needed in image)
# See k8s/base/statefulset.yaml for livenessProbe/readinessProbe

CMD ["redis-server"]
