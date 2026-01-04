//! Optimized Redis Server (Drop-in Replacement)
//!
//! High-performance Redis-compatible server without persistence.
//! Uses Redis standard port 6379 by default for drop-in replacement.
//!
//! ## Environment Variables
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | REDIS_PORT | 6379 | Server port (Redis default) |

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use redis_sim::production::OptimizedRedisServer;
use tracing_subscriber;

const DEFAULT_PORT: u16 = 6379;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let port = std::env::var("REDIS_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let addr = format!("0.0.0.0:{}", port);
    let server = OptimizedRedisServer::new(addr.clone());

    println!("Redis Rust Server (Drop-in Replacement)");
    println!("========================================");
    println!();
    println!("Listening on {}", addr);
    println!();
    println!("Performance optimizations:");
    println!("  - jemalloc custom allocator");
    println!("  - Actor-based shards (lock-free)");
    println!("  - Connection pooling");
    println!("  - Buffer pooling");
    println!();

    server.run().await?;

    Ok(())
}
