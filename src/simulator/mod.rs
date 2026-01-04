mod executor;
mod time;
mod network;
mod rng;
pub mod harness;
pub mod multi_node;
pub mod crash;
pub mod dst;
pub mod dst_integration;
pub mod partition_tests;
pub mod connection;

pub use executor::{Simulation, SimulationConfig};
pub use time::{VirtualTime, Duration};
pub use network::{Host, NetworkEvent, PacketDelay, NetworkFault};
pub use rng::{DeterministicRng, buggify};
pub use harness::{SimulationHarness, SimulatedRedisNode, ScenarioBuilder};
pub use multi_node::{MultiNodeSimulation, TimestampedOperation, LinearizabilityResult, check_single_key_linearizability};
pub use crash::{CrashSimulator, CrashConfig, CrashReason, NodeState, NodeSnapshot};
pub use dst::{DSTSimulation, DSTConfig, BatchRunner, BatchResult, SimulationResult};
pub use partition_tests::{PartitionConfig, PartitionTestResult, PartitionBatchResult, run_partition_test, run_partition_test_batch};
pub use connection::{SimulatedConnection, SimulatedReadBuffer, SimulatedWriteBuffer, PipelineSimulator, PipelineResult, ExecutionRecord};

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostId(pub usize);

#[derive(Debug, Clone)]
pub struct Event {
    pub time: VirtualTime,
    pub host_id: HostId,
    pub event_type: EventType,
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl Eq for Event {}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        other.time.cmp(&self.time)
    }
}

#[derive(Debug, Clone)]
pub enum EventType {
    Timer(TimerId),
    NetworkMessage(Message),
    HostStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimerId(pub u64);

#[derive(Debug, Clone)]
pub struct Message {
    pub from: HostId,
    pub to: HostId,
    pub payload: Vec<u8>,
}
