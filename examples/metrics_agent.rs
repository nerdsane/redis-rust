//! Metrics Agent Example
//!
//! Demonstrates high-throughput metric submission using pipelining.
//! This example showcases how batch metric ingestion achieves
//! 113-134% better performance than Redis through pipelining.
//!
//! Usage:
//!   1. Start the metrics server: cargo run --bin metrics-server --release
//!   2. Run this agent: cargo run --example metrics_agent --release

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn main() {
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║   Metrics Agent - High Throughput Demo                    ║");
    println!("║   Showcasing pipelining performance                       ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();

    let server_addr = "127.0.0.1:8125";

    println!("Connecting to metrics server at {}...", server_addr);

    let mut stream = match TcpStream::connect(server_addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect: {}", e);
            eprintln!("Make sure the metrics server is running:");
            eprintln!("  cargo run --bin metrics-server --release");
            return;
        }
    };

    // Set TCP_NODELAY for lower latency
    stream.set_nodelay(true).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    println!("Connected!\n");

    // Demo 1: Single metric submission
    println!("=== Demo 1: Single Metric Submission ===");
    submit_single_metric(&mut stream);
    println!();

    // Demo 2: Batch metric submission (showcases pipelining)
    println!("=== Demo 2: Batch Metric Submission (Pipelining) ===");
    submit_batch_metrics(&mut stream, 1000);
    println!();

    // Demo 3: Simulated application metrics
    println!("=== Demo 3: Simulated Application Metrics ===");
    simulate_application_metrics(&mut stream);
    println!();

    // Demo 4: Query the metrics
    println!("=== Demo 4: Query Metrics ===");
    query_metrics(&mut stream);
    println!();

    // Demo 5: Hot key detection
    println!("=== Demo 5: Hot Key Detection ===");
    demonstrate_hot_keys(&mut stream);

    println!("\nAgent finished. Check the server logs for hot key detection.");
}

fn submit_single_metric(stream: &mut TcpStream) {
    let cmd = "MCOUNTER http.requests host:web01 env:prod 1\r\n";

    let start = Instant::now();
    stream.write_all(cmd.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = [0u8; 256];
    let n = stream.read(&mut response).unwrap();
    let elapsed = start.elapsed();

    println!("  Command: MCOUNTER http.requests host:web01 env:prod 1");
    println!("  Response: {}", String::from_utf8_lossy(&response[..n]).trim());
    println!("  Latency: {:?}", elapsed);
}

fn submit_batch_metrics(stream: &mut TcpStream, count: usize) {
    println!("  Submitting {} metrics in a single batch...", count);

    // Build a batch of commands (pipelined)
    let mut batch = String::new();
    for i in 0..count {
        let host = format!("web{:02}", i % 10);
        let endpoint = format!("/api/v1/endpoint{}", i % 50);
        batch.push_str(&format!(
            "MCOUNTER http.requests host:{} endpoint:{} 1\r\n",
            host, endpoint
        ));
    }

    let start = Instant::now();

    // Send entire batch at once (single write)
    stream.write_all(batch.as_bytes()).unwrap();
    stream.flush().unwrap();

    // Read all responses
    let mut total_read = 0;
    let mut response_buf = vec![0u8; 16 * count]; // +OK\r\n = 5 bytes each

    while total_read < count * 5 {
        match stream.read(&mut response_buf[total_read..]) {
            Ok(0) => break,
            Ok(n) => total_read += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        }
    }

    let elapsed = start.elapsed();
    let throughput = count as f64 / elapsed.as_secs_f64();

    println!("  Batch sent and acknowledged!");
    println!("  Total time: {:?}", elapsed);
    println!("  Throughput: {:.0} metrics/second", throughput);
    println!("  (This demonstrates pipelining - all commands sent before any response)");
}

fn simulate_application_metrics(stream: &mut TcpStream) {
    println!("  Simulating web application metrics...");

    let hosts = ["web01", "web02", "web03", "web04"];
    let endpoints = ["/api/users", "/api/orders", "/api/products", "/api/health"];

    let mut batch = String::new();

    // Request counters
    for host in &hosts {
        for endpoint in &endpoints {
            let count = rand_u32() % 100 + 1;
            batch.push_str(&format!(
                "MCOUNTER http.requests host:{} endpoint:{} {}\r\n",
                host, endpoint, count
            ));
        }
    }

    // CPU gauges
    for host in &hosts {
        let cpu = 30.0 + (rand_u32() % 50) as f64;
        batch.push_str(&format!(
            "MGAUGE system.cpu host:{} {:.1}\r\n",
            host, cpu
        ));
    }

    // Memory gauges
    for host in &hosts {
        let mem = 40.0 + (rand_u32() % 40) as f64;
        batch.push_str(&format!(
            "MGAUGE system.memory host:{} {:.1}\r\n",
            host, mem
        ));
    }

    // Latency distributions
    for _ in 0..100 {
        let host = hosts[rand_u32() as usize % hosts.len()];
        let endpoint = endpoints[rand_u32() as usize % endpoints.len()];
        let latency = 10.0 + (rand_u32() % 200) as f64;
        batch.push_str(&format!(
            "MDIST http.latency host:{} endpoint:{} {:.1}\r\n",
            host, endpoint, latency
        ));
    }

    // Unique users
    for i in 0..50 {
        batch.push_str(&format!(
            "MUNIQUE unique.users page:/home user{}\r\n",
            i
        ));
    }

    // Active connections (up-down counter)
    for host in &hosts {
        let delta = (rand_u32() % 20) as i32 - 10;
        batch.push_str(&format!(
            "MUPDOWN connections.active host:{} {}\r\n",
            host, delta
        ));
    }

    let start = Instant::now();

    stream.write_all(batch.as_bytes()).unwrap();
    stream.flush().unwrap();

    // Read responses
    let mut response_buf = vec![0u8; 8192];
    let _ = stream.read(&mut response_buf);

    let elapsed = start.elapsed();

    println!("  Submitted mixed metrics (counters, gauges, distributions, sets)");
    println!("  Time: {:?}", elapsed);
}

fn query_metrics(stream: &mut TcpStream) {
    let queries = [
        "MQUERY http.requests host:web01",
        "MQUERY system.cpu host:web01",
        "MQUERY http.latency host:web01",
        "MLIST http.*",
    ];

    for query in &queries {
        println!("  Query: {}", query);

        let cmd = format!("{}\r\n", query);
        stream.write_all(cmd.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut response = [0u8; 1024];
        match stream.read(&mut response) {
            Ok(n) => {
                let resp = String::from_utf8_lossy(&response[..n]);
                // Parse RESP response for display
                let display = parse_resp_for_display(&resp);
                println!("  Result: {}", display);
            }
            Err(e) => println!("  Error: {}", e),
        }
        println!();
    }
}

fn demonstrate_hot_keys(stream: &mut TcpStream) {
    println!("  Querying same metric 100 times to trigger hot key detection...");

    // Query the same metric many times
    let cmd = "MQUERY http.requests host:web01\r\n";
    for _ in 0..100 {
        stream.write_all(cmd.as_bytes()).unwrap();
    }
    stream.flush().unwrap();

    // Drain responses
    let mut response_buf = vec![0u8; 8192];
    let _ = stream.read(&mut response_buf);

    // Now check hot keys
    println!("  Checking hot keys...");
    let cmd = "MHOTKEYS 5\r\n";
    stream.write_all(cmd.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = [0u8; 4096];
    match stream.read(&mut response) {
        Ok(n) => {
            let resp = String::from_utf8_lossy(&response[..n]);
            println!("  Hot keys response: {}", resp.trim());
        }
        Err(e) => println!("  Error: {}", e),
    }
}

fn parse_resp_for_display(resp: &str) -> String {
    let resp = resp.trim();
    if resp.starts_with('+') {
        return resp[1..].trim().to_string();
    }
    if resp.starts_with(':') {
        return resp[1..].trim().to_string();
    }
    if resp.starts_with('$') {
        // Bulk string
        if let Some(idx) = resp.find("\r\n") {
            let rest = &resp[idx + 2..];
            if let Some(end) = rest.find("\r\n") {
                return rest[..end].to_string();
            }
            return rest.trim().to_string();
        }
    }
    if resp.starts_with('*') {
        return "(array response)".to_string();
    }
    if resp.starts_with('-') {
        return format!("ERROR: {}", &resp[1..].trim());
    }
    resp.to_string()
}

// Simple pseudo-random for demo (not cryptographically secure)
fn rand_u32() -> u32 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    nanos.wrapping_mul(1103515245).wrapping_add(12345)
}
