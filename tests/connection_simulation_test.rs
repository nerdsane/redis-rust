//! Connection Layer Simulation Tests
//!
//! Tests the pipelining fix and connection handler behavior using
//! deterministic simulation. These tests verify that:
//! - Batched flushing produces correct results
//! - Batched flushing reduces syscalls (flush count)
//! - Partial reads are handled correctly
//! - The fix is deterministic and reproducible

use redis_sim::redis::{Command, RespValue, SDS};
use redis_sim::simulator::connection::{PipelineSimulator, SimulatedConnection};

/// Test that batched flushing produces identical results to unbatched
/// but with fewer flush() calls
#[test]
fn test_batched_vs_unbatched_correctness() {
    let pipeline_sizes = [1, 2, 4, 8, 16, 32, 64, 128];

    for size in pipeline_sizes {
        let commands: Vec<Command> = (0..size)
            .map(|i| Command::set(format!("key{}", i), SDS::from_str(&format!("val{}", i))))
            .collect();

        // Batched (the fix)
        let mut batched = SimulatedConnection::new(42);
        batched.send_pipeline(commands.clone());
        let batched_responses = batched.process();

        // Unbatched (the bug)
        let mut unbatched = SimulatedConnection::new(42).with_unbatched_flush();
        unbatched.send_pipeline(commands);
        let unbatched_responses = unbatched.process();

        // Results must be identical
        assert_eq!(
            batched_responses, unbatched_responses,
            "Pipeline size {} produced different results",
            size
        );

        // Batched should have exactly 1 flush
        assert_eq!(
            batched.flush_count(),
            1,
            "Batched pipeline {} should have 1 flush, got {}",
            size,
            batched.flush_count()
        );

        // Unbatched should have N flushes (one per command)
        assert_eq!(
            unbatched.flush_count(),
            size,
            "Unbatched pipeline {} should have {} flushes, got {}",
            size,
            size,
            unbatched.flush_count()
        );
    }
}

/// Test that the pipelining fix maintains command ordering
#[test]
fn test_pipeline_ordering() {
    let mut conn = SimulatedConnection::new(42);

    conn.send_pipeline(vec![
        Command::set("counter".to_string(), SDS::from_str("0")),
        Command::Incr("counter".to_string()),
        Command::Incr("counter".to_string()),
        Command::Incr("counter".to_string()),
        Command::Incr("counter".to_string()),
        Command::Incr("counter".to_string()),
        Command::Get("counter".to_string()),
    ]);

    let responses = conn.process();

    assert_eq!(responses.len(), 7);
    // SET returns OK
    assert_eq!(responses[0], RespValue::ok());
    // Each INCR returns the new value
    assert_eq!(responses[1], RespValue::Integer(1));
    assert_eq!(responses[2], RespValue::Integer(2));
    assert_eq!(responses[3], RespValue::Integer(3));
    assert_eq!(responses[4], RespValue::Integer(4));
    assert_eq!(responses[5], RespValue::Integer(5));
    // GET returns final value
    assert_eq!(responses[6], RespValue::BulkString(Some(b"5".to_vec())));

    // Single flush for entire pipeline
    assert_eq!(conn.flush_count(), 1);
}

/// Test mixed read/write pipeline
#[test]
fn test_mixed_pipeline() {
    let mut conn = SimulatedConnection::new(42);

    conn.send_pipeline(vec![
        Command::set("a".to_string(), SDS::from_str("1")),
        Command::set("b".to_string(), SDS::from_str("2")),
        Command::Get("a".to_string()),
        Command::Get("b".to_string()),
        Command::Get("nonexistent".to_string()),
        Command::del("a".to_string()),
        Command::Get("a".to_string()),
    ]);

    let responses = conn.process();

    assert_eq!(responses.len(), 7);
    assert_eq!(responses[0], RespValue::ok()); // SET a
    assert_eq!(responses[1], RespValue::ok()); // SET b
    assert_eq!(responses[2], RespValue::BulkString(Some(b"1".to_vec()))); // GET a
    assert_eq!(responses[3], RespValue::BulkString(Some(b"2".to_vec()))); // GET b
    assert_eq!(responses[4], RespValue::BulkString(None)); // GET nonexistent = nil
    assert_eq!(responses[5], RespValue::Integer(1)); // DEL a = 1 key deleted
    assert_eq!(responses[6], RespValue::BulkString(None)); // GET a after DEL = nil
}

/// Test that partial TCP reads are handled correctly
#[test]
fn test_partial_reads_handled() {
    // Run with 50% partial read probability
    let mut conn = SimulatedConnection::new(12345).with_partial_reads(0.5);

    let commands: Vec<Command> = (0..20)
        .map(|i| Command::set(format!("key{}", i), SDS::from_str(&format!("value{}", i))))
        .collect();

    conn.send_pipeline(commands);
    let responses = conn.process();

    // All 20 commands should still execute correctly
    assert_eq!(responses.len(), 20);
    for response in responses {
        assert_eq!(response, RespValue::ok());
    }
}

/// Test determinism - same seed produces same results
#[test]
fn test_deterministic_behavior() {
    let seeds = [0, 42, 12345, 99999, u64::MAX];

    for seed in seeds {
        let commands: Vec<Command> = (0..10)
            .map(|i| Command::set(format!("k{}", i), SDS::from_str(&format!("v{}", i))))
            .collect();

        // Run twice with same seed
        let mut conn1 = SimulatedConnection::new(seed);
        conn1.send_pipeline(commands.clone());
        let responses1 = conn1.process();

        let mut conn2 = SimulatedConnection::new(seed);
        conn2.send_pipeline(commands);
        let responses2 = conn2.process();

        assert_eq!(
            responses1, responses2,
            "Seed {} produced non-deterministic results",
            seed
        );
        assert_eq!(conn1.flush_count(), conn2.flush_count());
    }
}

/// Test flush reduction across many pipeline sizes
#[test]
fn test_flush_reduction_statistics() {
    let mut simulator =
        PipelineSimulator::new(42).with_sizes(vec![1, 2, 4, 8, 16, 32, 64, 128, 256]);

    simulator.run();

    println!("\n{}", simulator.summary());

    // Verify all results are correct
    for result in &simulator.results {
        assert!(
            result.all_responses_correct,
            "Pipeline {} failed correctness check",
            result.pipeline_size
        );
    }

    // Calculate total reduction
    let total_batched: usize = simulator.results.iter().map(|r| r.batched_flushes).sum();
    let total_unbatched: usize = simulator.results.iter().map(|r| r.unbatched_flushes).sum();

    // Batched should be much less
    assert!(
        total_batched < total_unbatched / 10,
        "Expected >90% flush reduction, got batched={} unbatched={}",
        total_batched,
        total_unbatched
    );
}

/// Test multi-seed correctness (chaos testing for connection layer)
#[test]
fn test_1000_seeds_connection_correctness() {
    let mut failures = Vec::new();

    for seed in 0..1000 {
        let commands: Vec<Command> = (0..16)
            .map(|i| {
                if i % 2 == 0 {
                    Command::set(format!("k{}", i), SDS::from_str(&format!("v{}", i)))
                } else {
                    Command::Get(format!("k{}", i - 1))
                }
            })
            .collect();

        // Batched
        let mut batched = SimulatedConnection::new(seed);
        batched.send_pipeline(commands.clone());
        let batched_responses = batched.process();

        // Unbatched
        let mut unbatched = SimulatedConnection::new(seed).with_unbatched_flush();
        unbatched.send_pipeline(commands);
        let unbatched_responses = unbatched.process();

        if batched_responses != unbatched_responses {
            failures.push(seed);
        }
    }

    assert!(failures.is_empty(), "Seeds with failures: {:?}", failures);
}

/// Test that PING commands work in pipeline
#[test]
fn test_ping_pipeline() {
    let mut conn = SimulatedConnection::new(42);

    conn.send_pipeline(vec![Command::Ping, Command::Ping, Command::Ping]);

    let responses = conn.process();

    assert_eq!(responses.len(), 3);
    for response in responses {
        assert_eq!(response, RespValue::pong());
    }
    assert_eq!(conn.flush_count(), 1);
}

/// Test empty pipeline
#[test]
fn test_empty_pipeline() {
    let mut conn = SimulatedConnection::new(42);

    // Don't send any commands
    let responses = conn.process();

    assert_eq!(responses.len(), 0);
    assert_eq!(conn.flush_count(), 0);
}

/// Test very large pipeline (stress test)
#[test]
fn test_large_pipeline_stress() {
    let mut conn = SimulatedConnection::new(42);

    let commands: Vec<Command> = (0..1000)
        .map(|i| {
            Command::set(
                format!("key{:04}", i),
                SDS::from_str(&format!("value{:04}", i)),
            )
        })
        .collect();

    conn.send_pipeline(commands);
    let responses = conn.process();

    assert_eq!(responses.len(), 1000);
    assert_eq!(
        conn.flush_count(),
        1,
        "1000 commands should still be 1 flush"
    );

    // Verify all OKs
    for (i, response) in responses.iter().enumerate() {
        assert_eq!(response, &RespValue::ok(), "Command {} failed", i);
    }
}
