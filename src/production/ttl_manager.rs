use super::SharedRedisState;
use tokio::time::{interval, Duration};
use tracing::debug;

pub struct TtlManager {
    state: SharedRedisState,
}

impl TtlManager {
    pub fn new(state: SharedRedisState) -> Self {
        TtlManager { state }
    }
    
    pub async fn run(self) {
        let mut tick = interval(Duration::from_millis(100));
        
        loop {
            tick.tick().await;
            debug!("TTL manager checking for expired keys");
            self.state.evict_expired();
        }
    }
}
