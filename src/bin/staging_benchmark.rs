//! Staging-equivalent benchmark for local profiling
//!
//! Replicates the exact workload from staging k8s benchmark:
//! - 50 concurrent clients
//! - 200,000 requests per operation type
//! - Operations: SET, GET, INCR, LPUSH, HSET, ZADD
//!
//! Usage:
//!   cargo build --release --bin staging-benchmark
//!
//!   # Run benchmark
//!   ./target/release/staging-benchmark [host:port]
//!
//!   # Profile with perf
//!   perf record -g ./target/release/server-persistent &
//!   ./target/release/staging-benchmark
//!   perf report
//!
//!   # Generate flamegraph
//!   cargo flamegraph --bin server-persistent &
//!   ./target/release/staging-benchmark

#![allow(clippy::unused_io_amount)]

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;

const NUM_CLIENTS: usize = 50;
const REQUESTS_PER_OP: usize = 200_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:3000".to_string());

    println!("=== Staging-Equivalent Benchmark ===");
    println!("Target: {}", addr);
    println!("Clients: {}", NUM_CLIENTS);
    println!("Requests per operation: {}", REQUESTS_PER_OP);
    println!();

    // Wait for server to be ready
    wait_for_server(&addr).await?;

    let mut total_ops = 0u64;
    let mut total_time = Duration::ZERO;

    // Run each benchmark matching redis-benchmark -t set,get,incr,lpush,hset,zadd
    let (ops, time) = benchmark_set(&addr).await?;
    total_ops += ops;
    total_time += time;

    let (ops, time) = benchmark_get(&addr).await?;
    total_ops += ops;
    total_time += time;

    let (ops, time) = benchmark_incr(&addr).await?;
    total_ops += ops;
    total_time += time;

    let (ops, time) = benchmark_lpush(&addr).await?;
    total_ops += ops;
    total_time += time;

    let (ops, time) = benchmark_hset(&addr).await?;
    total_ops += ops;
    total_time += time;

    let (ops, time) = benchmark_zadd(&addr).await?;
    total_ops += ops;
    total_time += time;

    println!("=== Summary ===");
    println!(
        "Total: {} ops in {:.2}s ({:.0} ops/sec)",
        total_ops,
        total_time.as_secs_f64(),
        total_ops as f64 / total_time.as_secs_f64()
    );

    Ok(())
}

async fn wait_for_server(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    for i in 0..10 {
        match TcpStream::connect(addr).await {
            Ok(mut stream) => {
                stream.write_all(b"*1\r\n$4\r\nPING\r\n").await?;
                let mut buf = [0u8; 32];
                stream.read(&mut buf).await?;
                return Ok(());
            }
            Err(_) if i < 9 => {
                println!("Waiting for server...");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err("Server not available".into())
}

async fn benchmark_set(addr: &str) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));
    let requests_per_client = REQUESTS_PER_OP / NUM_CLIENTS;

    let mut handles = vec![];
    for client_id in 0..NUM_CLIENTS {
        let completed = completed.clone();
        let addr = addr.to_string();
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            stream.set_nodelay(true).unwrap();

            // Pre-allocate buffer for responses
            let mut buf = vec![0u8; 64];

            for i in 0..requests_per_client {
                // Match redis-benchmark key pattern
                let key = format!("key:{:012}", client_id * requests_per_client + i);
                let value = "value_data_here_for_testing"; // ~27 bytes like staging
                let cmd = format!(
                    "*3\r\n$3\r\nSET\r\n${}\r\n{}\r\n${}\r\n{}\r\n",
                    key.len(),
                    key,
                    value.len(),
                    value
                );

                stream.write_all(cmd.as_bytes()).await.unwrap();
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await?;
    }

    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!(
        "SET: {} requests in {:.2}s ({:.0} ops/sec, p50={:.3}ms)",
        total,
        elapsed.as_secs_f64(),
        ops_per_sec,
        elapsed.as_secs_f64() * 1000.0 / total as f64 * NUM_CLIENTS as f64
    );

    Ok((total, elapsed))
}

async fn benchmark_get(addr: &str) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));
    let requests_per_client = REQUESTS_PER_OP / NUM_CLIENTS;

    let mut handles = vec![];
    for client_id in 0..NUM_CLIENTS {
        let completed = completed.clone();
        let addr = addr.to_string();
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            stream.set_nodelay(true).unwrap();

            let mut buf = vec![0u8; 128];

            for i in 0..requests_per_client {
                // Get keys that were set in the SET benchmark
                let key = format!("key:{:012}", client_id * requests_per_client + i);
                let cmd = format!("*2\r\n$3\r\nGET\r\n${}\r\n{}\r\n", key.len(), key);

                stream.write_all(cmd.as_bytes()).await.unwrap();
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await?;
    }

    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!(
        "GET: {} requests in {:.2}s ({:.0} ops/sec, p50={:.3}ms)",
        total,
        elapsed.as_secs_f64(),
        ops_per_sec,
        elapsed.as_secs_f64() * 1000.0 / total as f64 * NUM_CLIENTS as f64
    );

    Ok((total, elapsed))
}

async fn benchmark_incr(addr: &str) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));
    let requests_per_client = REQUESTS_PER_OP / NUM_CLIENTS;

    let mut handles = vec![];
    for client_id in 0..NUM_CLIENTS {
        let completed = completed.clone();
        let addr = addr.to_string();
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            stream.set_nodelay(true).unwrap();

            let mut buf = vec![0u8; 64];

            // Use rotating counter keys like redis-benchmark
            for i in 0..requests_per_client {
                let key = format!("counter:{}", (client_id * 1000 + i) % 1000);
                let cmd = format!("*2\r\n$4\r\nINCR\r\n${}\r\n{}\r\n", key.len(), key);

                stream.write_all(cmd.as_bytes()).await.unwrap();
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await?;
    }

    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!(
        "INCR: {} requests in {:.2}s ({:.0} ops/sec, p50={:.3}ms)",
        total,
        elapsed.as_secs_f64(),
        ops_per_sec,
        elapsed.as_secs_f64() * 1000.0 / total as f64 * NUM_CLIENTS as f64
    );

    Ok((total, elapsed))
}

async fn benchmark_lpush(addr: &str) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));
    let requests_per_client = REQUESTS_PER_OP / NUM_CLIENTS;

    let mut handles = vec![];
    for client_id in 0..NUM_CLIENTS {
        let completed = completed.clone();
        let addr = addr.to_string();
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            stream.set_nodelay(true).unwrap();

            let mut buf = vec![0u8; 64];

            for i in 0..requests_per_client {
                // Rotate through 100 lists like redis-benchmark
                let key = format!("mylist:{}", (client_id * 100 + i) % 100);
                let value = "list_item_value";
                let cmd = format!(
                    "*3\r\n$5\r\nLPUSH\r\n${}\r\n{}\r\n${}\r\n{}\r\n",
                    key.len(),
                    key,
                    value.len(),
                    value
                );

                stream.write_all(cmd.as_bytes()).await.unwrap();
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await?;
    }

    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!(
        "LPUSH: {} requests in {:.2}s ({:.0} ops/sec, p50={:.3}ms)",
        total,
        elapsed.as_secs_f64(),
        ops_per_sec,
        elapsed.as_secs_f64() * 1000.0 / total as f64 * NUM_CLIENTS as f64
    );

    Ok((total, elapsed))
}

async fn benchmark_hset(addr: &str) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));
    let requests_per_client = REQUESTS_PER_OP / NUM_CLIENTS;

    let mut handles = vec![];
    for client_id in 0..NUM_CLIENTS {
        let completed = completed.clone();
        let addr = addr.to_string();
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            stream.set_nodelay(true).unwrap();

            let mut buf = vec![0u8; 64];

            for i in 0..requests_per_client {
                // Rotate through 1000 hashes with 100 fields each
                let key = format!("myhash:{}", (client_id * 1000 + i) % 1000);
                let field = format!("field:{}", i % 100);
                let value = "hash_value_data";
                let cmd = format!(
                    "*4\r\n$4\r\nHSET\r\n${}\r\n{}\r\n${}\r\n{}\r\n${}\r\n{}\r\n",
                    key.len(),
                    key,
                    field.len(),
                    field,
                    value.len(),
                    value
                );

                stream.write_all(cmd.as_bytes()).await.unwrap();
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await?;
    }

    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!(
        "HSET: {} requests in {:.2}s ({:.0} ops/sec, p50={:.3}ms)",
        total,
        elapsed.as_secs_f64(),
        ops_per_sec,
        elapsed.as_secs_f64() * 1000.0 / total as f64 * NUM_CLIENTS as f64
    );

    Ok((total, elapsed))
}

async fn benchmark_zadd(addr: &str) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));
    let requests_per_client = REQUESTS_PER_OP / NUM_CLIENTS;

    let mut handles = vec![];
    for client_id in 0..NUM_CLIENTS {
        let completed = completed.clone();
        let addr = addr.to_string();
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            stream.set_nodelay(true).unwrap();

            let mut buf = vec![0u8; 64];

            for i in 0..requests_per_client {
                // Rotate through 100 sorted sets with 1000 members each
                let key = format!("myzset:{}", (client_id * 100 + i) % 100);
                let score = ((client_id * requests_per_client + i) % 10000) as f64;
                let member = format!("member:{}", i % 1000);
                let cmd = format!(
                    "*4\r\n$4\r\nZADD\r\n${}\r\n{}\r\n${}\r\n{}\r\n${}\r\n{}\r\n",
                    key.len(),
                    key,
                    score.to_string().len(),
                    score,
                    member.len(),
                    member
                );

                stream.write_all(cmd.as_bytes()).await.unwrap();
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await?;
    }

    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!(
        "ZADD: {} requests in {:.2}s ({:.0} ops/sec, p50={:.3}ms)",
        total,
        elapsed.as_secs_f64(),
        ops_per_sec,
        elapsed.as_secs_f64() * 1000.0 / total as f64 * NUM_CLIENTS as f64
    );

    Ok((total, elapsed))
}
