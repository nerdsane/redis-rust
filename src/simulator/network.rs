use super::{HostId, VirtualTime, Duration, DeterministicRng};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct NetworkEvent {
    pub from: HostId,
    pub to: HostId,
    pub payload: Vec<u8>,
    pub delivery_time: VirtualTime,
}

#[derive(Debug, Clone)]
pub enum NetworkFault {
    Drop,
    Delay(Duration),
    Reorder,
}

#[derive(Debug, Clone)]
pub struct PacketDelay {
    pub min_latency: Duration,
    pub max_latency: Duration,
}

impl Default for PacketDelay {
    fn default() -> Self {
        PacketDelay {
            min_latency: Duration::from_millis(1),
            max_latency: Duration::from_millis(10),
        }
    }
}

pub struct Network {
    packet_delay: PacketDelay,
    drop_rate: f64,
    partition_map: HashMap<(HostId, HostId), bool>,
}

impl Network {
    pub fn new() -> Self {
        Network {
            packet_delay: PacketDelay::default(),
            drop_rate: 0.0,
            partition_map: HashMap::new(),
        }
    }

    pub fn set_drop_rate(&mut self, rate: f64) {
        self.drop_rate = rate.clamp(0.0, 1.0);
    }

    pub fn partition(&mut self, host1: HostId, host2: HostId) {
        self.partition_map.insert((host1, host2), true);
        self.partition_map.insert((host2, host1), true);
    }

    pub fn heal_partition(&mut self, host1: HostId, host2: HostId) {
        self.partition_map.remove(&(host1, host2));
        self.partition_map.remove(&(host2, host1));
    }

    pub fn should_deliver(
        &self,
        from: HostId,
        to: HostId,
        rng: &mut DeterministicRng,
    ) -> Option<Duration> {
        if self.partition_map.get(&(from, to)).copied().unwrap_or(false) {
            return None;
        }

        if rng.gen_bool(self.drop_rate) {
            return None;
        }

        let latency = rng.gen_range(
            self.packet_delay.min_latency.as_millis(),
            self.packet_delay.max_latency.as_millis(),
        );
        Some(Duration::from_millis(latency))
    }
}

pub struct Host {
    pub id: HostId,
    pub name: String,
}

impl Host {
    pub fn new(id: HostId, name: String) -> Self {
        Host { id, name }
    }
}
