use crate::redis::{Command, CommandExecutor, RespValue};
use crate::simulator::VirtualTime;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

const NUM_SHARDS: usize = 16;

#[derive(Debug)]
pub enum ShardMessage {
    Command {
        cmd: Command,
        virtual_time: VirtualTime,
        response_tx: oneshot::Sender<RespValue>,
    },
    /// Fire-and-forget batch command (no response needed)
    BatchCommand {
        cmd: Command,
        virtual_time: VirtualTime,
    },
    EvictExpired {
        virtual_time: VirtualTime,
        response_tx: oneshot::Sender<usize>,
    },
}

pub struct ShardActor {
    executor: CommandExecutor,
    rx: mpsc::UnboundedReceiver<ShardMessage>,
    shard_id: usize,
}

impl ShardActor {
    fn new(rx: mpsc::UnboundedReceiver<ShardMessage>, simulation_start_epoch: i64, shard_id: usize) -> Self {
        debug_assert!(shard_id < NUM_SHARDS, "Shard ID {} out of bounds", shard_id);
        let mut executor = CommandExecutor::new();
        executor.set_simulation_start_epoch(simulation_start_epoch);
        ShardActor { executor, rx, shard_id }
    }

    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ShardMessage::Command { cmd, virtual_time, response_tx } => {
                    self.executor.set_time(virtual_time);
                    let response = self.executor.execute(&cmd);
                    let _ = response_tx.send(response);
                }
                ShardMessage::BatchCommand { cmd, virtual_time } => {
                    // Fire-and-forget: execute without sending response
                    self.executor.set_time(virtual_time);
                    let _ = self.executor.execute(&cmd);
                }
                ShardMessage::EvictExpired { virtual_time, response_tx } => {
                    let evicted = self.executor.evict_expired_direct(virtual_time);
                    let _ = response_tx.send(evicted);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ShardHandle {
    tx: mpsc::UnboundedSender<ShardMessage>,
    shard_id: usize,
}

impl ShardHandle {
    #[inline]
    async fn execute(&self, cmd: Command, virtual_time: VirtualTime) -> RespValue {
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::Command {
            cmd,
            virtual_time,
            response_tx,
        };

        if self.tx.send(msg).is_err() {
            debug_assert!(false, "Shard {} channel closed unexpectedly", self.shard_id);
            return RespValue::Error("ERR shard unavailable".to_string());
        }

        response_rx.await.unwrap_or_else(|_| {
            debug_assert!(false, "Shard {} response channel dropped", self.shard_id);
            RespValue::Error("ERR shard response failed".to_string())
        })
    }

    /// Fire-and-forget execution - no response channel allocation
    #[inline]
    fn execute_fire_and_forget(&self, cmd: Command, virtual_time: VirtualTime) {
        let msg = ShardMessage::BatchCommand { cmd, virtual_time };
        let _ = self.tx.send(msg);
    }

    #[inline]
    async fn evict_expired(&self, virtual_time: VirtualTime) -> usize {
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::EvictExpired {
            virtual_time,
            response_tx,
        };

        if self.tx.send(msg).is_err() {
            return 0;
        }

        response_rx.await.unwrap_or(0)
    }
}

#[inline]
fn hash_key(key: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % NUM_SHARDS;
    debug_assert!(idx < NUM_SHARDS, "Hash produced invalid shard index");
    idx
}

#[derive(Clone)]
pub struct ShardedActorState {
    shards: Arc<[ShardHandle; NUM_SHARDS]>,
    start_time: SystemTime,
}

impl ShardedActorState {
    pub fn new() -> Self {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time before UNIX epoch")
            .as_secs() as i64;

        let shards: [ShardHandle; NUM_SHARDS] = std::array::from_fn(|shard_id| {
            let (tx, rx) = mpsc::unbounded_channel();
            let actor = ShardActor::new(rx, epoch, shard_id);
            tokio::spawn(actor.run());
            ShardHandle { tx, shard_id }
        });

        ShardedActorState {
            shards: Arc::new(shards),
            start_time: SystemTime::now(),
        }
    }

    #[inline]
    fn get_current_virtual_time(&self) -> VirtualTime {
        let elapsed = self.start_time.elapsed().expect("System time went backwards");
        VirtualTime::from_millis(elapsed.as_millis() as u64)
    }

    pub async fn evict_expired_all_shards(&self) -> usize {
        let virtual_time = self.get_current_virtual_time();
        let mut total = 0usize;

        for shard in self.shards.iter() {
            total = total.saturating_add(shard.evict_expired(virtual_time).await);
        }

        total
    }

    pub async fn execute(&self, cmd: &Command) -> RespValue {
        let virtual_time = self.get_current_virtual_time();

        match cmd {
            Command::Ping => RespValue::SimpleString("PONG".to_string()),

            Command::Info => {
                let info = format!(
                    "# Server\r\n\
                     redis_mode:tiger_style\r\n\
                     num_shards:{}\r\n\
                     architecture:actor_message_passing\r\n\
                     allocator:jemalloc\r\n\
                     \r\n\
                     # Stats\r\n\
                     current_time_ms:{}\r\n",
                    NUM_SHARDS,
                    virtual_time.as_millis()
                );
                RespValue::BulkString(Some(info.into_bytes()))
            }

            Command::FlushDb | Command::FlushAll => {
                let mut futures = Vec::with_capacity(NUM_SHARDS);
                for shard in self.shards.iter() {
                    futures.push(shard.execute(Command::FlushDb, virtual_time));
                }
                for future in futures {
                    let _ = future.await;
                }
                RespValue::SimpleString("OK".to_string())
            }

            Command::Keys(pattern) => {
                let mut futures = Vec::with_capacity(NUM_SHARDS);
                for shard in self.shards.iter() {
                    futures.push(shard.execute(Command::Keys(pattern.clone()), virtual_time));
                }

                let mut all_keys: Vec<RespValue> = Vec::new();
                for future in futures {
                    if let RespValue::Array(Some(keys)) = future.await {
                        all_keys.extend(keys);
                    }
                }
                RespValue::Array(Some(all_keys))
            }

            Command::MGet(keys) => {
                let futures: Vec<_> = keys.iter().map(|key| {
                    let shard_idx = hash_key(key);
                    self.shards[shard_idx].execute(Command::Get(key.clone()), virtual_time)
                }).collect();

                // Execute all GET operations concurrently
                let results = futures::future::join_all(futures).await;
                RespValue::Array(Some(results))
            }

            Command::MSet(pairs) => {
                // Group key-value pairs by target shard
                let mut shard_batches: [Vec<(String, crate::redis::SDS)>; NUM_SHARDS] = Default::default();
                for (key, value) in pairs {
                    let shard_idx = hash_key(key);
                    shard_batches[shard_idx].push((key.clone(), value.clone()));
                }

                // Collect non-empty batches with their shard indices
                let batches: Vec<_> = shard_batches.into_iter()
                    .enumerate()
                    .filter(|(_, b)| !b.is_empty())
                    .collect();

                if batches.is_empty() {
                    return RespValue::SimpleString("OK".to_string());
                }

                // For single-shard MSET (common case), execute directly
                if batches.len() == 1 {
                    let (shard_idx, batch) = batches.into_iter().next().unwrap();
                    self.shards[shard_idx].execute(Command::BatchSet(batch), virtual_time).await;
                } else {
                    // Multi-shard: send all concurrently
                    let futures: Vec<_> = batches.into_iter().map(|(shard_idx, batch)| {
                        self.shards[shard_idx].execute(Command::BatchSet(batch), virtual_time)
                    }).collect();
                    futures::future::join_all(futures).await;
                }

                RespValue::SimpleString("OK".to_string())
            }

            Command::Exists(keys) => {
                let futures: Vec<_> = keys.iter().map(|key| {
                    let shard_idx = hash_key(key);
                    self.shards[shard_idx].execute(Command::Exists(vec![key.clone()]), virtual_time)
                }).collect();

                // Execute all EXISTS operations concurrently
                let results = futures::future::join_all(futures).await;
                let count: i64 = results.into_iter().filter_map(|r| {
                    if let RespValue::Integer(n) = r { Some(n) } else { None }
                }).sum();
                RespValue::Integer(count)
            }

            _ => {
                if let Some(key) = cmd.get_primary_key() {
                    let shard_idx = hash_key(key);
                    debug_assert!(shard_idx < NUM_SHARDS, "Invalid shard index for key");
                    self.shards[shard_idx].execute(cmd.clone(), virtual_time).await
                } else {
                    self.shards[0].execute(cmd.clone(), virtual_time).await
                }
            }
        }
    }
}

impl Default for ShardedActorState {
    fn default() -> Self {
        Self::new()
    }
}
