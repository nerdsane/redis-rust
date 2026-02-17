#![allow(unused_imports)]
//! Redis Server with Streaming Persistence
//!
//! A Redis-compatible server that persists data to object store using
//! streaming delta writes. Supports recovery on startup.
//!
//! ## Environment Variables
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | REDIS_PORT | 3000 | Server port |
//! | REDIS_STORE_TYPE | localfs | memory, localfs, or s3 |
//! | REDIS_DATA_PATH | /data | LocalFs path |
//! | REDIS_S3_BUCKET | - | S3 bucket name |
//! | REDIS_S3_PREFIX | redis-stream | S3 key prefix |
//! | REDIS_S3_ENDPOINT | - | MinIO endpoint URL |
//! | AWS_ACCESS_KEY_ID | - | S3 credentials |
//! | AWS_SECRET_ACCESS_KEY | - | S3 credentials |
//! | AWS_REGION | us-east-1 | S3 region |
//!
//! ## Kubernetes Clustering
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | POD_NAME | - | StatefulSet pod name (e.g., redis-rust-0) |
//! | POD_NAMESPACE | default | Kubernetes namespace |
//! | REPLICATION_ENABLED | false | Enable gossip-based replication |
//! | GOSSIP_PORT | 7000 | Port for gossip protocol |
//! | CLUSTER_SIZE | 3 | Number of replicas in StatefulSet |
//! | SERVICE_NAME | redis-rust-headless | Headless service name |
//!
//! ## WAL (Write-Ahead Log)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | REDIS_WAL_ENABLED | false | Enable WAL persistence |
//! | REDIS_WAL_DIR | /tmp/redis-wal | WAL file directory |
//! | REDIS_WAL_FSYNC | everysec | Fsync policy: always, everysec, no |
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

use bytes::{BufMut, BytesMut};
use parking_lot::RwLock;
use redis_sim::observability::{init_tracing, shutdown, DatadogConfig};
use redis_sim::production::{GossipManager, ReplicatedShardedState};
use redis_sim::redis::{Command, RespCodec, RespValue};
use redis_sim::replication::{ConsistencyLevel, GossipState, ReplicationConfig};
use redis_sim::streaming::{
    create_integration, ObjectStoreType, StreamingConfig, StreamingIntegrationTrait, WorkerHandles,
};
use redis_sim::streaming::wal_config::{FsyncPolicy, WalConfig};
use redis_sim::streaming::wal_store::LocalWalStore;
use redis_sim::streaming::wal_actor::spawn_wal_actor;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tracing::{debug, error, info, warn};

// Redis-compatible defaults for drop-in replacement
const DEFAULT_PORT: u16 = 6379;
const DEFAULT_REPLICA_ID: u64 = 1;
const DEFAULT_DATA_PATH: &str = "/data";
const DEFAULT_S3_PREFIX: &str = "redis-stream";
#[cfg(feature = "s3")]
const DEFAULT_S3_REGION: &str = "us-east-1";
const DEFAULT_GOSSIP_PORT: u16 = 7000;
const DEFAULT_CLUSTER_SIZE: usize = 3;
const DEFAULT_SERVICE_NAME: &str = "redis-rust-headless";

// TigerStyle: Explicit limits with _MAX suffix
const CLUSTER_SIZE_MAX: usize = 100;
const REPLICA_ID_MAX: u64 = 99;
const GOSSIP_INTERVAL_MS_MIN: u64 = 10;
const GOSSIP_INTERVAL_MS_MAX: u64 = 10_000;

/// Kubernetes cluster configuration
///
/// TigerStyle Invariants:
/// - replica_id < cluster_size (replica must be valid member)
/// - peers.len() == cluster_size - 1 (all peers except self)
/// - cluster_size <= CLUSTER_SIZE_MAX
/// - gossip_interval_ms in [GOSSIP_INTERVAL_MS_MIN, GOSSIP_INTERVAL_MS_MAX]
struct ClusterConfig {
    /// Whether replication is enabled
    enabled: bool,
    /// Replica ID (0-indexed, derived from StatefulSet ordinal)
    replica_id: u64,
    /// Gossip port for inter-pod communication
    gossip_port: u16,
    /// Number of replicas in the cluster
    cluster_size: usize,
    /// Peer addresses for gossip (built from headless service DNS)
    peers: Vec<String>,
    /// Gossip interval in milliseconds
    gossip_interval_ms: u64,
}

impl ClusterConfig {
    /// Build cluster configuration from environment variables
    ///
    /// TigerStyle: Explicit validation with debug_assert for invariants
    fn from_env() -> Self {
        let enabled = std::env::var("REPLICATION_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);

        let gossip_port = std::env::var("GOSSIP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_GOSSIP_PORT);

        let cluster_size = std::env::var("CLUSTER_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_CLUSTER_SIZE)
            .min(CLUSTER_SIZE_MAX); // TigerStyle: Enforce limit

        let gossip_interval_ms = std::env::var("GOSSIP_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100)
            .clamp(GOSSIP_INTERVAL_MS_MIN, GOSSIP_INTERVAL_MS_MAX); // TigerStyle: Clamp to valid range

        // Parse replica ID from POD_NAME (e.g., "redis-rust-0" -> 0)
        let replica_id = Self::parse_replica_id_from_env().min(REPLICA_ID_MAX);

        // Build peer list from Kubernetes DNS
        let peers = if enabled {
            Self::build_peer_list(replica_id, cluster_size, gossip_port)
        } else {
            vec![]
        };

        let config = ClusterConfig {
            enabled,
            replica_id,
            gossip_port,
            cluster_size,
            peers,
            gossip_interval_ms,
        };

        // TigerStyle: Verify invariants in debug builds
        config.verify_invariants();
        config
    }

    /// TigerStyle: Verify all struct invariants hold
    #[inline]
    fn verify_invariants(&self) {
        debug_assert!(
            self.cluster_size <= CLUSTER_SIZE_MAX,
            "Invariant: cluster_size {} exceeds max {}",
            self.cluster_size,
            CLUSTER_SIZE_MAX
        );
        debug_assert!(
            self.replica_id <= REPLICA_ID_MAX,
            "Invariant: replica_id {} exceeds max {}",
            self.replica_id,
            REPLICA_ID_MAX
        );
        debug_assert!(
            (self.replica_id as usize) < self.cluster_size || !self.enabled,
            "Invariant: replica_id {} must be < cluster_size {} when enabled",
            self.replica_id,
            self.cluster_size
        );
        debug_assert!(
            self.gossip_interval_ms >= GOSSIP_INTERVAL_MS_MIN,
            "Invariant: gossip_interval_ms {} below min {}",
            self.gossip_interval_ms,
            GOSSIP_INTERVAL_MS_MIN
        );
        debug_assert!(
            self.gossip_interval_ms <= GOSSIP_INTERVAL_MS_MAX,
            "Invariant: gossip_interval_ms {} exceeds max {}",
            self.gossip_interval_ms,
            GOSSIP_INTERVAL_MS_MAX
        );
        if self.enabled {
            debug_assert!(
                self.peers.len() == self.cluster_size - 1,
                "Invariant: peers.len() {} must equal cluster_size - 1 ({})",
                self.peers.len(),
                self.cluster_size - 1
            );
        }
    }

    /// Parse replica ID from POD_NAME environment variable
    /// Example: "redis-rust-0" -> 0, "redis-rust-2" -> 2
    fn parse_replica_id_from_env() -> u64 {
        if let Ok(pod_name) = std::env::var("POD_NAME") {
            // StatefulSet pods are named: <statefulset-name>-<ordinal>
            // Extract the ordinal from the end
            if let Some(ordinal_str) = pod_name.rsplit('-').next() {
                if let Ok(ordinal) = ordinal_str.parse::<u64>() {
                    return ordinal;
                }
            }
        }
        // Fall back to REPLICA_ID env var or default
        std::env::var("REPLICA_ID")
            .ok()
            .and_then(|s| {
                // Handle StatefulSet pod name format in REPLICA_ID too
                if let Some(ordinal_str) = s.rsplit('-').next() {
                    ordinal_str.parse::<u64>().ok()
                } else {
                    s.parse::<u64>().ok()
                }
            })
            .unwrap_or(DEFAULT_REPLICA_ID)
    }

    /// Build peer list from Kubernetes headless service DNS
    /// DNS format: <pod-name>.<service-name>.<namespace>.svc.cluster.local
    fn build_peer_list(my_replica_id: u64, cluster_size: usize, gossip_port: u16) -> Vec<String> {
        let service_name =
            std::env::var("SERVICE_NAME").unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_string());
        let namespace = std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "default".to_string());
        let statefulset_name = std::env::var("POD_NAME")
            .map(|pod| {
                // Extract StatefulSet name by removing the ordinal suffix
                // "redis-rust-0" -> "redis-rust"
                let parts: Vec<&str> = pod.rsplitn(2, '-').collect();
                if parts.len() == 2 {
                    parts[1].to_string()
                } else {
                    "redis-rust".to_string()
                }
            })
            .unwrap_or_else(|_| "redis-rust".to_string());

        let mut peers = Vec::with_capacity(cluster_size - 1);
        for i in 0..cluster_size {
            let peer_id = i as u64;
            if peer_id != my_replica_id {
                // Kubernetes DNS: <pod-name>.<headless-svc>.<namespace>.svc.cluster.local
                let peer_addr = format!(
                    "{}-{}.{}.{}.svc.cluster.local:{}",
                    statefulset_name, i, service_name, namespace, gossip_port
                );
                peers.push(peer_addr);
            }
        }
        peers
    }

    /// Convert to ReplicationConfig
    fn to_replication_config(&self) -> ReplicationConfig {
        ReplicationConfig {
            enabled: self.enabled,
            replica_id: self.replica_id,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: self.gossip_interval_ms,
            peers: self.peers.clone(),
            replication_factor: self.cluster_size,
            partitioned_mode: false,
            selective_gossip: false,
            virtual_nodes_per_physical: 150,
        }
    }
}

/// Gossip listener for receiving deltas from peers
///
/// TigerStyle: This is the receiving half of the gossip protocol.
/// - Binds to configured gossip port (not the hardcoded GossipManager port)
/// - Handles framed messages (4-byte length prefix + JSON payload)
/// - Applies received deltas via CRDT merge (idempotent)
///
/// Message framing: [4 bytes big-endian length][JSON payload]
async fn start_gossip_listener(
    port: u16,
    state: Arc<ReplicatedShardedState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::AsyncReadExt;

    // TigerStyle: Explicit limit
    const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB

    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("Gossip listener started on {}", addr);

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        info!("Gossip connection from {}", peer_addr);

        let state_clone = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_gossip_connection(stream, state_clone, MAX_MESSAGE_SIZE).await {
                warn!("Gossip connection error from {}: {}", peer_addr, e);
            }
        });
    }
}

/// Handle a single gossip connection from a peer
///
/// TigerStyle: Explicit error handling, bounded message sizes
async fn handle_gossip_connection(
    mut stream: TcpStream,
    state: Arc<ReplicatedShardedState>,
    max_message_size: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use redis_sim::replication::gossip::GossipMessage;
    use tokio::io::AsyncReadExt;

    loop {
        // Read 4-byte length prefix
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            // Connection closed
            break;
        }
        let msg_len = u32::from_be_bytes(len_buf) as usize;

        // TigerStyle: Explicit bounds check
        if msg_len > max_message_size {
            warn!(
                "Gossip message too large: {} bytes (max {})",
                msg_len, max_message_size
            );
            break;
        }

        // Read message payload
        let mut msg_buf = vec![0u8; msg_len];
        if stream.read_exact(&mut msg_buf).await.is_err() {
            break;
        }

        // Deserialize and process
        match GossipMessage::deserialize(&msg_buf) {
            Ok(msg) => {
                let source = msg.source_replica();
                if let Some(deltas) = msg.into_deltas() {
                    if !deltas.is_empty() {
                        debug!("Received {} deltas from replica {}", deltas.len(), source.0);
                        // Apply via CRDT merge (idempotent operation)
                        state.apply_remote_deltas(deltas);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to deserialize gossip message: {}", e);
            }
        }
    }

    Ok(())
}

/// Server configuration from environment variables
struct Config {
    port: u16,
    store_type: String,
    data_path: PathBuf,
    #[cfg(feature = "s3")]
    s3_bucket: Option<String>,
    #[cfg(feature = "s3")]
    s3_prefix: String,
    #[cfg(feature = "s3")]
    s3_endpoint: Option<String>,
    #[cfg(feature = "s3")]
    s3_region: String,
}

impl Config {
    fn from_env() -> Self {
        Config {
            port: std::env::var("REDIS_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            store_type: std::env::var("REDIS_STORE_TYPE")
                .unwrap_or_else(|_| "localfs".to_string())
                .to_lowercase(),
            data_path: std::env::var("REDIS_DATA_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_DATA_PATH)),
            #[cfg(feature = "s3")]
            s3_bucket: std::env::var("REDIS_S3_BUCKET").ok(),
            #[cfg(feature = "s3")]
            s3_prefix: std::env::var("REDIS_S3_PREFIX")
                .unwrap_or_else(|_| DEFAULT_S3_PREFIX.to_string()),
            #[cfg(feature = "s3")]
            s3_endpoint: std::env::var("REDIS_S3_ENDPOINT").ok(),
            #[cfg(feature = "s3")]
            s3_region: std::env::var("AWS_REGION")
                .unwrap_or_else(|_| DEFAULT_S3_REGION.to_string()),
        }
    }

    fn wal_config_from_env() -> Option<WalConfig> {
        let enabled = std::env::var("REDIS_WAL_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);

        if !enabled {
            return None;
        }

        let wal_dir = std::env::var("REDIS_WAL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/redis-wal"));

        let fsync_policy = match std::env::var("REDIS_WAL_FSYNC")
            .unwrap_or_else(|_| "everysec".to_string())
            .to_lowercase()
            .as_str()
        {
            "always" => FsyncPolicy::Always,
            "no" | "none" => FsyncPolicy::No,
            _ => FsyncPolicy::EverySecond,
        };

        Some(WalConfig {
            enabled: true,
            wal_dir,
            fsync_policy,
            ..WalConfig::default()
        })
    }

    fn to_streaming_config(&self) -> Result<StreamingConfig, String> {
        match self.store_type.as_str() {
            "memory" => Ok(StreamingConfig::test()),
            "localfs" => Ok(StreamingConfig {
                enabled: true,
                store_type: ObjectStoreType::LocalFs,
                prefix: DEFAULT_S3_PREFIX.to_string(),
                local_path: Some(self.data_path.clone()),
                #[cfg(feature = "s3")]
                s3: None,
                write_buffer: redis_sim::streaming::WriteBufferConfig::default(),
                checkpoint: redis_sim::streaming::config::CheckpointConfig::default(),
                compaction: redis_sim::streaming::config::CompactionConfig::default(),
                wal: Self::wal_config_from_env(),
            }),
            #[cfg(feature = "s3")]
            "s3" => {
                let bucket = self
                    .s3_bucket
                    .clone()
                    .ok_or("REDIS_S3_BUCKET required for S3 store type".to_string())?;
                Ok(StreamingConfig {
                    enabled: true,
                    store_type: ObjectStoreType::S3,
                    prefix: self.s3_prefix.clone(),
                    local_path: None,
                    s3: Some(redis_sim::streaming::S3Config {
                        bucket,
                        prefix: self.s3_prefix.clone(),
                        region: self.s3_region.clone(),
                        endpoint: self.s3_endpoint.clone(),
                    }),
                    write_buffer: redis_sim::streaming::WriteBufferConfig::default(),
                    checkpoint: redis_sim::streaming::config::CheckpointConfig::default(),
                    compaction: redis_sim::streaming::config::CompactionConfig::default(),
                    wal: Self::wal_config_from_env(),
                })
            }
            #[cfg(not(feature = "s3"))]
            "s3" => Err("S3 support not compiled. Rebuild with --features s3".to_string()),
            other => Err(format!(
                "Unknown store type: {}. Use 'memory', 'localfs', or 's3'",
                other
            )),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize observability (Datadog when feature enabled, basic tracing otherwise)
    let dd_config = DatadogConfig::from_env();
    init_tracing(&dd_config)?;

    // Load configuration from environment
    let config = Config::from_env();
    let streaming_config = config.to_streaming_config()?;

    println!("Redis Server with Streaming Persistence");
    println!("========================================");
    println!();
    println!("Configuration:");
    println!("  Port: {}", config.port);
    println!("  Store: {}", config.store_type);
    match config.store_type.as_str() {
        "localfs" => println!("  Path: {}", config.data_path.display()),
        #[cfg(feature = "s3")]
        "s3" => {
            println!(
                "  Bucket: {}",
                config.s3_bucket.as_deref().unwrap_or("(not set)")
            );
            println!("  Prefix: {}", config.s3_prefix);
            if let Some(endpoint) = &config.s3_endpoint {
                println!("  Endpoint: {}", endpoint);
            }
        }
        _ => {}
    }
    #[cfg(feature = "datadog")]
    println!("  Datadog observability enabled");

    // Load cluster configuration from Kubernetes environment
    let cluster_config = ClusterConfig::from_env();
    let repl_config = cluster_config.to_replication_config();

    println!();
    println!("Cluster Configuration:");
    println!(
        "  Replication: {}",
        if cluster_config.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("  Replica ID: {}", cluster_config.replica_id);
    if cluster_config.enabled {
        println!("  Gossip Port: {}", cluster_config.gossip_port);
        println!("  Cluster Size: {}", cluster_config.cluster_size);
        println!("  Peers: {:?}", cluster_config.peers);
    }
    println!();

    // Create state with replication config
    let mut state = ReplicatedShardedState::new(repl_config.clone());

    // Ensure persistence directory exists for localfs
    if config.store_type == "localfs" {
        std::fs::create_dir_all(&config.data_path)?;
    }

    // Save WAL config before streaming_config is consumed by create_integration
    let wal_config = streaming_config.wal.clone();

    // Create integration and perform recovery
    let integration = create_integration(streaming_config, DEFAULT_REPLICA_ID).await?;

    info!("Checking for existing data to recover...");
    let stats = integration.recover(&state).await?;
    if stats.segments_loaded > 0 {
        info!(
            "Recovered {} segments, {} deltas, {} keys",
            stats.segments_loaded,
            stats.deltas_replayed,
            state.key_count().await
        );
    } else {
        info!("Starting fresh (no existing data)");
    }

    // WAL replay: apply entries not yet in object store
    if let Some(ref wc) = wal_config {
        if wc.enabled {
            use redis_sim::streaming::wal::WalRotator;
            std::fs::create_dir_all(&wc.wal_dir)?;
            let replay_store = LocalWalStore::new(wc.wal_dir.clone())
                .map_err(|e| format!("Failed to open WAL for replay: {}", e))?;
            let replay_rotator = WalRotator::new(replay_store, wc.max_file_size)
                .map_err(|e| format!("Failed to create WAL rotator for replay: {}", e))?;

            // High-water mark: max timestamp from object store segments
            // (stats doesn't expose this directly, so we replay all WAL entries;
            // CRDT idempotency makes duplicate replay safe)
            let wal_entries = replay_rotator.recover_all_entries()
                .map_err(|e| format!("WAL replay failed: {}", e))?;

            if !wal_entries.is_empty() {
                let mut deltas = Vec::with_capacity(wal_entries.len());
                for entry in &wal_entries {
                    match entry.to_delta() {
                        Ok(delta) => deltas.push(delta),
                        Err(e) => {
                            warn!("Skipping corrupt WAL entry: {}", e);
                        }
                    }
                }
                info!("WAL replay: {} entries recovered from local WAL", deltas.len());
                state.apply_recovered_state(None, deltas);
                info!("After WAL replay: {} keys", state.key_count().await);
            } else {
                info!("WAL replay: no entries to replay");
            }
        }
    }

    let (worker_handles, sender) = integration.start_workers().await?;

    // Connect delta sink BEFORE wrapping state in Arc
    state.set_delta_sink(sender);

    // Start WAL actor if enabled
    let wal_task = if let Some(ref wc) = wal_config {
        if wc.enabled {
            std::fs::create_dir_all(&wc.wal_dir)?;
            let wal_store = LocalWalStore::new(wc.wal_dir.clone())
                .map_err(|e| format!("Failed to create WAL store: {}", e))?;
            let (wal_handle, wal_join) = spawn_wal_actor(wal_store, wc.clone())
                .map_err(|e| format!("Failed to spawn WAL actor: {}", e))?;

            // Start periodic sync tick for EverySecond mode
            if wc.fsync_policy == FsyncPolicy::EverySecond {
                let tick_handle = wal_handle.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                    loop {
                        interval.tick().await;
                        tick_handle.sync_tick();
                    }
                });
            }

            info!(
                "WAL enabled: dir={}, fsync={:?}",
                wc.wal_dir.display(),
                wc.fsync_policy
            );
            println!("  WAL: enabled ({:?} fsync)", wc.fsync_policy);

            state.set_wal_handle(wal_handle.clone());
            Some((wal_handle, wal_join))
        } else {
            None
        }
    } else {
        None
    };

    let state = Arc::new(state);

    // Start gossip server and loop if replication is enabled
    if cluster_config.enabled {
        info!(
            "Starting gossip server on port {} with {} peers",
            cluster_config.gossip_port,
            cluster_config.peers.len()
        );

        // Get the gossip state from ReplicatedShardedState (this is where deltas are queued)
        let gossip_state = state
            .get_gossip_state()
            .expect("ReplicatedShardedState should have gossip state when replication is enabled");

        // Start custom gossip server on configured port (not the hardcoded GossipManager port)
        let gossip_port = cluster_config.gossip_port;
        let state_for_gossip = state.clone();
        tokio::spawn(async move {
            if let Err(e) = start_gossip_listener(gossip_port, state_for_gossip).await {
                error!("Gossip server error: {}", e);
            }
        });

        // Start gossip loop (sends deltas to peers)
        // The loop drains outbound messages from the shared gossip state
        let loop_config = repl_config.clone();
        let loop_state = gossip_state.clone();
        tokio::spawn(async move {
            GossipManager::start_gossip_loop(loop_config, loop_state, move || {
                // Deltas are already queued in gossip_state by state.execute()
                // The gossip loop drains them via drain_outbound()
                // Return empty here since queue_deltas is called elsewhere
                vec![]
            })
            .await;
        });

        info!("Gossip replication started");
    }

    println!("Starting server...");
    println!();

    // Start health check server on port+1
    let health_port = config.port + 1;
    let health_addr = format!("0.0.0.0:{}", health_port);
    let health_listener = TcpListener::bind(&health_addr).await?;
    info!("Health check listening on {}", health_addr);

    // Start main TCP listener
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = TcpListener::bind(&addr).await?;

    info!("Server listening on {}", addr);
    println!("Server listening on {}", addr);
    println!("Health check on {}", health_addr);
    println!("Press Ctrl+C to shutdown gracefully");
    println!();

    // Accept connections until shutdown
    loop {
        tokio::select! {
            // Main Redis connections
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, state).await {
                                error!("Connection error from {}: {}", addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {}", e);
                    }
                }
            }
            // Health check connections
            result = health_listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        tokio::spawn(async move {
                            if let Err(e) = handle_health_check(stream).await {
                                warn!("Health check error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Health check accept error: {}", e);
                    }
                }
            }
            _ = signal::ctrl_c() => {
                info!("Shutdown signal received");
                println!("\nShutdown signal received, flushing data...");
                break;
            }
        }
    }

    // Graceful shutdown — WAL first (closest to write path), then streaming
    if let Some((wal_handle, wal_join)) = wal_task {
        info!("Shutting down WAL actor (final fsync)...");
        wal_handle.shutdown().await;
        let _ = wal_join.await;
        info!("WAL actor shutdown complete");
    }

    info!("Shutting down streaming persistence workers...");
    worker_handles.shutdown().await;

    // Shutdown observability (flush pending spans/metrics)
    shutdown();

    println!("Server shutdown complete");
    info!("Server shutdown complete");

    Ok(())
}

async fn handle_health_check(
    mut stream: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Read the HTTP request (we don't care about the content)
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf).await?;

    // Send a simple HTTP 200 OK response
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nOK";
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;

    Ok(())
}

async fn handle_connection(
    mut stream: TcpStream,
    state: Arc<ReplicatedShardedState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Enable TCP_NODELAY for lower latency
    let _ = stream.set_nodelay(true);

    let mut read_buf = [0u8; 8192];
    let mut buffer = BytesMut::with_capacity(4096);
    let mut write_buffer = BytesMut::with_capacity(4096);

    loop {
        let n = stream.read(&mut read_buf).await?;
        if n == 0 {
            break;
        }

        buffer.extend_from_slice(&read_buf[..n]);

        // Process all available commands (pipelining support)
        loop {
            match RespCodec::parse(&mut buffer) {
                Ok(Some(resp_value)) => match Command::from_resp_zero_copy(&resp_value) {
                    Ok(cmd) => {
                        let response = state.execute(cmd).await;
                        encode_resp_into(&response, &mut write_buffer);
                    }
                    Err(e) => {
                        encode_error_into(&e, &mut write_buffer);
                    }
                },
                Ok(None) => break, // Need more data
                Err(e) => {
                    encode_error_into(&format!("protocol error: {}", e), &mut write_buffer);
                    buffer.clear();
                    break;
                }
            }
        }

        // Flush all responses
        if !write_buffer.is_empty() {
            stream.write_all(&write_buffer).await?;
            stream.flush().await?;
            write_buffer.clear();
        }
    }

    Ok(())
}

fn encode_resp_into(value: &RespValue, buf: &mut BytesMut) {
    match value {
        RespValue::SimpleString(s) => {
            buf.put_u8(b'+');
            buf.extend_from_slice(s.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::Error(s) => {
            buf.put_u8(b'-');
            buf.extend_from_slice(s.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::Integer(n) => {
            buf.put_u8(b':');
            buf.extend_from_slice(n.to_string().as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::BulkString(None) => {
            buf.extend_from_slice(b"$-1\r\n");
        }
        RespValue::BulkString(Some(data)) => {
            buf.put_u8(b'$');
            buf.extend_from_slice(data.len().to_string().as_bytes());
            buf.extend_from_slice(b"\r\n");
            buf.extend_from_slice(data);
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::Array(None) => {
            buf.extend_from_slice(b"*-1\r\n");
        }
        RespValue::Array(Some(elements)) => {
            buf.put_u8(b'*');
            buf.extend_from_slice(elements.len().to_string().as_bytes());
            buf.extend_from_slice(b"\r\n");
            for elem in elements {
                encode_resp_into(elem, buf);
            }
        }
    }
}

fn encode_error_into(msg: &str, buf: &mut BytesMut) {
    buf.put_u8(b'-');
    buf.extend_from_slice(b"ERR ");
    buf.extend_from_slice(msg.as_bytes());
    buf.extend_from_slice(b"\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Test parsing replica ID from StatefulSet pod names
    /// Invariant: Replica ID must be correctly extracted from pod name
    #[test]
    fn test_parse_replica_id_from_pod_name() {
        let _guard = ENV_LOCK.lock().unwrap();

        // Clean up any leftover env vars
        std::env::remove_var("POD_NAME");
        std::env::remove_var("REPLICA_ID");

        // Test: redis-rust-0 -> 0
        std::env::set_var("POD_NAME", "redis-rust-0");
        assert_eq!(ClusterConfig::parse_replica_id_from_env(), 0);

        // Test: redis-rust-2 -> 2
        std::env::set_var("POD_NAME", "redis-rust-2");
        assert_eq!(ClusterConfig::parse_replica_id_from_env(), 2);

        // Test: my-cache-42 -> 42
        std::env::set_var("POD_NAME", "my-cache-42");
        assert_eq!(ClusterConfig::parse_replica_id_from_env(), 42);

        // Clean up
        std::env::remove_var("POD_NAME");
    }

    /// Test parsing replica ID from REPLICA_ID env var (fallback)
    #[test]
    fn test_parse_replica_id_from_replica_id_var() {
        let _guard = ENV_LOCK.lock().unwrap();

        // Clean up any leftover env vars
        std::env::remove_var("POD_NAME");
        std::env::remove_var("REPLICA_ID");

        // Test direct numeric value
        std::env::set_var("REPLICA_ID", "5");
        assert_eq!(ClusterConfig::parse_replica_id_from_env(), 5);

        // Test StatefulSet format in REPLICA_ID
        std::env::set_var("REPLICA_ID", "redis-rust-3");
        assert_eq!(ClusterConfig::parse_replica_id_from_env(), 3);

        // Clean up
        std::env::remove_var("REPLICA_ID");
    }

    /// Test building peer list from Kubernetes DNS
    /// Invariant: Peer list must exclude self and include all other replicas
    #[test]
    fn test_build_peer_list() {
        let _guard = ENV_LOCK.lock().unwrap();

        // Set up environment for pod 1 in a 3-replica cluster
        std::env::set_var("POD_NAME", "redis-rust-1");
        std::env::set_var("POD_NAMESPACE", "rapid-sims");
        std::env::set_var("SERVICE_NAME", "redis-rust-headless");

        let peers = ClusterConfig::build_peer_list(1, 3, 7000);

        // Should have 2 peers (excluding self)
        assert_eq!(peers.len(), 2, "Should exclude self from peer list");

        // Should contain peer 0 and peer 2
        assert!(
            peers.iter().any(|p| p.contains("redis-rust-0")),
            "Should include replica 0"
        );
        assert!(
            peers.iter().any(|p| p.contains("redis-rust-2")),
            "Should include replica 2"
        );

        // Should NOT contain self (replica 1)
        assert!(
            !peers.iter().any(|p| p.contains("redis-rust-1.")),
            "Should NOT include self"
        );

        // Verify DNS format
        for peer in &peers {
            assert!(
                peer.contains(".redis-rust-headless.rapid-sims.svc.cluster.local:7000"),
                "Peer should use Kubernetes DNS format: {}",
                peer
            );
        }

        // Clean up
        std::env::remove_var("POD_NAME");
        std::env::remove_var("POD_NAMESPACE");
        std::env::remove_var("SERVICE_NAME");
    }

    /// Test peer list for first replica (edge case)
    #[test]
    fn test_build_peer_list_first_replica() {
        let _guard = ENV_LOCK.lock().unwrap();

        std::env::set_var("POD_NAME", "redis-rust-0");
        std::env::set_var("POD_NAMESPACE", "default");
        std::env::set_var("SERVICE_NAME", "redis-rust-headless");

        let peers = ClusterConfig::build_peer_list(0, 3, 7000);

        assert_eq!(peers.len(), 2);
        assert!(peers.iter().any(|p| p.contains("redis-rust-1")));
        assert!(peers.iter().any(|p| p.contains("redis-rust-2")));

        // Clean up
        std::env::remove_var("POD_NAME");
        std::env::remove_var("POD_NAMESPACE");
        std::env::remove_var("SERVICE_NAME");
    }

    /// Test cluster config to replication config conversion
    /// Invariant: ReplicationConfig must preserve all cluster settings
    #[test]
    fn test_cluster_config_to_replication_config() {
        let cluster = ClusterConfig {
            enabled: true,
            replica_id: 2,
            gossip_port: 7000,
            cluster_size: 3,
            peers: vec![
                "redis-rust-0.svc:7000".to_string(),
                "redis-rust-1.svc:7000".to_string(),
            ],
            gossip_interval_ms: 100,
        };

        let repl = cluster.to_replication_config();

        assert!(repl.enabled, "Replication should be enabled");
        assert_eq!(repl.replica_id, 2, "Replica ID should match");
        assert_eq!(repl.peers.len(), 2, "Peers should match");
        assert_eq!(
            repl.replication_factor, 3,
            "Replication factor should match cluster size"
        );
        assert_eq!(repl.gossip_interval_ms, 100, "Gossip interval should match");
    }
}
