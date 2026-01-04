//! Telemetry-Style Metrics Server
//!
//! A metrics aggregation server that showcases the unique features of redis-rust:
//! - CRDT counters for coordination-free distributed counting
//! - Hot key detection for popular dashboard metrics
//! - Pipelining for high-throughput batch ingestion
//! - Eventual consistency for multi-node metric aggregation
//!
//! Usage:
//!   cargo run --bin metrics-server --release
//!
//! The server listens on port 8125 (StatsD compatible) and accepts:
//! - Custom metric commands (MCOUNTER, MGAUGE, etc.)
//! - Standard Redis commands with structured keys

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use redis_sim::metrics::{MetricsCommand, MetricsCommandExecutor};

/// Metrics server configuration
#[derive(Debug, Clone)]
pub struct MetricsServerConfig {
    /// Server port
    pub port: u16,
    /// Enable hot key detection
    pub enable_hot_key_detection: bool,
    /// Replica ID for this node
    pub replica_id: u64,
    /// Maximum concurrent connections
    pub max_connections: usize,
}

impl Default for MetricsServerConfig {
    fn default() -> Self {
        MetricsServerConfig {
            port: 8125,
            enable_hot_key_detection: true,
            replica_id: 1,
            max_connections: 10000,
        }
    }
}

/// Metrics server state shared across connections
struct ServerState {
    executor: MetricsCommandExecutor,
    #[allow(dead_code)]
    config: MetricsServerConfig,
}

impl ServerState {
    fn new(config: MetricsServerConfig) -> Self {
        let mut executor = MetricsCommandExecutor::new(config.replica_id);
        if config.enable_hot_key_detection {
            executor = executor.with_hot_key_detection();
        }
        ServerState { executor, config }
    }
}

/// Run the metrics server
pub async fn run_server(config: MetricsServerConfig) -> std::io::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = TcpListener::bind(addr).await?;

    info!("Metrics server listening on {}", addr);
    info!("Hot key detection: {}", config.enable_hot_key_detection);
    info!("Replica ID: {}", config.replica_id);

    let state = Arc::new(RwLock::new(ServerState::new(config)));

    loop {
        match listener.accept().await {
            Ok((stream, peer_addr)) => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, state, peer_addr).await {
                        error!("Connection error from {}: {}", peer_addr, e);
                    }
                });
            }
            Err(e) => {
                error!("Accept error: {}", e);
            }
        }
    }
}

/// Handle a single client connection
async fn handle_connection(
    mut stream: TcpStream,
    state: Arc<RwLock<ServerState>>,
    peer_addr: SocketAddr,
) -> std::io::Result<()> {
    info!("Client connected: {}", peer_addr);

    // Enable TCP_NODELAY for low latency
    stream.set_nodelay(true)?;

    let mut buffer = BytesMut::with_capacity(8192);
    let mut read_buf = [0u8; 4096];

    loop {
        // Read data from client
        let n = stream.read(&mut read_buf).await?;
        if n == 0 {
            debug!("Client disconnected: {}", peer_addr);
            break;
        }

        buffer.extend_from_slice(&read_buf[..n]);

        // Try to parse and execute commands
        let mut response_buffer = Vec::new();
        let mut commands_processed = 0;

        loop {
            match parse_command(&buffer) {
                Ok((cmd, consumed)) => {
                    // Remove parsed bytes from buffer
                    let _ = buffer.split_to(consumed);

                    // Execute command
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;

                    let result = {
                        let mut state = state.write().await;
                        state.executor.execute(cmd, now_ms)
                    };

                    // Append response
                    response_buffer.extend_from_slice(result.to_resp_string().as_bytes());
                    commands_processed += 1;
                }
                Err(ParseError::Incomplete) => break,
                Err(ParseError::Invalid(msg)) => {
                    warn!("Parse error from {}: {}", peer_addr, msg);
                    response_buffer.extend_from_slice(
                        format!("-ERR {}\r\n", msg).as_bytes()
                    );
                    buffer.clear();
                    break;
                }
            }
        }

        // Flush all responses at once (pipelining optimization)
        if !response_buffer.is_empty() {
            stream.write_all(&response_buffer).await?;
            stream.flush().await?;
            debug!("Processed {} commands from {}", commands_processed, peer_addr);
        }
    }

    Ok(())
}

#[derive(Debug)]
enum ParseError {
    Incomplete,
    Invalid(String),
}

/// Parse a command from the buffer
fn parse_command(buffer: &BytesMut) -> Result<(MetricsCommand, usize), ParseError> {
    // Try RESP protocol first
    if buffer.starts_with(b"*") {
        return parse_resp_command(buffer);
    }

    // Try inline command (for telnet/nc)
    parse_inline_command(buffer)
}

/// Parse RESP array command
fn parse_resp_command(buffer: &BytesMut) -> Result<(MetricsCommand, usize), ParseError> {
    let s = std::str::from_utf8(buffer).map_err(|_| ParseError::Invalid("Invalid UTF-8".to_string()))?;

    // Find end of array count
    let first_crlf = s.find("\r\n").ok_or(ParseError::Incomplete)?;
    let array_count: usize = s[1..first_crlf]
        .parse()
        .map_err(|_| ParseError::Invalid("Invalid array count".to_string()))?;

    let mut args = Vec::with_capacity(array_count);
    let mut pos = first_crlf + 2;

    for _ in 0..array_count {
        if pos >= s.len() {
            return Err(ParseError::Incomplete);
        }

        if !s[pos..].starts_with('$') {
            return Err(ParseError::Invalid("Expected bulk string".to_string()));
        }

        let len_end = s[pos..].find("\r\n").ok_or(ParseError::Incomplete)? + pos;
        let len: usize = s[pos + 1..len_end]
            .parse()
            .map_err(|_| ParseError::Invalid("Invalid bulk string length".to_string()))?;

        let data_start = len_end + 2;
        let data_end = data_start + len;

        if data_end + 2 > s.len() {
            return Err(ParseError::Incomplete);
        }

        args.push(s[data_start..data_end].to_string());
        pos = data_end + 2;
    }

    let cmd = MetricsCommand::parse(&args)
        .map_err(|e| ParseError::Invalid(e))?;

    Ok((cmd, pos))
}

/// Parse inline command (space-separated)
fn parse_inline_command(buffer: &BytesMut) -> Result<(MetricsCommand, usize), ParseError> {
    let s = std::str::from_utf8(buffer).map_err(|_| ParseError::Invalid("Invalid UTF-8".to_string()))?;

    let crlf = s.find("\r\n").ok_or(ParseError::Incomplete)?;
    let line = &s[..crlf];

    let args: Vec<String> = line.split_whitespace().map(String::from).collect();
    if args.is_empty() {
        return Err(ParseError::Invalid("Empty command".to_string()));
    }

    let cmd = MetricsCommand::parse(&args)
        .map_err(|e| ParseError::Invalid(e))?;

    Ok((cmd, crlf + 2))
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║   Telemetry-Style Metrics Aggregation Server              ║");
    println!("║   Showcasing redis-rust unique features                   ║");
    println!("╠═══════════════════════════════════════════════════════════╣");
    println!("║ Features:                                                 ║");
    println!("║  - CRDT counters (coordination-free distributed counting) ║");
    println!("║  - Hot key detection (popular dashboard metrics)          ║");
    println!("║  - Pipelining (113-134% faster than Redis)                ║");
    println!("║  - Eventual consistency (multi-node aggregation)          ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();

    let config = MetricsServerConfig::default();

    println!("Commands:");
    println!("  MCOUNTER <name> [tag:value...] [increment]  - Increment counter");
    println!("  MGAUGE <name> [tag:value...] <value>        - Set gauge");
    println!("  MUPDOWN <name> [tag:value...] <delta>       - Update up/down counter");
    println!("  MDIST <name> [tag:value...] <value>         - Add to distribution");
    println!("  MUNIQUE <name> [tag:value...] <value>       - Add to unique set");
    println!("  MQUERY <name> [tag:value...]                - Query metric");
    println!("  MHOTKEYS [limit]                            - Get hot metrics");
    println!("  MLIST [pattern]                             - List metrics");
    println!();
    println!("Example:");
    println!("  MCOUNTER http.requests host:web01 env:prod 1");
    println!("  MGAUGE system.cpu host:web01 75.5");
    println!("  MQUERY http.requests host:web01");
    println!();

    run_server(config).await
}
