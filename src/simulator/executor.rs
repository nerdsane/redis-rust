use super::*;
use super::network::Network;
use std::collections::{BinaryHeap, HashMap};

pub struct SimulationConfig {
    pub seed: u64,
    pub max_time: VirtualTime,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        SimulationConfig {
            seed: 42,
            max_time: VirtualTime::from_millis(60_000),
        }
    }
}

pub struct Simulation {
    config: SimulationConfig,
    current_time: VirtualTime,
    events: BinaryHeap<Event>,
    hosts: HashMap<HostId, Host>,
    network: Network,
    rng: DeterministicRng,
    next_timer_id: u64,
    next_host_id: usize,
    message_queue: Vec<Message>,
}

impl Simulation {
    pub fn new(config: SimulationConfig) -> Self {
        Simulation {
            rng: DeterministicRng::new(config.seed),
            current_time: VirtualTime::ZERO,
            events: BinaryHeap::new(),
            hosts: HashMap::new(),
            network: Network::new(),
            next_timer_id: 0,
            next_host_id: 0,
            message_queue: Vec::new(),
            config,
        }
    }

    pub fn add_host(&mut self, name: String) -> HostId {
        let id = HostId(self.next_host_id);
        self.next_host_id += 1;
        let host = Host::new(id, name);
        self.hosts.insert(id, host);
        
        self.events.push(Event {
            time: self.current_time,
            host_id: id,
            event_type: EventType::HostStart,
        });
        
        id
    }

    pub fn schedule_timer(&mut self, host_id: HostId, delay: Duration) -> TimerId {
        let timer_id = TimerId(self.next_timer_id);
        self.next_timer_id += 1;
        
        self.events.push(Event {
            time: self.current_time + delay,
            host_id,
            event_type: EventType::Timer(timer_id),
        });
        
        timer_id
    }

    pub fn send_message(&mut self, from: HostId, to: HostId, payload: Vec<u8>) {
        if let Some(delay) = self.network.should_deliver(from, to, &mut self.rng) {
            self.events.push(Event {
                time: self.current_time + delay,
                host_id: to,
                event_type: EventType::NetworkMessage(Message {
                    from,
                    to,
                    payload,
                }),
            });
        }
    }

    pub fn set_network_drop_rate(&mut self, rate: f64) {
        self.network.set_drop_rate(rate);
    }

    pub fn partition_hosts(&mut self, host1: HostId, host2: HostId) {
        self.network.partition(host1, host2);
    }

    pub fn heal_partition(&mut self, host1: HostId, host2: HostId) {
        self.network.heal_partition(host1, host2);
    }

    pub fn current_time(&self) -> VirtualTime {
        self.current_time
    }

    pub fn rng(&mut self) -> &mut DeterministicRng {
        &mut self.rng
    }

    pub fn run_until(&mut self, max_time: VirtualTime, mut event_handler: impl FnMut(&mut Self, &Event)) {
        while let Some(event) = self.events.pop() {
            if event.time > max_time {
                self.events.push(event);
                break;
            }
            
            self.current_time = event.time;
            event_handler(self, &event);
        }
    }

    pub fn run(&mut self, event_handler: impl FnMut(&mut Self, &Event)) {
        let max_time = self.config.max_time;
        self.run_until(max_time, event_handler);
    }
}
