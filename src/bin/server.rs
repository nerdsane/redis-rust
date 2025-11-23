use redis_sim::production::ProductionRedisServer;
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Start production Redis server on port 6379
    let server = ProductionRedisServer::new("0.0.0.0:6379".to_string());
    
    println!("🚀 Redis Cache Server starting on 0.0.0.0:6379");
    println!("   Compatible with redis-cli and all Redis clients");
    println!("   Actor-based architecture for concurrent connections");
    println!("   Production-ready caching with 35+ commands");
    println!();
    
    server.run().await?;
    
    Ok(())
}
