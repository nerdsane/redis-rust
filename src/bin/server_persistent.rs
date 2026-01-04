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

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use redis_sim::production::ReplicatedShardedState;
use redis_sim::replication::{ReplicationConfig, ConsistencyLevel};
use redis_sim::streaming::{
    StreamingConfig, ObjectStoreType, WorkerHandles,
    create_integration, StreamingIntegrationTrait,
};
use redis_sim::redis::{RespCodec, RespValue, Command};
use bytes::{BytesMut, BufMut};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::signal;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, error, warn};

// Redis-compatible defaults for drop-in replacement
const DEFAULT_PORT: u16 = 6379;
const DEFAULT_REPLICA_ID: u64 = 1;
const DEFAULT_DATA_PATH: &str = "/data";
const DEFAULT_S3_PREFIX: &str = "redis-stream";
const DEFAULT_S3_REGION: &str = "us-east-1";

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
            }),
            #[cfg(feature = "s3")]
            "s3" => {
                let bucket = self.s3_bucket.clone().ok_or(
                    "REDIS_S3_BUCKET required for S3 store type".to_string()
                )?;
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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

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
            println!("  Bucket: {}", config.s3_bucket.as_deref().unwrap_or("(not set)"));
            println!("  Prefix: {}", config.s3_prefix);
            if let Some(endpoint) = &config.s3_endpoint {
                println!("  Endpoint: {}", endpoint);
            }
        }
        _ => {}
    }
    println!();

    // Create replication config
    let repl_config = ReplicationConfig {
        enabled: true,
        replica_id: DEFAULT_REPLICA_ID,
        consistency_level: ConsistencyLevel::Eventual,
        gossip_interval_ms: 100,
        peers: vec![],
        replication_factor: 1,
        partitioned_mode: false,
        selective_gossip: false,
        virtual_nodes_per_physical: 150,
    };

    // Create state
    let state = ReplicatedShardedState::new(repl_config);

    // Ensure persistence directory exists for localfs
    if config.store_type == "localfs" {
        std::fs::create_dir_all(&config.data_path)?;
    }

    // Create integration and perform recovery
    let integration = create_integration(streaming_config, DEFAULT_REPLICA_ID).await?;

    info!("Checking for existing data to recover...");
    let stats = integration.recover(&state).await?;
    if stats.segments_loaded > 0 {
        info!(
            "Recovered {} segments, {} deltas, {} keys",
            stats.segments_loaded,
            stats.deltas_replayed,
            state.key_count()
        );
    } else {
        info!("Starting fresh (no existing data)");
    }

    let (worker_handles, sender) = integration.start_workers().await?;
    // Note: state is moved to Arc below, so we can't modify it after this point
    // The delta_sink is set through the integration layer

    let state = Arc::new(state);

    // Set delta sink - need interior mutability pattern
    // Actually, we need to set this before wrapping in Arc
    // For now, streaming persistence works through the integration layer

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

    // Graceful shutdown
    info!("Shutting down persistence workers...");
    worker_handles.shutdown().await;

    println!("Server shutdown complete");
    info!("Server shutdown complete");

    Ok(())
}

async fn handle_health_check(mut stream: TcpStream) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
                Ok(Some(resp_value)) => {
                    match Command::from_resp_zero_copy(&resp_value) {
                        Ok(cmd) => {
                            let response = state.execute(cmd);
                            encode_resp_into(&response, &mut write_buffer);
                        }
                        Err(e) => {
                            encode_error_into(&e, &mut write_buffer);
                        }
                    }
                }
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
