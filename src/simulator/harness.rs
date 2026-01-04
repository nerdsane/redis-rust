use crate::redis::{Command, CommandExecutor, RespValue};
use super::{VirtualTime, Duration, DeterministicRng};

#[derive(Debug, Clone)]
pub struct Operation {
    pub client_id: usize,
    pub time: VirtualTime,
    pub command: Command,
}

#[derive(Debug, Clone)]
pub struct OperationResult {
    pub client_id: usize,
    pub invoke_time: VirtualTime,
    pub complete_time: VirtualTime,
    pub command: Command,
    pub response: RespValue,
}

pub struct SimulatedRedisNode {
    executor: CommandExecutor,
    current_time: VirtualTime,
    #[allow(dead_code)]
    simulation_start_epoch: i64,
}

impl SimulatedRedisNode {
    pub fn new(simulation_start_epoch: i64) -> Self {
        let mut executor = CommandExecutor::new();
        executor.set_simulation_start_epoch(simulation_start_epoch);
        SimulatedRedisNode {
            executor,
            current_time: VirtualTime::ZERO,
            simulation_start_epoch,
        }
    }

    #[inline]
    pub fn advance_time(&mut self, new_time: VirtualTime) {
        debug_assert!(new_time >= self.current_time, "Time cannot go backwards");
        self.current_time = new_time;
        self.executor.set_time(new_time);
    }

    #[inline]
    pub fn execute(&mut self, cmd: &Command) -> RespValue {
        self.executor.execute(cmd)
    }

    pub fn evict_expired(&mut self) -> usize {
        self.executor.evict_expired_direct(self.current_time)
    }

    pub fn current_time(&self) -> VirtualTime {
        self.current_time
    }
}

pub struct SimulationHarness {
    node: SimulatedRedisNode,
    rng: DeterministicRng,
    history: Vec<OperationResult>,
    buggify_enabled: bool,
    buggify_probability: f64,
}

impl SimulationHarness {
    pub fn new(seed: u64) -> Self {
        Self::with_config(seed, 0, false, 0.0)
    }

    pub fn with_config(seed: u64, start_epoch: i64, buggify_enabled: bool, buggify_probability: f64) -> Self {
        SimulationHarness {
            node: SimulatedRedisNode::new(start_epoch),
            rng: DeterministicRng::new(seed),
            history: Vec::new(),
            buggify_enabled,
            buggify_probability,
        }
    }

    pub fn advance_time(&mut self, time: VirtualTime) {
        self.node.advance_time(time);
    }

    pub fn advance_time_ms(&mut self, ms: u64) {
        let new_time = self.node.current_time() + Duration::from_millis(ms);
        self.node.advance_time(new_time);
    }

    pub fn execute(&mut self, client_id: usize, cmd: Command) -> RespValue {
        let invoke_time = self.node.current_time();

        if self.buggify_enabled && self.rng.gen_bool(self.buggify_probability) {
            let delay_ms = self.rng.gen_range(1, 10);
            let delayed_time = invoke_time + Duration::from_millis(delay_ms);
            self.node.advance_time(delayed_time);
        }

        let response = self.node.execute(&cmd);
        let complete_time = self.node.current_time();

        self.history.push(OperationResult {
            client_id,
            invoke_time,
            complete_time,
            command: cmd,
            response: response.clone(),
        });

        response
    }

    pub fn evict_expired(&mut self) -> usize {
        self.node.evict_expired()
    }

    pub fn history(&self) -> &[OperationResult] {
        &self.history
    }

    pub fn current_time(&self) -> VirtualTime {
        self.node.current_time()
    }

    pub fn rng(&mut self) -> &mut DeterministicRng {
        &mut self.rng
    }
}

pub struct ScenarioBuilder {
    operations: Vec<Operation>,
    seed: u64,
    start_epoch: i64,
    buggify_enabled: bool,
    buggify_probability: f64,
}

impl ScenarioBuilder {
    pub fn new(seed: u64) -> Self {
        ScenarioBuilder {
            operations: Vec::new(),
            seed,
            start_epoch: 0,
            buggify_enabled: false,
            buggify_probability: 0.0,
        }
    }

    pub fn with_buggify(mut self, probability: f64) -> Self {
        self.buggify_enabled = true;
        self.buggify_probability = probability;
        self
    }

    pub fn with_start_epoch(mut self, epoch: i64) -> Self {
        self.start_epoch = epoch;
        self
    }

    pub fn at_time(self, time_ms: u64) -> ScenarioAtTime {
        ScenarioAtTime {
            builder: self,
            time: VirtualTime::from_millis(time_ms),
        }
    }

    pub fn run(self) -> SimulationHarness {
        let mut harness = SimulationHarness::with_config(
            self.seed,
            self.start_epoch,
            self.buggify_enabled,
            self.buggify_probability,
        );

        let mut ops = self.operations;
        ops.sort_by_key(|op| op.time);

        for op in ops {
            harness.advance_time(op.time);
            harness.execute(op.client_id, op.command);
        }

        harness
    }

    pub fn run_with_eviction(self, eviction_interval_ms: u64) -> SimulationHarness {
        let mut harness = SimulationHarness::with_config(
            self.seed,
            self.start_epoch,
            self.buggify_enabled,
            self.buggify_probability,
        );

        let mut ops = self.operations;
        ops.sort_by_key(|op| op.time);

        let max_time = ops.last().map(|op| op.time).unwrap_or(VirtualTime::ZERO);
        let mut next_eviction = VirtualTime::from_millis(eviction_interval_ms);
        let mut op_idx = 0;

        while op_idx < ops.len() || harness.current_time() < max_time {
            let next_op_time = ops.get(op_idx).map(|op| op.time);

            match (next_op_time, next_eviction <= max_time) {
                (Some(op_time), true) if op_time <= next_eviction => {
                    harness.advance_time(op_time);
                    let op = &ops[op_idx];
                    harness.execute(op.client_id, op.command.clone());
                    op_idx += 1;
                }
                (_, true) => {
                    harness.advance_time(next_eviction);
                    harness.evict_expired();
                    next_eviction = next_eviction + Duration::from_millis(eviction_interval_ms);
                }
                (Some(op_time), false) => {
                    harness.advance_time(op_time);
                    let op = &ops[op_idx];
                    harness.execute(op.client_id, op.command.clone());
                    op_idx += 1;
                }
                (None, false) => break,
            }
        }

        harness
    }
}

pub struct ScenarioAtTime {
    builder: ScenarioBuilder,
    time: VirtualTime,
}

impl ScenarioAtTime {
    pub fn client(mut self, client_id: usize, cmd: Command) -> ScenarioBuilder {
        self.builder.operations.push(Operation {
            client_id,
            time: self.time,
            command: cmd,
        });
        self.builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;

    #[test]
    fn test_basic_set_get() {
        let harness = ScenarioBuilder::new(42)
            .at_time(0).client(1, Command::Set("key".into(), SDS::from_str("value")))
            .at_time(10).client(1, Command::Get("key".into()))
            .run();

        assert_eq!(harness.history().len(), 2);
        match &harness.history()[1].response {
            RespValue::BulkString(Some(data)) => {
                assert_eq!(data, b"value");
            }
            _ => panic!("Expected BulkString"),
        }
    }

    #[test]
    fn test_ttl_expiration_with_fast_forward() {
        let harness = ScenarioBuilder::new(42)
            .at_time(0).client(1, Command::SetEx("temp".into(), 1, SDS::from_str("expires")))
            .at_time(500).client(1, Command::Get("temp".into()))
            .at_time(1500).client(1, Command::Get("temp".into()))
            .run_with_eviction(100);

        assert_eq!(harness.history().len(), 3);

        match &harness.history()[1].response {
            RespValue::BulkString(Some(data)) => assert_eq!(data, b"expires"),
            _ => panic!("Should exist at 500ms"),
        }

        match &harness.history()[2].response {
            RespValue::BulkString(None) => {}
            _ => panic!("Should be expired at 1500ms"),
        }
    }

    #[test]
    fn test_ttl_boundary_race() {
        let harness = ScenarioBuilder::new(42)
            .at_time(0).client(1, Command::SetEx("race".into(), 1, SDS::from_str("data")))
            .at_time(999).client(1, Command::Get("race".into()))
            .at_time(1000).client(2, Command::Get("race".into()))
            .at_time(1001).client(3, Command::Get("race".into()))
            .run_with_eviction(100);

        match &harness.history()[1].response {
            RespValue::BulkString(Some(_)) => {}
            _ => panic!("Should exist at 999ms"),
        }

        match &harness.history()[3].response {
            RespValue::BulkString(None) => {}
            _ => panic!("Should be expired at 1001ms"),
        }
    }

    #[test]
    fn test_concurrent_increments() {
        let harness = ScenarioBuilder::new(42)
            .at_time(0).client(1, Command::Set("counter".into(), SDS::from_str("0")))
            .at_time(10).client(1, Command::Incr("counter".into()))
            .at_time(10).client(2, Command::Incr("counter".into()))
            .at_time(10).client(3, Command::Incr("counter".into()))
            .at_time(20).client(1, Command::Get("counter".into()))
            .run();

        match &harness.history().last().unwrap().response {
            RespValue::BulkString(Some(data)) => {
                assert_eq!(data, b"3");
            }
            _ => panic!("Expected counter to be 3"),
        }
    }

    #[test]
    fn test_deterministic_replay() {
        let run1 = ScenarioBuilder::new(12345)
            .at_time(0).client(1, Command::Set("a".into(), SDS::from_str("1")))
            .at_time(5).client(2, Command::Set("b".into(), SDS::from_str("2")))
            .at_time(10).client(1, Command::Get("a".into()))
            .at_time(10).client(2, Command::Get("b".into()))
            .run();

        let run2 = ScenarioBuilder::new(12345)
            .at_time(0).client(1, Command::Set("a".into(), SDS::from_str("1")))
            .at_time(5).client(2, Command::Set("b".into(), SDS::from_str("2")))
            .at_time(10).client(1, Command::Get("a".into()))
            .at_time(10).client(2, Command::Get("b".into()))
            .run();

        assert_eq!(run1.history().len(), run2.history().len());
        for (r1, r2) in run1.history().iter().zip(run2.history().iter()) {
            assert_eq!(format!("{:?}", r1.response), format!("{:?}", r2.response));
        }
    }

    #[test]
    fn test_buggify_chaos() {
        let harness = ScenarioBuilder::new(42)
            .with_buggify(0.5)
            .at_time(0).client(1, Command::Set("key".into(), SDS::from_str("value")))
            .at_time(100).client(1, Command::Get("key".into()))
            .run();

        match &harness.history().last().unwrap().response {
            RespValue::BulkString(Some(data)) => assert_eq!(data, b"value"),
            _ => panic!("Data should persist through chaos"),
        }
    }

    #[test]
    fn test_persist_cancels_expiration() {
        let harness = ScenarioBuilder::new(42)
            .at_time(0).client(1, Command::SetEx("persist_test".into(), 1, SDS::from_str("data")))
            .at_time(500).client(1, Command::Persist("persist_test".into()))
            .at_time(2000).client(1, Command::Get("persist_test".into()))
            .run_with_eviction(100);

        match &harness.history().last().unwrap().response {
            RespValue::BulkString(Some(data)) => assert_eq!(data, b"data"),
            _ => panic!("PERSIST should cancel expiration"),
        }
    }

    #[test]
    fn test_multi_seed_invariants() {
        for seed in 0..100 {
            let harness = ScenarioBuilder::new(seed)
                .with_buggify(0.1)
                .at_time(0).client(1, Command::Set("x".into(), SDS::from_str("0")))
                .at_time(10).client(1, Command::Incr("x".into()))
                .at_time(20).client(1, Command::Incr("x".into()))
                .at_time(30).client(1, Command::Get("x".into()))
                .run();

            match &harness.history().last().unwrap().response {
                RespValue::BulkString(Some(data)) => {
                    assert_eq!(data, b"2", "Seed {} failed invariant", seed);
                }
                _ => panic!("Seed {} produced unexpected response", seed),
            }
        }
    }
}
