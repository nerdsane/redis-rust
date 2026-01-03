use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = env::args().nth(1).unwrap_or("3000".to_string());
    let addr = format!("127.0.0.1:{}", port);

    println!("🔥 Redis Server Benchmark\n");
    println!("Connecting to {}...\n", addr);
    
    let num_requests = 5_000;
    let num_clients = 25;
    
    println!("Configuration:");
    println!("  Requests per test: {}", num_requests);
    println!("  Concurrent clients: {}\n", num_clients);
    println!("Running benchmarks...\n");
    
    benchmark_ping(&addr, num_requests, num_clients).await?;
    benchmark_set(&addr, num_requests, num_clients).await?;
    benchmark_get(&addr, num_requests, num_clients).await?;
    benchmark_incr(&addr, num_requests, num_clients).await?;
    benchmark_mset(&addr, num_requests / 10, num_clients).await?;
    benchmark_mixed(&addr, num_requests, num_clients).await?;
    
    println!("\n✅ Benchmark complete!");
    
    Ok(())
}

async fn benchmark_ping(addr: &str, num_requests: usize, num_clients: usize) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));

    let mut handles = vec![];
    let requests_per_client = num_requests / num_clients;

    for _ in 0..num_clients {
        let completed = completed.clone();
        let addr = addr.to_string();
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            let cmd = b"*1\r\n$4\r\nPING\r\n";
            
            for _ in 0..requests_per_client {
                stream.write_all(cmd).await.unwrap();
                let mut buf = vec![0u8; 64];
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.await?;
    }
    
    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();
    let latency_ms = elapsed.as_secs_f64() * 1000.0 / total as f64;
    
    println!("PING:");
    println!("  {} requests completed in {:.2}s", total, elapsed.as_secs_f64());
    println!("  {:.0} requests per second", ops_per_sec);
    println!("  {:.3} ms average latency\n", latency_ms);
    
    Ok(())
}

async fn benchmark_set(addr: &str, num_requests: usize, num_clients: usize) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));

    let mut handles = vec![];
    let requests_per_client = num_requests / num_clients;

    for client_id in 0..num_clients {
        let completed = completed.clone();
        let addr = addr.to_string();
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            
            for i in 0..requests_per_client {
                let key = format!("key:{}:{}", client_id, i);
                let value = format!("value_{}", i);
                let cmd = format!("*3\r\n$3\r\nSET\r\n${}\r\n{}\r\n${}\r\n{}\r\n", 
                    key.len(), key, value.len(), value);
                
                stream.write_all(cmd.as_bytes()).await.unwrap();
                let mut buf = vec![0u8; 64];
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.await?;
    }
    
    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();
    let latency_ms = elapsed.as_secs_f64() * 1000.0 / total as f64;
    
    println!("SET:");
    println!("  {} requests completed in {:.2}s", total, elapsed.as_secs_f64());
    println!("  {:.0} requests per second", ops_per_sec);
    println!("  {:.3} ms average latency\n", latency_ms);
    
    Ok(())
}

async fn benchmark_get(addr: &str, num_requests: usize, num_clients: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut setup_stream = TcpStream::connect(addr).await?;
    for i in 0..100 {
        let cmd = format!("*3\r\n$3\r\nSET\r\n$8\r\nget_key{}\r\n$5\r\nvalue\r\n", i);
        setup_stream.write_all(cmd.as_bytes()).await?;
        let mut buf = vec![0u8; 64];
        setup_stream.read(&mut buf).await?;
    }
    
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));
    
    let mut handles = vec![];
    let requests_per_client = num_requests / num_clients;
    
    for _ in 0..num_clients {
        let completed = completed.clone();
        let addr = addr.to_string();
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();

            for i in 0..requests_per_client {
                let key_id = i % 100;
                let cmd = format!("*2\r\n$3\r\nGET\r\n$8\r\nget_key{}\r\n", key_id);
                
                stream.write_all(cmd.as_bytes()).await.unwrap();
                let mut buf = vec![0u8; 128];
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.await?;
    }
    
    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();
    let latency_ms = elapsed.as_secs_f64() * 1000.0 / total as f64;
    
    println!("GET:");
    println!("  {} requests completed in {:.2}s", total, elapsed.as_secs_f64());
    println!("  {:.0} requests per second", ops_per_sec);
    println!("  {:.3} ms average latency\n", latency_ms);
    
    Ok(())
}

async fn benchmark_incr(addr: &str, num_requests: usize, num_clients: usize) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));

    let mut handles = vec![];
    let requests_per_client = num_requests / num_clients;

    for client_id in 0..num_clients {
        let completed = completed.clone();
        let addr = addr.to_string();
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            let key = format!("counter:{}", client_id);
            let cmd = format!("*2\r\n$4\r\nINCR\r\n${}\r\n{}\r\n", key.len(), key);
            
            for _ in 0..requests_per_client {
                stream.write_all(cmd.as_bytes()).await.unwrap();
                let mut buf = vec![0u8; 64];
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.await?;
    }
    
    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();
    let latency_ms = elapsed.as_secs_f64() * 1000.0 / total as f64;
    
    println!("INCR:");
    println!("  {} requests completed in {:.2}s", total, elapsed.as_secs_f64());
    println!("  {:.0} requests per second", ops_per_sec);
    println!("  {:.3} ms average latency\n", latency_ms);
    
    Ok(())
}

async fn benchmark_mset(addr: &str, num_requests: usize, num_clients: usize) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));

    let mut handles = vec![];
    let requests_per_client = num_requests / num_clients;

    for client_id in 0..num_clients {
        let completed = completed.clone();
        let addr = addr.to_string();
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            
            for i in 0..requests_per_client {
                let cmd = format!(
                    "*11\r\n$4\r\nMSET\r\n$5\r\nmk1:{}\r\n$3\r\nmv1\r\n$5\r\nmk2:{}\r\n$3\r\nmv2\r\n$5\r\nmk3:{}\r\n$3\r\nmv3\r\n$5\r\nmk4:{}\r\n$3\r\nmv4\r\n$5\r\nmk5:{}\r\n$3\r\nmv5\r\n",
                    client_id * 1000 + i, client_id * 1000 + i, client_id * 1000 + i, 
                    client_id * 1000 + i, client_id * 1000 + i
                );
                
                stream.write_all(cmd.as_bytes()).await.unwrap();
                let mut buf = vec![0u8; 64];
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.await?;
    }
    
    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();
    let latency_ms = elapsed.as_secs_f64() * 1000.0 / total as f64;
    
    println!("MSET (5 keys per operation):");
    println!("  {} requests completed in {:.2}s", total, elapsed.as_secs_f64());
    println!("  {:.0} requests per second", ops_per_sec);
    println!("  {:.3} ms average latency\n", latency_ms);
    
    Ok(())
}

async fn benchmark_mixed(addr: &str, num_requests: usize, num_clients: usize) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let completed = Arc::new(AtomicU64::new(0));

    let mut handles = vec![];
    let requests_per_client = num_requests / num_clients;

    for client_id in 0..num_clients {
        let completed = completed.clone();
        let addr = addr.to_string();
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            
            for i in 0..requests_per_client {
                let cmd = match i % 5 {
                    0 => format!("*3\r\n$3\r\nSET\r\n$7\r\nmix:{}:{}\r\n$5\r\nvalue\r\n", client_id, i),
                    1 => format!("*2\r\n$3\r\nGET\r\n$7\r\nmix:{}:{}\r\n", client_id, i.saturating_sub(1)),
                    2 => format!("*2\r\n$4\r\nINCR\r\n$9\r\nmixctr:{}\r\n", client_id),
                    3 => format!("*2\r\n$6\r\nEXISTS\r\n$7\r\nmix:{}:{}\r\n", client_id, i.saturating_sub(2)),
                    _ => "*1\r\n$4\r\nPING\r\n".to_string(),
                };
                
                stream.write_all(cmd.as_bytes()).await.unwrap();
                let mut buf = vec![0u8; 256];
                stream.read(&mut buf).await.unwrap();
                completed.fetch_add(1, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.await?;
    }
    
    let elapsed = start.elapsed();
    let total = completed.load(Ordering::Relaxed);
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();
    let latency_ms = elapsed.as_secs_f64() * 1000.0 / total as f64;
    
    println!("MIXED (SET/GET/INCR/EXISTS/PING):");
    println!("  {} requests completed in {:.2}s", total, elapsed.as_secs_f64());
    println!("  {:.0} requests per second", ops_per_sec);
    println!("  {:.3} ms average latency\n", latency_ms);
    
    Ok(())
}
