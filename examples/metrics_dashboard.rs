//! Metrics Dashboard Example
//!
//! Demonstrates querying metrics and hot key detection.
//! This example showcases how the system detects frequently
//! queried metrics and optimizes for dashboard workloads.
//!
//! Usage:
//!   1. Start the metrics server: cargo run --bin metrics-server --release
//!   2. Run the agent first: cargo run --example metrics_agent --release
//!   3. Run this dashboard: cargo run --example metrics_dashboard --release

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn main() {
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║   Metrics Dashboard - Query Demo                          ║");
    println!("║   Showcasing hot key detection and CRDT queries           ║");
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

    stream.set_nodelay(true).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    println!("Connected!\n");

    // First, add some sample data
    println!("=== Adding Sample Data ===");
    add_sample_data(&mut stream);
    println!();

    // Show all available metrics
    println!("=== Available Metrics ===");
    list_all_metrics(&mut stream);
    println!();

    // Query specific metrics
    println!("=== Request Counters by Host ===");
    query_request_counters(&mut stream);
    println!();

    println!("=== System Gauges ===");
    query_system_gauges(&mut stream);
    println!();

    println!("=== Latency Distribution ===");
    query_latency_distribution(&mut stream);
    println!();

    // Demonstrate hot key detection
    println!("=== Hot Key Detection Demo ===");
    demonstrate_hot_key_detection(&mut stream);
    println!();

    // Show CRDT convergence benefit
    println!("=== CRDT Counter Demonstration ===");
    demonstrate_crdt_counters(&mut stream);

    println!("\nDashboard demo complete!");
}

fn add_sample_data(stream: &mut TcpStream) {
    let commands = [
        // Request counters
        "MCOUNTER http.requests host:web01 env:prod 150",
        "MCOUNTER http.requests host:web02 env:prod 200",
        "MCOUNTER http.requests host:web03 env:prod 175",
        "MCOUNTER http.errors host:web01 env:prod 5",
        "MCOUNTER http.errors host:web02 env:prod 3",
        // System gauges
        "MGAUGE system.cpu host:web01 65.5",
        "MGAUGE system.cpu host:web02 78.2",
        "MGAUGE system.cpu host:web03 45.0",
        "MGAUGE system.memory host:web01 72.1",
        "MGAUGE system.memory host:web02 85.5",
        // Active connections (up-down counter)
        "MUPDOWN connections.active host:web01 25",
        "MUPDOWN connections.active host:web02 30",
        // Unique users
        "MUNIQUE unique.visitors page:/home user_alice",
        "MUNIQUE unique.visitors page:/home user_bob",
        "MUNIQUE unique.visitors page:/home user_charlie",
        "MUNIQUE unique.visitors page:/about user_alice",
        "MUNIQUE unique.visitors page:/about user_david",
    ];

    let mut batch = String::new();
    for cmd in &commands {
        batch.push_str(cmd);
        batch.push_str("\r\n");
    }

    stream.write_all(batch.as_bytes()).unwrap();
    stream.flush().unwrap();

    // Drain responses
    let mut buf = vec![0u8; 4096];
    let _ = stream.read(&mut buf);

    println!("  Added {} sample metrics", commands.len());
}

fn list_all_metrics(stream: &mut TcpStream) {
    let cmd = "MLIST\r\n";
    stream.write_all(cmd.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = [0u8; 8192];
    match stream.read(&mut response) {
        Ok(n) => {
            let resp = String::from_utf8_lossy(&response[..n]);
            println!("  Metrics in the system:");
            // Parse array response
            for line in resp.lines() {
                if line.starts_with('$') {
                    continue;
                }
                if line.starts_with('*') {
                    continue;
                }
                if !line.is_empty() && !line.contains(':') {
                    continue;
                }
                if line.contains("metric:") {
                    println!("    - {}", line);
                }
            }
        }
        Err(e) => println!("  Error: {}", e),
    }
}

fn query_request_counters(stream: &mut TcpStream) {
    let hosts = ["web01", "web02", "web03"];

    for host in &hosts {
        let cmd = format!("MQUERY http.requests host:{}\r\n", host);
        stream.write_all(cmd.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut response = [0u8; 1024];
        match stream.read(&mut response) {
            Ok(n) => {
                let resp = String::from_utf8_lossy(&response[..n]);
                let value = parse_value(&resp);
                println!("  {}: {} requests", host, value);
            }
            Err(e) => println!("  {}: Error - {}", host, e),
        }
    }
}

fn query_system_gauges(stream: &mut TcpStream) {
    let hosts = ["web01", "web02", "web03"];

    println!("  CPU Usage:");
    for host in &hosts {
        let cmd = format!("MQUERY system.cpu host:{}\r\n", host);
        stream.write_all(cmd.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut response = [0u8; 1024];
        match stream.read(&mut response) {
            Ok(n) => {
                let resp = String::from_utf8_lossy(&response[..n]);
                let value = parse_value(&resp);
                println!("    {}: {}%", host, value);
            }
            Err(_) => println!("    {}: N/A", host),
        }
    }

    println!("  Memory Usage:");
    for host in &hosts[..2] {
        let cmd = format!("MQUERY system.memory host:{}\r\n", host);
        stream.write_all(cmd.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut response = [0u8; 1024];
        match stream.read(&mut response) {
            Ok(n) => {
                let resp = String::from_utf8_lossy(&response[..n]);
                let value = parse_value(&resp);
                println!("    {}: {}%", host, value);
            }
            Err(_) => println!("    {}: N/A", host),
        }
    }
}

fn query_latency_distribution(stream: &mut TcpStream) {
    // First add some latency data
    let mut batch = String::new();
    for i in 0..100 {
        let latency = 10.0 + (i as f64 * 2.0);
        batch.push_str(&format!(
            "MDIST http.latency endpoint:/api/users {:.1}\r\n",
            latency
        ));
    }

    stream.write_all(batch.as_bytes()).unwrap();
    stream.flush().unwrap();

    // Drain responses
    let mut buf = vec![0u8; 4096];
    let _ = stream.read(&mut buf);

    // Query distribution
    let cmd = "MQUERY http.latency endpoint:/api/users\r\n";
    stream.write_all(cmd.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = [0u8; 4096];
    match stream.read(&mut response) {
        Ok(n) => {
            let resp = String::from_utf8_lossy(&response[..n]);
            println!("  Latency statistics for /api/users:");
            println!("  {}", resp.trim());
            // Would parse the array response for real display
        }
        Err(e) => println!("  Error: {}", e),
    }
}

fn demonstrate_hot_key_detection(stream: &mut TcpStream) {
    println!("  Querying http.requests 50 times to trigger hot key detection...");

    // Query same metric repeatedly
    let cmd = "MQUERY http.requests host:web01\r\n";
    for _ in 0..50 {
        stream.write_all(cmd.as_bytes()).unwrap();
    }
    stream.flush().unwrap();

    // Drain responses
    let mut buf = vec![0u8; 8192];
    let _ = stream.read(&mut buf);

    // Check hot keys
    std::thread::sleep(Duration::from_millis(100));

    let cmd = "MHOTKEYS 5\r\n";
    stream.write_all(cmd.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = [0u8; 4096];
    match stream.read(&mut response) {
        Ok(n) => {
            let resp = String::from_utf8_lossy(&response[..n]);
            println!("  Hot keys detected:");
            println!("  {}", resp.trim());
            println!();
            println!("  Hot key detection enables:");
            println!("    - Automatic replication factor increase for popular metrics");
            println!("    - Optimized routing to serve from nearest replica");
            println!("    - Dashboard query pattern analysis");
        }
        Err(e) => println!("  Error: {}", e),
    }
}

fn demonstrate_crdt_counters(stream: &mut TcpStream) {
    println!("  CRDT counters enable coordination-free distributed counting.");
    println!("  Multiple nodes can increment the same counter without coordination.");
    println!();
    println!("  Simulation: Two 'replicas' incrementing same counter concurrently:");

    // Simulate two replicas incrementing
    let cmd1 = "MCOUNTER distributed.counter replica:node1 100\r\n";
    let cmd2 = "MCOUNTER distributed.counter replica:node2 150\r\n";

    stream.write_all(cmd1.as_bytes()).unwrap();
    stream.write_all(cmd2.as_bytes()).unwrap();
    stream.flush().unwrap();

    // Drain responses
    let mut buf = vec![0u8; 256];
    let _ = stream.read(&mut buf);

    // Query total
    let cmd = "MQUERY distributed.counter replica:node1\r\n";
    stream.write_all(cmd.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = [0u8; 256];
    match stream.read(&mut response) {
        Ok(n) => {
            let resp = String::from_utf8_lossy(&response[..n]);
            let value = parse_value(&resp);
            println!("    Node 1 increment: 100");
            println!("    Node 2 increment: 150");
            println!("    Query result (node1 tags): {}", value);
        }
        Err(e) => println!("  Error: {}", e),
    }

    println!();
    println!("  In a real multi-node deployment:");
    println!("    - Each node maintains its own GCounter contribution");
    println!("    - Merge is commutative and idempotent (no lost updates)");
    println!("    - Gossip protocol synchronizes counters across nodes");
    println!("    - Final value = sum of all replica contributions");
}

fn parse_value(resp: &str) -> String {
    let resp = resp.trim();
    if resp.starts_with(':') {
        return resp[1..].trim_end_matches("\r\n").to_string();
    }
    if resp.starts_with('$') {
        if resp.contains("-1") {
            return "nil".to_string();
        }
        if let Some(idx) = resp.find("\r\n") {
            let rest = &resp[idx + 2..];
            if let Some(end) = rest.find("\r\n") {
                return rest[..end].to_string();
            }
            return rest.trim().to_string();
        }
    }
    if resp.starts_with('*') {
        return "(array)".to_string();
    }
    if resp.starts_with('-') {
        return format!("error: {}", &resp[1..].trim());
    }
    resp.to_string()
}
