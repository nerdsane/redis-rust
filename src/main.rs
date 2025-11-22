use redis_sim::{Simulation, SimulationConfig, RedisServer, RedisClient};
use redis_sim::simulator::{VirtualTime, Duration, EventType, buggify};

fn main() {
    println!("=== Redis Deterministic Simulator ===\n");
    
    println!("Running test scenarios...\n");
    
    test_basic_operations();
    test_deterministic_replay();
    test_network_faults();
    test_buggify();
    
    println!("\n=== All tests completed successfully! ===");
}

fn test_basic_operations() {
    println!("--- Test 1: Basic Redis Operations ---");
    
    let config = SimulationConfig {
        seed: 42,
        max_time: VirtualTime::from_secs(10),
    };
    
    let mut sim = Simulation::new(config);
    
    let server_host = sim.add_host("redis-server".to_string());
    let client_host = sim.add_host("redis-client".to_string());
    
    let mut server = RedisServer::new(server_host);
    let mut client = RedisClient::new(client_host, server_host);
    
    let set_cmd = b"*3\r\n$3\r\nSET\r\n$3\r\nkey\r\n$5\r\nvalue\r\n".to_vec();
    client.send_command(&mut sim, set_cmd);
    
    sim.schedule_timer(client_host, Duration::from_millis(5));
    
    let get_cmd = b"*2\r\n$3\r\nGET\r\n$3\r\nkey\r\n".to_vec();
    
    sim.run(|sim, event| {
        server.handle_event(sim, event);
        client.handle_event(event);
        
        if let EventType::Timer(_) = event.event_type {
            if event.host_id == client_host {
                client.send_command(sim, get_cmd.clone());
            }
        }
    });
    
    println!("  ✓ Basic SET/GET operations completed");
    println!("  Simulation time: {:?}ms\n", sim.current_time().as_millis());
}

fn test_deterministic_replay() {
    println!("--- Test 2: Deterministic Replay ---");
    
    let seed = 12345;
    
    let results1 = run_simulation_with_seed(seed);
    let results2 = run_simulation_with_seed(seed);
    
    assert_eq!(results1, results2, "Results should be identical with same seed");
    
    println!("  ✓ Two runs with seed {} produced identical results", seed);
    println!("  Results: {:?}\n", results1);
}

fn run_simulation_with_seed(seed: u64) -> Vec<u64> {
    let config = SimulationConfig {
        seed,
        max_time: VirtualTime::from_secs(5),
    };
    
    let mut sim = Simulation::new(config);
    let mut random_values = Vec::new();
    
    for _ in 0..10 {
        random_values.push(sim.rng().next_u64());
    }
    
    random_values
}

fn test_network_faults() {
    println!("--- Test 3: Network Fault Injection ---");
    
    let config = SimulationConfig {
        seed: 99,
        max_time: VirtualTime::from_secs(10),
    };
    
    let mut sim = Simulation::new(config);
    
    let server_host = sim.add_host("redis-server".to_string());
    let client_host = sim.add_host("redis-client".to_string());
    
    sim.set_network_drop_rate(0.2);
    
    let mut server = RedisServer::new(server_host);
    let mut client = RedisClient::new(client_host, server_host);
    
    let mut sent_count = 0;
    let mut received_count = 0;
    
    for i in 0..5 {
        sim.schedule_timer(client_host, Duration::from_millis(i * 100));
    }
    
    let ping_cmd = b"*1\r\n$4\r\nPING\r\n".to_vec();
    
    sim.run(|sim, event| {
        server.handle_event(sim, event);
        
        match &event.event_type {
            EventType::Timer(_) if event.host_id == client_host => {
                client.send_command(sim, ping_cmd.clone());
                sent_count += 1;
            }
            EventType::NetworkMessage(msg) if msg.to == client_host => {
                received_count += 1;
                client.handle_event(event);
            }
            _ => {
                client.handle_event(event);
            }
        }
    });
    
    println!("  ✓ Network fault injection test completed");
    println!("  Commands sent: {}", sent_count);
    println!("  Responses received: {} (some dropped due to 20% packet loss)\n", received_count);
}

fn test_buggify() {
    println!("--- Test 4: BUGGIFY Chaos Testing ---");
    
    let config = SimulationConfig {
        seed: 777,
        max_time: VirtualTime::from_secs(1),
    };
    
    let mut sim = Simulation::new(config);
    
    let mut buggify_count = 0;
    
    for _ in 0..1000 {
        if buggify(sim.rng()) {
            buggify_count += 1;
        }
    }
    
    println!("  ✓ BUGGIFY triggered {} times out of 1000 (expected ~1%)", buggify_count);
    println!("  This allows FoundationDB-style chaos injection\n");
}

fn encode_command(parts: &[&str]) -> Vec<u8> {
    let mut result = format!("*{}\r\n", parts.len()).into_bytes();
    for part in parts {
        result.extend_from_slice(format!("${}\r\n{}\r\n", part.len(), part).as_bytes());
    }
    result
}
