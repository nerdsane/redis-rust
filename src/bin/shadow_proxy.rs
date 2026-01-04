//! Shadow Testing Proxy for Redis
//!
//! Forwards all Redis commands to both the official Redis server and
//! the Rust implementation, comparing responses for compatibility testing.
//!
//! ## Usage
//!
//! ```bash
//! # Start official Redis on port 6379
//! docker run -p 6379:6379 redis
//!
//! # Start Rust implementation on port 6380
//! REDIS_PORT=6380 server-persistent
//!
//! # Start shadow proxy on port 6381
//! SHADOW_LISTEN_PORT=6381 \
//! SHADOW_PRIMARY=localhost:6379 \
//! SHADOW_SECONDARY=localhost:6380 \
//! shadow-proxy
//!
//! # Connect clients to proxy
//! redis-cli -p 6381
//! ```
//!
//! ## Environment Variables
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | SHADOW_LISTEN_PORT | 6381 | Proxy listen port |
//! | SHADOW_PRIMARY | localhost:6379 | Primary Redis (responses returned to client) |
//! | SHADOW_SECONDARY | localhost:6380 | Secondary Redis (shadow, responses compared) |
//! | SHADOW_LOG_MISMATCHES | true | Log response mismatches |
//! | SHADOW_FAIL_ON_MISMATCH | false | Return error to client on mismatch |

use bytes::{BytesMut, BufMut, Buf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn, error, debug};

const DEFAULT_LISTEN_PORT: u16 = 6381;
const DEFAULT_PRIMARY: &str = "localhost:6379";
const DEFAULT_SECONDARY: &str = "localhost:6380";

struct Config {
    listen_port: u16,
    primary: String,
    secondary: String,
    log_mismatches: bool,
    fail_on_mismatch: bool,
}

impl Config {
    fn from_env() -> Self {
        Config {
            listen_port: std::env::var("SHADOW_LISTEN_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_LISTEN_PORT),
            primary: std::env::var("SHADOW_PRIMARY")
                .unwrap_or_else(|_| DEFAULT_PRIMARY.to_string()),
            secondary: std::env::var("SHADOW_SECONDARY")
                .unwrap_or_else(|_| DEFAULT_SECONDARY.to_string()),
            log_mismatches: std::env::var("SHADOW_LOG_MISMATCHES")
                .map(|s| s != "false" && s != "0")
                .unwrap_or(true),
            fail_on_mismatch: std::env::var("SHADOW_FAIL_ON_MISMATCH")
                .map(|s| s == "true" || s == "1")
                .unwrap_or(false),
        }
    }
}

struct Stats {
    total_commands: AtomicU64,
    mismatches: AtomicU64,
    primary_errors: AtomicU64,
    secondary_errors: AtomicU64,
}

impl Stats {
    fn new() -> Self {
        Stats {
            total_commands: AtomicU64::new(0),
            mismatches: AtomicU64::new(0),
            primary_errors: AtomicU64::new(0),
            secondary_errors: AtomicU64::new(0),
        }
    }

    fn print_summary(&self) {
        let total = self.total_commands.load(Ordering::Relaxed);
        let mismatches = self.mismatches.load(Ordering::Relaxed);
        let primary_err = self.primary_errors.load(Ordering::Relaxed);
        let secondary_err = self.secondary_errors.load(Ordering::Relaxed);

        let match_rate = if total > 0 {
            ((total - mismatches) as f64 / total as f64) * 100.0
        } else {
            100.0
        };

        println!();
        println!("Shadow Proxy Statistics");
        println!("=======================");
        println!("Total commands:    {}", total);
        println!("Mismatches:        {}", mismatches);
        println!("Match rate:        {:.2}%", match_rate);
        println!("Primary errors:    {}", primary_err);
        println!("Secondary errors:  {}", secondary_err);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let config = Arc::new(Config::from_env());
    let stats = Arc::new(Stats::new());

    println!("Redis Shadow Testing Proxy");
    println!("==========================");
    println!();
    println!("Configuration:");
    println!("  Listen port: {}", config.listen_port);
    println!("  Primary:     {} (responses returned to client)", config.primary);
    println!("  Secondary:   {} (shadow, responses compared)", config.secondary);
    println!("  Log mismatches: {}", config.log_mismatches);
    println!("  Fail on mismatch: {}", config.fail_on_mismatch);
    println!();

    let addr = format!("0.0.0.0:{}", config.listen_port);
    let listener = TcpListener::bind(&addr).await?;

    info!("Shadow proxy listening on {}", addr);
    println!("Proxy listening on {}", addr);
    println!("Press Ctrl+C to shutdown and see statistics");
    println!();

    let stats_clone = stats.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        stats_clone.print_summary();
        std::process::exit(0);
    });

    loop {
        let (client_stream, client_addr) = listener.accept().await?;
        let config = config.clone();
        let stats = stats.clone();

        tokio::spawn(async move {
            debug!("New connection from {}", client_addr);
            if let Err(e) = handle_client(client_stream, config, stats).await {
                error!("Connection error from {}: {}", client_addr, e);
            }
        });
    }
}

async fn handle_client(
    mut client: TcpStream,
    config: Arc<Config>,
    stats: Arc<Stats>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = client.set_nodelay(true);

    // Connect to both backends
    let mut primary = TcpStream::connect(&config.primary).await?;
    let mut secondary = TcpStream::connect(&config.secondary).await?;
    let _ = primary.set_nodelay(true);
    let _ = secondary.set_nodelay(true);

    let mut client_buf = BytesMut::with_capacity(4096);
    let mut primary_buf = BytesMut::with_capacity(4096);
    let mut secondary_buf = BytesMut::with_capacity(4096);
    let mut read_buf = [0u8; 4096];

    loop {
        // Read from client
        let n = client.read(&mut read_buf).await?;
        if n == 0 {
            break;
        }

        let request = &read_buf[..n];
        let cmd_preview = extract_command_preview(request);

        // Forward to both backends in parallel
        let (primary_result, secondary_result) = tokio::join!(
            forward_and_receive(&mut primary, request, &mut primary_buf),
            forward_and_receive(&mut secondary, request, &mut secondary_buf),
        );

        stats.total_commands.fetch_add(1, Ordering::Relaxed);

        // Handle primary response
        let primary_response = match primary_result {
            Ok(resp) => resp,
            Err(e) => {
                stats.primary_errors.fetch_add(1, Ordering::Relaxed);
                error!("Primary error for {}: {}", cmd_preview, e);
                // Return error to client
                let err_resp = format!("-ERR primary backend error: {}\r\n", e);
                client.write_all(err_resp.as_bytes()).await?;
                continue;
            }
        };

        // Handle secondary response
        let secondary_response = match secondary_result {
            Ok(resp) => Some(resp),
            Err(e) => {
                stats.secondary_errors.fetch_add(1, Ordering::Relaxed);
                warn!("Secondary error for {}: {}", cmd_preview, e);
                None
            }
        };

        // Compare responses
        if let Some(ref secondary_resp) = secondary_response {
            if !responses_match(&primary_response, secondary_resp) {
                stats.mismatches.fetch_add(1, Ordering::Relaxed);

                if config.log_mismatches {
                    warn!(
                        "MISMATCH for command: {}\n  Primary:   {}\n  Secondary: {}",
                        cmd_preview,
                        format_response_preview(&primary_response),
                        format_response_preview(secondary_resp)
                    );
                }

                if config.fail_on_mismatch {
                    let err_resp = "-ERR shadow mismatch detected\r\n";
                    client.write_all(err_resp.as_bytes()).await?;
                    continue;
                }
            }
        }

        // Return primary response to client
        client.write_all(&primary_response).await?;
    }

    Ok(())
}

async fn forward_and_receive(
    conn: &mut TcpStream,
    request: &[u8],
    buf: &mut BytesMut,
) -> Result<Vec<u8>, std::io::Error> {
    conn.write_all(request).await?;
    conn.flush().await?;

    buf.clear();
    let mut temp = [0u8; 4096];

    // Read response (simplified - assumes single response fits in buffer)
    // In production, this would need proper RESP parsing for pipelining
    let n = conn.read(&mut temp).await?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection closed",
        ));
    }

    Ok(temp[..n].to_vec())
}

fn extract_command_preview(data: &[u8]) -> String {
    // Extract first line or first 50 chars for logging
    let preview: String = data
        .iter()
        .take(100)
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' })
        .collect();

    // Try to extract the command name from RESP format
    if let Some(cmd) = parse_resp_command(data) {
        return cmd;
    }

    preview.chars().take(50).collect()
}

fn parse_resp_command(data: &[u8]) -> Option<String> {
    // Simple RESP array parser to extract command name
    // Format: *N\r\n$len\r\nCOMMAND\r\n...
    if data.first() != Some(&b'*') {
        return None;
    }

    let s = std::str::from_utf8(data).ok()?;
    let lines: Vec<&str> = s.split("\r\n").collect();

    // lines[0] = "*N", lines[1] = "$len", lines[2] = "COMMAND"
    if lines.len() >= 3 && lines[1].starts_with('$') {
        return Some(lines[2].to_uppercase());
    }

    None
}

fn responses_match(a: &[u8], b: &[u8]) -> bool {
    // For now, exact match. Could be enhanced to handle:
    // - Floating point tolerance
    // - Order-independent arrays
    // - Timestamp differences
    a == b
}

fn format_response_preview(data: &[u8]) -> String {
    let preview: String = data
        .iter()
        .take(100)
        .map(|&b| {
            if b == b'\r' {
                '\\'.to_string() + "r"
            } else if b == b'\n' {
                '\\'.to_string() + "n"
            } else if b.is_ascii_graphic() || b == b' ' {
                (b as char).to_string()
            } else {
                format!("\\x{:02x}", b)
            }
        })
        .collect::<Vec<_>>()
        .join("");

    if data.len() > 100 {
        format!("{}... ({} bytes)", preview, data.len())
    } else {
        preview
    }
}
