use crate::redis::CommandExecutor;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::simulator::VirtualTime;

#[derive(Clone)]
pub struct SharedRedisState {
    executor: Arc<RwLock<CommandExecutor>>,
    start_time: SystemTime,
}

impl SharedRedisState {
    pub fn new() -> Self {
        let mut executor = CommandExecutor::new();
        
        // Set simulation_start_epoch to current Unix timestamp
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        executor.set_simulation_start_epoch(epoch);
        
        SharedRedisState {
            executor: Arc::new(RwLock::new(executor)),
            start_time: SystemTime::now(),
        }
    }
    
    pub fn with_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut CommandExecutor) -> R,
    {
        let mut executor = self.executor.write();
        
        // Update virtual time based on real elapsed time
        let elapsed = self.start_time.elapsed().unwrap();
        let virtual_time = VirtualTime::from_millis(elapsed.as_millis() as u64);
        executor.set_time(virtual_time);
        
        f(&mut executor)
    }
    
    pub fn evict_expired(&self) {
        self.with_lock(|executor| {
            // This triggers automatic expiration cleanup
            executor.execute(&crate::redis::Command::Ping);
        });
    }
}
