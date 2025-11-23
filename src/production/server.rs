use super::{SharedRedisState, connection::ConnectionHandler, ttl_manager::TtlManager};
use tokio::net::TcpListener;
use tracing::{info, error};

pub struct ProductionRedisServer {
    addr: String,
}

impl ProductionRedisServer {
    pub fn new(addr: String) -> Self {
        ProductionRedisServer { addr }
    }
    
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // Create shared state
        let state = SharedRedisState::new();
        
        // Spawn TTL manager actor
        let ttl_manager = TtlManager::new(state.clone());
        tokio::spawn(async move {
            ttl_manager.run().await;
        });
        
        // Bind TCP listener
        let listener = TcpListener::bind(&self.addr).await?;
        info!("Redis server listening on {}", self.addr);
        
        // Accept connections and spawn actor for each
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let client_addr = addr.to_string();
                    let state_clone = state.clone();
                    
                    // Spawn connection handler actor
                    tokio::spawn(async move {
                        let handler = ConnectionHandler::new(
                            stream,
                            state_clone,
                            client_addr,
                        );
                        handler.run().await;
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }
}
