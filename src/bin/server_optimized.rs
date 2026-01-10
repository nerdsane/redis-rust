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
//!
//! ## TLS Configuration (requires `tls` feature)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | TLS_CERT_PATH | - | Path to server certificate (PEM) |
//! | TLS_KEY_PATH | - | Path to server private key (PEM) |
//! | TLS_CA_PATH | - | Path to CA certificate for client verification (optional) |
//! | TLS_REQUIRE_CLIENT_CERT | false | Require client certificates (mutual TLS) |
//!
//! ## ACL Configuration (requires `acl` feature)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | REDIS_REQUIRE_PASS | - | Simple password for AUTH command |
//! | ACL_FILE | - | Path to ACL configuration file |
//!
//! ## Datadog (when built with --features datadog)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | DD_SERVICE | redis-rust | Service name |
//! | DD_ENV | development | Environment |
//! | DD_DOGSTATSD_URL | 127.0.0.1:8125 | DogStatsD address |
//! | DD_TRACE_AGENT_URL | http://127.0.0.1:8126 | APM agent URL |

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use redis_sim::observability::{init_tracing, shutdown, DatadogConfig};
use redis_sim::production::{OptimizedRedisServer, ServerConfig};

const DEFAULT_PORT: u16 = 6379;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize observability (Datadog when feature enabled, basic tracing otherwise)
    let dd_config = DatadogConfig::from_env();
    init_tracing(&dd_config)?;

    // Load security configuration from environment
    let security_config = ServerConfig::from_env();

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
    #[cfg(feature = "datadog")]
    println!("  - Datadog observability enabled");
    println!();

    // Display security configuration
    println!("Security:");
    if security_config.tls_enabled() {
        #[cfg(feature = "tls")]
        {
            let tls = security_config.tls.as_ref().unwrap();
            println!("  - TLS: ENABLED");
            println!("    Certificate: {:?}", tls.cert_path);
            println!("    Key: {:?}", tls.key_path);
            if let Some(ca) = &tls.ca_path {
                println!("    CA: {:?}", ca);
            }
            if tls.require_client_cert {
                println!("    Client certificates: REQUIRED");
            }
        }
        #[cfg(not(feature = "tls"))]
        {
            println!("  - TLS: Configured but feature not enabled (build with --features tls)");
        }
    } else {
        println!("  - TLS: disabled");
    }

    if security_config.acl_enabled() {
        #[cfg(feature = "acl")]
        {
            println!("  - ACL: ENABLED");
            if security_config.acl.require_pass.is_some() {
                println!("    Authentication: password required");
            }
            if let Some(acl_file) = &security_config.acl.acl_file {
                println!("    ACL file: {:?}", acl_file);
            }
        }
        #[cfg(not(feature = "acl"))]
        {
            println!("  - ACL: Configured but feature not enabled (build with --features acl)");
        }
    } else {
        println!("  - ACL: disabled (no authentication)");
    }
    println!();

    server.run().await?;

    // Graceful shutdown
    shutdown();

    Ok(())
}
