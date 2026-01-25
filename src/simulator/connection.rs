//! Connection Layer Simulation
//!
//! Simulates TCP connection behavior for deterministic testing of:
//! - Pipelining (multiple commands in one read)
//! - Batched responses (multiple responses in one write)
//! - Partial reads (incomplete RESP data)
//! - Network delays and drops
//!
//! This closes the gap between CommandExecutor simulation and production
//! TCP handling, allowing us to test the connection handler logic
//! deterministically.

use super::{DeterministicRng, VirtualTime};
use crate::redis::{Command, CommandExecutor, RespCodec, RespValue};
use bytes::{BufMut, BytesMut};
use std::collections::VecDeque;

/// Simulated TCP read buffer behavior
pub struct SimulatedReadBuffer {
    /// Data waiting to be "read" by the connection handler
    pending_data: BytesMut,
    /// Commands queued to be converted to RESP and added to pending_data
    queued_commands: VecDeque<Command>,
    /// Simulate partial reads (return only part of available data)
    partial_read_probability: f64,
    /// RNG for partial read simulation
    rng: DeterministicRng,
}

impl SimulatedReadBuffer {
    pub fn new(seed: u64) -> Self {
        SimulatedReadBuffer {
            pending_data: BytesMut::with_capacity(8192),
            queued_commands: VecDeque::new(),
            partial_read_probability: 0.0,
            rng: DeterministicRng::new(seed),
        }
    }

    pub fn with_partial_reads(mut self, probability: f64) -> Self {
        self.partial_read_probability = probability;
        self
    }

    /// Queue a command to be "sent" by the client
    pub fn queue_command(&mut self, cmd: Command) {
        self.queued_commands.push_back(cmd);
    }

    /// Queue multiple commands (simulates pipelining)
    pub fn queue_pipeline(&mut self, commands: Vec<Command>) {
        for cmd in commands {
            self.queued_commands.push_back(cmd);
        }
    }

    /// Flush queued commands to the read buffer (simulates TCP arrival)
    pub fn flush_to_buffer(&mut self) {
        while let Some(cmd) = self.queued_commands.pop_front() {
            self.encode_command(&cmd);
        }
    }

    /// Flush a specific number of commands (simulates partial TCP arrival)
    pub fn flush_n_to_buffer(&mut self, n: usize) {
        for _ in 0..n {
            if let Some(cmd) = self.queued_commands.pop_front() {
                self.encode_command(&cmd);
            }
        }
    }

    /// Simulate a read() call - returns available data
    pub fn read(&mut self) -> Option<BytesMut> {
        if self.pending_data.is_empty() {
            return None;
        }

        // Simulate partial reads based on probability
        if self.partial_read_probability > 0.0
            && self.rng.gen_bool(self.partial_read_probability)
            && self.pending_data.len() > 1
        {
            let len = self.pending_data.len() as u64;
            let split_point = self.rng.gen_range(1, len) as usize;
            let partial = self.pending_data.split_to(split_point);
            Some(partial)
        } else {
            Some(self.pending_data.split())
        }
    }

    /// Check if there's data available to read
    pub fn has_data(&self) -> bool {
        !self.pending_data.is_empty() || !self.queued_commands.is_empty()
    }

    /// Encode a command to RESP format
    fn encode_command(&mut self, cmd: &Command) {
        match cmd {
            Command::Ping => {
                self.pending_data.extend_from_slice(b"*1\r\n$4\r\nPING\r\n");
            }
            Command::Set { key, value, .. } => {
                let key_bytes = key.as_bytes();
                let value_bytes = value.as_bytes();
                self.pending_data.extend_from_slice(b"*3\r\n$3\r\nSET\r\n$");
                self.pending_data
                    .extend_from_slice(key_bytes.len().to_string().as_bytes());
                self.pending_data.extend_from_slice(b"\r\n");
                self.pending_data.extend_from_slice(key_bytes);
                self.pending_data.extend_from_slice(b"\r\n$");
                self.pending_data
                    .extend_from_slice(value_bytes.len().to_string().as_bytes());
                self.pending_data.extend_from_slice(b"\r\n");
                self.pending_data.extend_from_slice(value_bytes);
                self.pending_data.extend_from_slice(b"\r\n");
            }
            Command::Get(key) => {
                let key_bytes = key.as_bytes();
                self.pending_data.extend_from_slice(b"*2\r\n$3\r\nGET\r\n$");
                self.pending_data
                    .extend_from_slice(key_bytes.len().to_string().as_bytes());
                self.pending_data.extend_from_slice(b"\r\n");
                self.pending_data.extend_from_slice(key_bytes);
                self.pending_data.extend_from_slice(b"\r\n");
            }
            Command::Incr(key) => {
                let key_bytes = key.as_bytes();
                self.pending_data
                    .extend_from_slice(b"*2\r\n$4\r\nINCR\r\n$");
                self.pending_data
                    .extend_from_slice(key_bytes.len().to_string().as_bytes());
                self.pending_data.extend_from_slice(b"\r\n");
                self.pending_data.extend_from_slice(key_bytes);
                self.pending_data.extend_from_slice(b"\r\n");
            }
            Command::Del(keys) => {
                // Encode DEL with all keys
                let num_elements = 1 + keys.len();
                self.pending_data
                    .extend_from_slice(format!("*{}\r\n$3\r\nDEL\r\n", num_elements).as_bytes());
                for key in keys {
                    let key_bytes = key.as_bytes();
                    self.pending_data.extend_from_slice(b"$");
                    self.pending_data
                        .extend_from_slice(key_bytes.len().to_string().as_bytes());
                    self.pending_data.extend_from_slice(b"\r\n");
                    self.pending_data.extend_from_slice(key_bytes);
                    self.pending_data.extend_from_slice(b"\r\n");
                }
            }
            _ => {
                // For other commands, encode as simple ping for now
                self.pending_data.extend_from_slice(b"*1\r\n$4\r\nPING\r\n");
            }
        }
    }

    pub fn pending_commands(&self) -> usize {
        self.queued_commands.len()
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending_data.len()
    }
}

/// Simulated write buffer (captures responses)
#[derive(Debug)]
pub struct SimulatedWriteBuffer {
    /// Accumulated response data
    data: BytesMut,
    /// Number of flush() calls (measures batching efficiency)
    flush_count: usize,
    /// Bytes per flush (for analysis)
    bytes_per_flush: Vec<usize>,
}

impl SimulatedWriteBuffer {
    pub fn new() -> Self {
        SimulatedWriteBuffer {
            data: BytesMut::with_capacity(8192),
            flush_count: 0,
            bytes_per_flush: Vec::new(),
        }
    }

    /// Simulate write_all() - accumulates data
    pub fn write_all(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data);
    }

    /// Simulate flush() - records the flush event
    pub fn flush(&mut self) {
        if !self.data.is_empty() {
            self.bytes_per_flush.push(self.data.len());
            self.flush_count += 1;
        }
    }

    /// Get total flush count (fewer = better batching)
    pub fn flush_count(&self) -> usize {
        self.flush_count
    }

    /// Get bytes per flush statistics
    pub fn bytes_per_flush(&self) -> &[usize] {
        &self.bytes_per_flush
    }

    /// Get average bytes per flush
    pub fn avg_bytes_per_flush(&self) -> f64 {
        if self.bytes_per_flush.is_empty() {
            0.0
        } else {
            self.bytes_per_flush.iter().sum::<usize>() as f64 / self.bytes_per_flush.len() as f64
        }
    }

    /// Get total bytes written
    pub fn total_bytes(&self) -> usize {
        self.data.len()
    }

    /// Clear the buffer (simulate data sent)
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// Get the accumulated data
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl Default for SimulatedWriteBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Simulates the connection handler behavior
///
/// This mirrors the logic in `connection_optimized.rs` but in a
/// deterministic, testable way.
pub struct SimulatedConnection {
    /// Read buffer (incoming commands)
    read_buffer: SimulatedReadBuffer,
    /// Write buffer (outgoing responses)
    write_buffer: SimulatedWriteBuffer,
    /// Parse buffer (accumulates partial reads)
    parse_buffer: BytesMut,
    /// Command executor
    executor: CommandExecutor,
    /// Response buffer (before flush)
    response_buffer: BytesMut,
    /// Execution history
    history: Vec<ExecutionRecord>,
    /// Current virtual time
    current_time: VirtualTime,
    /// Enable batched flushing (the fix we implemented)
    batched_flush: bool,
}

/// Record of a command execution
#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    pub command: Command,
    pub response: RespValue,
    pub flush_after: bool,
    pub time: VirtualTime,
}

impl SimulatedConnection {
    pub fn new(seed: u64) -> Self {
        SimulatedConnection {
            read_buffer: SimulatedReadBuffer::new(seed),
            write_buffer: SimulatedWriteBuffer::new(),
            parse_buffer: BytesMut::with_capacity(8192),
            executor: CommandExecutor::new(),
            response_buffer: BytesMut::with_capacity(8192),
            history: Vec::new(),
            current_time: VirtualTime::ZERO,
            batched_flush: true, // Default to the fixed behavior
        }
    }

    /// Disable batched flushing (simulates the old buggy behavior)
    pub fn with_unbatched_flush(mut self) -> Self {
        self.batched_flush = false;
        self
    }

    /// Enable partial read simulation
    pub fn with_partial_reads(mut self, probability: f64) -> Self {
        self.read_buffer = self.read_buffer.with_partial_reads(probability);
        self
    }

    /// Queue a single command
    pub fn send_command(&mut self, cmd: Command) {
        self.read_buffer.queue_command(cmd);
    }

    /// Queue a pipeline of commands
    pub fn send_pipeline(&mut self, commands: Vec<Command>) {
        self.read_buffer.queue_pipeline(commands);
    }

    /// Simulate the connection handler processing loop
    ///
    /// This mirrors `OptimizedConnectionHandler::run()` but deterministically
    pub fn process(&mut self) -> Vec<RespValue> {
        let mut responses = Vec::new();

        // Flush queued commands to simulate TCP arrival
        self.read_buffer.flush_to_buffer();

        // Simulate read() call
        while let Some(data) = self.read_buffer.read() {
            self.parse_buffer.extend_from_slice(&data);

            // Process ALL available commands (pipelining support)
            loop {
                match RespCodec::parse(&mut self.parse_buffer) {
                    Ok(Some(resp_value)) => {
                        match Command::from_resp_zero_copy(&resp_value) {
                            Ok(cmd) => {
                                let response = self.executor.execute(&cmd);
                                responses.push(response.clone());

                                // Encode response
                                Self::encode_resp(&response, &mut self.response_buffer);

                                if self.batched_flush {
                                    // NEW BEHAVIOR: Don't flush yet, continue processing
                                    self.history.push(ExecutionRecord {
                                        command: cmd,
                                        response,
                                        flush_after: false,
                                        time: self.current_time,
                                    });
                                } else {
                                    // OLD BUGGY BEHAVIOR: Flush after each command
                                    self.write_buffer.write_all(&self.response_buffer);
                                    self.write_buffer.flush();
                                    self.response_buffer.clear();

                                    self.history.push(ExecutionRecord {
                                        command: cmd,
                                        response,
                                        flush_after: true,
                                        time: self.current_time,
                                    });
                                }
                            }
                            Err(_) => {
                                // Command parse error
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        // Need more data
                        break;
                    }
                    Err(_) => {
                        // RESP parse error
                        self.parse_buffer.clear();
                        break;
                    }
                }
            }

            // Batched flush: flush ALL responses at once
            if self.batched_flush && !self.response_buffer.is_empty() {
                self.write_buffer.write_all(&self.response_buffer);
                self.write_buffer.flush();
                self.response_buffer.clear();

                // Mark the last command as having flush_after
                if let Some(last) = self.history.last_mut() {
                    last.flush_after = true;
                }
            }
        }

        responses
    }

    /// Process with simulated partial TCP arrivals
    pub fn process_with_partial_arrivals(&mut self, commands_per_read: usize) -> Vec<RespValue> {
        let mut all_responses = Vec::new();

        while self.read_buffer.pending_commands() > 0 {
            self.read_buffer.flush_n_to_buffer(commands_per_read);
            let responses = self.process();
            all_responses.extend(responses);
        }

        all_responses
    }

    /// Get flush statistics
    pub fn flush_count(&self) -> usize {
        self.write_buffer.flush_count()
    }

    /// Get bytes per flush
    pub fn bytes_per_flush(&self) -> &[usize] {
        self.write_buffer.bytes_per_flush()
    }

    /// Get average bytes per flush
    pub fn avg_bytes_per_flush(&self) -> f64 {
        self.write_buffer.avg_bytes_per_flush()
    }

    /// Get execution history
    pub fn history(&self) -> &[ExecutionRecord] {
        &self.history
    }

    /// Get total commands executed
    pub fn commands_executed(&self) -> usize {
        self.history.len()
    }

    /// Encode a RespValue to bytes
    fn encode_resp(value: &RespValue, buf: &mut BytesMut) {
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
                    Self::encode_resp(elem, buf);
                }
            }
        }
    }
}

/// Pipeline simulation harness for testing batching behavior
pub struct PipelineSimulator {
    seed: u64,
    pipeline_sizes: Vec<usize>,
    pub results: Vec<PipelineResult>,
}

/// Result of a pipeline simulation run
#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub pipeline_size: usize,
    pub batched_flushes: usize,
    pub unbatched_flushes: usize,
    pub commands_executed: usize,
    pub all_responses_correct: bool,
}

impl PipelineSimulator {
    pub fn new(seed: u64) -> Self {
        PipelineSimulator {
            seed,
            pipeline_sizes: vec![1, 2, 4, 8, 16, 32, 64],
            results: Vec::new(),
        }
    }

    pub fn with_sizes(mut self, sizes: Vec<usize>) -> Self {
        self.pipeline_sizes = sizes;
        self
    }

    /// Run pipeline simulation comparing batched vs unbatched
    pub fn run(&mut self) -> &[PipelineResult] {
        let mut rng = DeterministicRng::new(self.seed);

        for &size in &self.pipeline_sizes.clone() {
            // Generate pipeline of commands
            let commands: Vec<Command> = (0..size)
                .map(|i| {
                    if rng.gen_bool(0.5) {
                        Command::set(
                            format!("key{}", i),
                            crate::redis::SDS::from_str(&format!("value{}", i)),
                        )
                    } else {
                        Command::Get(format!("key{}", i % (i.max(1))))
                    }
                })
                .collect();

            // Test with batched flushing (fixed behavior)
            let mut batched_conn = SimulatedConnection::new(self.seed + size as u64);
            batched_conn.send_pipeline(commands.clone());
            let batched_responses = batched_conn.process();
            let batched_flushes = batched_conn.flush_count();

            // Test with unbatched flushing (old buggy behavior)
            let mut unbatched_conn =
                SimulatedConnection::new(self.seed + size as u64).with_unbatched_flush();
            unbatched_conn.send_pipeline(commands.clone());
            let unbatched_responses = unbatched_conn.process();
            let unbatched_flushes = unbatched_conn.flush_count();

            // Verify responses are identical
            let all_correct = batched_responses == unbatched_responses;

            self.results.push(PipelineResult {
                pipeline_size: size,
                batched_flushes,
                unbatched_flushes,
                commands_executed: size,
                all_responses_correct: all_correct,
            });
        }

        &self.results
    }

    /// Get summary statistics
    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str("Pipeline Simulation Results:\n");
        s.push_str("┌──────────┬─────────────────┬───────────────────┬──────────┐\n");
        s.push_str("│ Pipeline │ Batched Flushes │ Unbatched Flushes │ Correct? │\n");
        s.push_str("├──────────┼─────────────────┼───────────────────┼──────────┤\n");

        for result in &self.results {
            s.push_str(&format!(
                "│ {:>8} │ {:>15} │ {:>17} │ {:>8} │\n",
                result.pipeline_size,
                result.batched_flushes,
                result.unbatched_flushes,
                if result.all_responses_correct {
                    "Yes"
                } else {
                    "NO"
                }
            ));
        }

        s.push_str("└──────────┴─────────────────┴───────────────────┴──────────┘\n");

        // Calculate flush reduction
        let total_batched: usize = self.results.iter().map(|r| r.batched_flushes).sum();
        let total_unbatched: usize = self.results.iter().map(|r| r.unbatched_flushes).sum();
        let reduction = if total_unbatched > 0 {
            (1.0 - total_batched as f64 / total_unbatched as f64) * 100.0
        } else {
            0.0
        };

        s.push_str(&format!("\nFlush reduction: {:.1}%\n", reduction));
        s.push_str(&format!("Total batched flushes: {}\n", total_batched));
        s.push_str(&format!("Total unbatched flushes: {}\n", total_unbatched));

        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;

    #[test]
    fn test_single_command() {
        let mut conn = SimulatedConnection::new(42);
        conn.send_command(Command::Ping);

        let responses = conn.process();

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], RespValue::simple("PONG"));
        assert_eq!(conn.flush_count(), 1);
    }

    #[test]
    fn test_pipeline_batched_vs_unbatched() {
        let commands = vec![
            Command::set("k1".to_string(), SDS::from_str("v1")),
            Command::set("k2".to_string(), SDS::from_str("v2")),
            Command::Get("k1".to_string()),
            Command::Get("k2".to_string()),
        ];

        // Batched (fixed behavior)
        let mut batched = SimulatedConnection::new(42);
        batched.send_pipeline(commands.clone());
        let batched_responses = batched.process();

        // Unbatched (old buggy behavior)
        let mut unbatched = SimulatedConnection::new(42).with_unbatched_flush();
        unbatched.send_pipeline(commands);
        let unbatched_responses = unbatched.process();

        // Same responses
        assert_eq!(batched_responses, unbatched_responses);

        // But different flush counts!
        assert_eq!(batched.flush_count(), 1, "Batched should flush once");
        assert_eq!(
            unbatched.flush_count(),
            4,
            "Unbatched flushes after each command"
        );
    }

    #[test]
    fn test_pipeline_correctness() {
        let mut conn = SimulatedConnection::new(42);

        conn.send_pipeline(vec![
            Command::set("counter".to_string(), SDS::from_str("0")),
            Command::Incr("counter".to_string()),
            Command::Incr("counter".to_string()),
            Command::Incr("counter".to_string()),
            Command::Get("counter".to_string()),
        ]);

        let responses = conn.process();

        assert_eq!(responses.len(), 5);
        assert_eq!(responses[0], RespValue::simple("OK"));
        assert_eq!(responses[1], RespValue::Integer(1));
        assert_eq!(responses[2], RespValue::Integer(2));
        assert_eq!(responses[3], RespValue::Integer(3));
        assert_eq!(responses[4], RespValue::BulkString(Some(b"3".to_vec())));

        // Single flush for entire pipeline
        assert_eq!(conn.flush_count(), 1);
    }

    #[test]
    fn test_large_pipeline() {
        let mut conn = SimulatedConnection::new(42);

        let commands: Vec<Command> = (0..100)
            .map(|i| Command::set(format!("key{}", i), SDS::from_str(&format!("value{}", i))))
            .collect();

        conn.send_pipeline(commands);
        let responses = conn.process();

        assert_eq!(responses.len(), 100);
        assert_eq!(
            conn.flush_count(),
            1,
            "100 commands should still be 1 flush"
        );

        // Compare with unbatched
        let mut unbatched = SimulatedConnection::new(42).with_unbatched_flush();
        let commands: Vec<Command> = (0..100)
            .map(|i| Command::set(format!("key{}", i), SDS::from_str(&format!("value{}", i))))
            .collect();
        unbatched.send_pipeline(commands);
        unbatched.process();

        assert_eq!(unbatched.flush_count(), 100, "Unbatched = 100 flushes");
    }

    #[test]
    fn test_partial_reads() {
        let mut conn = SimulatedConnection::new(42).with_partial_reads(0.5);

        conn.send_pipeline(vec![
            Command::set("k1".to_string(), SDS::from_str("v1")),
            Command::Get("k1".to_string()),
        ]);

        let responses = conn.process();

        // Should still get correct responses despite partial reads
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0], RespValue::simple("OK"));
        assert_eq!(responses[1], RespValue::BulkString(Some(b"v1".to_vec())));
    }

    #[test]
    fn test_pipeline_simulator() {
        let mut sim = PipelineSimulator::new(12345).with_sizes(vec![1, 4, 16, 64]);

        sim.run();

        println!("{}", sim.summary());

        // All results should be correct
        for result in sim.results.iter() {
            assert!(
                result.all_responses_correct,
                "Pipeline {} failed",
                result.pipeline_size
            );

            // Batched should always be <= unbatched flushes
            assert!(
                result.batched_flushes <= result.unbatched_flushes,
                "Batched ({}) should be <= unbatched ({})",
                result.batched_flushes,
                result.unbatched_flushes
            );
        }
    }

    #[test]
    fn test_deterministic_replay() {
        // Same seed should produce identical results
        let seed = 99999;

        let mut conn1 = SimulatedConnection::new(seed);
        conn1.send_pipeline(vec![
            Command::set("k".to_string(), SDS::from_str("v")),
            Command::Get("k".to_string()),
        ]);
        let responses1 = conn1.process();

        let mut conn2 = SimulatedConnection::new(seed);
        conn2.send_pipeline(vec![
            Command::set("k".to_string(), SDS::from_str("v")),
            Command::Get("k".to_string()),
        ]);
        let responses2 = conn2.process();

        assert_eq!(responses1, responses2);
        assert_eq!(conn1.flush_count(), conn2.flush_count());
    }

    #[test]
    fn test_multi_seed_pipeline_correctness() {
        // Run 100 different seeds with various pipeline sizes
        for seed in 0..100 {
            let mut sim = PipelineSimulator::new(seed);
            let results = sim.run();

            for result in results {
                assert!(
                    result.all_responses_correct,
                    "Seed {} pipeline {} failed correctness check",
                    seed, result.pipeline_size
                );
            }
        }
    }
}
