use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReplicaId(pub u64);

impl ReplicaId {
    pub fn new(id: u64) -> Self {
        ReplicaId(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LamportClock {
    pub time: u64,
    pub replica_id: ReplicaId,
}

impl LamportClock {
    pub fn new(replica_id: ReplicaId) -> Self {
        LamportClock { time: 0, replica_id }
    }

    pub fn tick(&mut self) -> Self {
        self.time += 1;
        *self
    }

    pub fn update(&mut self, other: &LamportClock) {
        self.time = self.time.max(other.time) + 1;
    }

    pub fn merge(&self, other: &LamportClock) -> Self {
        LamportClock {
            time: self.time.max(other.time),
            replica_id: self.replica_id,
        }
    }
}

impl PartialOrd for LamportClock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LamportClock {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.time.cmp(&other.time) {
            Ordering::Equal => self.replica_id.0.cmp(&other.replica_id.0),
            other => other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwRegister<T> {
    pub value: Option<T>,
    pub timestamp: LamportClock,
    pub tombstone: bool,
}

impl<T: Clone> LwwRegister<T> {
    pub fn new(replica_id: ReplicaId) -> Self {
        LwwRegister {
            value: None,
            timestamp: LamportClock::new(replica_id),
            tombstone: false,
        }
    }

    pub fn with_value(value: T, timestamp: LamportClock) -> Self {
        LwwRegister {
            value: Some(value),
            timestamp,
            tombstone: false,
        }
    }

    pub fn set(&mut self, value: T, clock: &mut LamportClock) {
        let ts = clock.tick();
        self.value = Some(value);
        self.timestamp = ts;
        self.tombstone = false;
    }

    pub fn delete(&mut self, clock: &mut LamportClock) {
        let ts = clock.tick();
        self.value = None;
        self.timestamp = ts;
        self.tombstone = true;
    }

    pub fn merge(&self, other: &Self) -> Self {
        if other.timestamp > self.timestamp {
            other.clone()
        } else {
            self.clone()
        }
    }

    pub fn get(&self) -> Option<&T> {
        if self.tombstone {
            None
        } else {
            self.value.as_ref()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VectorClock {
    clocks: HashMap<ReplicaId, u64>,
}

impl VectorClock {
    pub fn new() -> Self {
        VectorClock {
            clocks: HashMap::new(),
        }
    }

    pub fn increment(&mut self, replica_id: ReplicaId) {
        let counter = self.clocks.entry(replica_id).or_insert(0);
        *counter += 1;
    }

    pub fn get(&self, replica_id: &ReplicaId) -> u64 {
        *self.clocks.get(replica_id).unwrap_or(&0)
    }

    pub fn merge(&self, other: &Self) -> Self {
        let mut merged = self.clocks.clone();
        for (replica_id, &count) in &other.clocks {
            let entry = merged.entry(*replica_id).or_insert(0);
            *entry = (*entry).max(count);
        }
        VectorClock { clocks: merged }
    }

    pub fn happens_before(&self, other: &Self) -> bool {
        let mut dominated = false;
        for (replica_id, &self_count) in &self.clocks {
            let other_count = other.get(replica_id);
            if self_count > other_count {
                return false;
            }
            if self_count < other_count {
                dominated = true;
            }
        }
        for (replica_id, &other_count) in &other.clocks {
            if !self.clocks.contains_key(replica_id) && other_count > 0 {
                dominated = true;
            }
        }
        dominated
    }

    pub fn concurrent_with(&self, other: &Self) -> bool {
        !self.happens_before(other) && !other.happens_before(self) && self != other
    }
}

impl PartialEq for VectorClock {
    fn eq(&self, other: &Self) -> bool {
        for (k, v) in &self.clocks {
            if other.get(k) != *v {
                return false;
            }
        }
        for (k, v) in &other.clocks {
            if self.get(k) != *v {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lww_register_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut clock1 = LamportClock::new(r1);
        let mut clock2 = LamportClock::new(r2);

        let mut reg1: LwwRegister<String> = LwwRegister::new(r1);
        let mut reg2: LwwRegister<String> = LwwRegister::new(r2);

        reg1.set("value1".to_string(), &mut clock1);
        reg2.set("value2".to_string(), &mut clock2);
        clock2.tick();
        reg2.set("value2_updated".to_string(), &mut clock2);

        let merged = reg1.merge(&reg2);
        assert_eq!(merged.get(), Some(&"value2_updated".to_string()));
    }

    #[test]
    fn test_vector_clock_happens_before() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut vc1 = VectorClock::new();
        let mut vc2 = VectorClock::new();

        vc1.increment(r1);
        vc2.increment(r1);
        vc2.increment(r2);

        assert!(vc1.happens_before(&vc2));
        assert!(!vc2.happens_before(&vc1));
    }

    #[test]
    fn test_vector_clock_concurrent() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut vc1 = VectorClock::new();
        let mut vc2 = VectorClock::new();

        vc1.increment(r1);
        vc2.increment(r2);

        assert!(vc1.concurrent_with(&vc2));
    }
}
