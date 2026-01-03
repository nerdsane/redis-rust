FROM rust:1.83-slim AS builder

RUN apt-get update && apt-get install -y build-essential && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --bin redis-server-optimized

FROM debian:bookworm-slim

COPY --from=builder /app/target/release/redis-server-optimized /usr/local/bin/

EXPOSE 3000

CMD ["redis-server-optimized"]
