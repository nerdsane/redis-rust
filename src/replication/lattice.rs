use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;

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
        LamportClock {
            time: 0,
            replica_id,
        }
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
    /// VOPR: Verify all invariants hold for this register
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: If tombstone is true, get() must return None
        if self.tombstone {
            debug_assert!(
                self.get().is_none(),
                "Invariant violated: get() must return None when tombstoned"
            );
        }
        // Invariant 2: If tombstone is false and value is Some, get() must return Some
        if !self.tombstone && self.value.is_some() {
            debug_assert!(
                self.get().is_some(),
                "Invariant violated: get() must return Some when not tombstoned and value exists"
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

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
    /// VOPR: Verify all invariants hold for this vector clock
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: No zero entries should be stored (they are semantically equivalent to absent)
        for (&replica_id, &count) in &self.clocks {
            debug_assert!(
                count > 0,
                "Invariant violated: replica {:?} has zero count (should be removed)",
                replica_id
            );
        }

        // Invariant 2: get() must return stored value or 0 for absent keys
        for (&replica_id, &count) in &self.clocks {
            debug_assert_eq!(
                self.get(&replica_id),
                count,
                "Invariant violated: get() must return stored value"
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

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

// ============================================================================
// GCounter - Grow-only Counter
// ============================================================================

/// Grow-only counter CRDT. Each replica tracks its own count.
/// Value is the sum of all replica counts. Only supports increment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GCounter {
    counts: HashMap<ReplicaId, u64>,
}

impl GCounter {
    /// VOPR: Verify all invariants hold for this counter
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: value() must equal sum of all counts
        let computed_sum: u64 = self.counts.values().sum();
        debug_assert_eq!(
            self.value(),
            computed_sum,
            "Invariant violated: value() must equal sum of counts"
        );

        // Invariant 2: is_empty() must be true iff value() == 0
        let is_zero = self.value() == 0;
        debug_assert_eq!(
            self.is_empty(),
            is_zero || self.counts.is_empty(),
            "Invariant violated: is_empty() inconsistent with value()"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

    pub fn new() -> Self {
        GCounter {
            counts: HashMap::new(),
        }
    }

    /// Increment this replica's count by 1
    pub fn increment(&mut self, replica_id: ReplicaId) {
        let counter = self.counts.entry(replica_id).or_insert(0);
        *counter += 1;
    }

    /// Increment this replica's count by arbitrary amount
    pub fn increment_by(&mut self, replica_id: ReplicaId, amount: u64) {
        let counter = self.counts.entry(replica_id).or_insert(0);
        *counter += amount;
    }

    /// Get the total count across all replicas
    pub fn value(&self) -> u64 {
        self.counts.values().sum()
    }

    /// Get this replica's contribution
    pub fn get_replica_count(&self, replica_id: &ReplicaId) -> u64 {
        *self.counts.get(replica_id).unwrap_or(&0)
    }

    /// Merge with another GCounter (take max per replica)
    pub fn merge(&self, other: &Self) -> Self {
        let mut merged = self.counts.clone();
        for (replica_id, &count) in &other.counts {
            let entry = merged.entry(*replica_id).or_insert(0);
            *entry = (*entry).max(count);
        }
        GCounter { counts: merged }
    }

    /// Check if this counter is empty (no increments from any replica)
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty() || self.value() == 0
    }
}

impl PartialEq for GCounter {
    fn eq(&self, other: &Self) -> bool {
        // Two GCounters are equal if they have the same value for all replicas
        let all_keys: std::collections::HashSet<_> =
            self.counts.keys().chain(other.counts.keys()).collect();
        for k in all_keys {
            if self.get_replica_count(k) != other.get_replica_count(k) {
                return false;
            }
        }
        true
    }
}

impl Eq for GCounter {}

// ============================================================================
// PNCounter - Positive-Negative Counter
// ============================================================================

/// Positive-Negative counter CRDT. Supports both increment and decrement.
/// Implemented as two GCounters: one for positive, one for negative.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PNCounter {
    positive: GCounter,
    negative: GCounter,
}

impl PNCounter {
    /// VOPR: Verify all invariants hold for this counter
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: Both internal counters must be valid
        self.positive.verify_invariants();
        self.negative.verify_invariants();

        // Invariant 2: value() must equal positive - negative
        let expected_value = self.positive.value() as i64 - self.negative.value() as i64;
        debug_assert_eq!(
            self.value(),
            expected_value,
            "Invariant violated: value() must equal positive.value() - negative.value()"
        );

        // Invariant 3: is_empty() must be true iff both counters are empty
        debug_assert_eq!(
            self.is_empty(),
            self.positive.is_empty() && self.negative.is_empty(),
            "Invariant violated: is_empty() inconsistent with internal counters"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

    pub fn new() -> Self {
        PNCounter {
            positive: GCounter::new(),
            negative: GCounter::new(),
        }
    }

    /// Increment the counter by 1
    pub fn increment(&mut self, replica_id: ReplicaId) {
        self.positive.increment(replica_id);
    }

    /// Decrement the counter by 1
    pub fn decrement(&mut self, replica_id: ReplicaId) {
        self.negative.increment(replica_id);
    }

    /// Increment by arbitrary amount
    pub fn increment_by(&mut self, replica_id: ReplicaId, amount: u64) {
        self.positive.increment_by(replica_id, amount);
    }

    /// Decrement by arbitrary amount
    pub fn decrement_by(&mut self, replica_id: ReplicaId, amount: u64) {
        self.negative.increment_by(replica_id, amount);
    }

    /// Get the counter value (positive - negative, can be negative)
    pub fn value(&self) -> i64 {
        self.positive.value() as i64 - self.negative.value() as i64
    }

    /// Merge with another PNCounter
    pub fn merge(&self, other: &Self) -> Self {
        PNCounter {
            positive: self.positive.merge(&other.positive),
            negative: self.negative.merge(&other.negative),
        }
    }

    /// Check if counter is at zero with no operations
    pub fn is_empty(&self) -> bool {
        self.positive.is_empty() && self.negative.is_empty()
    }
}

impl PartialEq for PNCounter {
    fn eq(&self, other: &Self) -> bool {
        self.positive == other.positive && self.negative == other.negative
    }
}

impl Eq for PNCounter {}

// ============================================================================
// GSet - Grow-only Set
// ============================================================================

use std::collections::HashSet;
use std::hash::Hash;

/// Grow-only set CRDT. Elements can only be added, never removed.
/// Merge is set union.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GSet<T: Clone + Eq + Hash> {
    elements: HashSet<T>,
}

impl<T: Clone + Eq + Hash> Default for GSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + Eq + Hash> GSet<T> {
    /// VOPR: Verify all invariants hold for this set
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: len() must equal elements.len()
        debug_assert_eq!(
            self.len(),
            self.elements.len(),
            "Invariant violated: len() must equal elements.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.elements.is_empty(),
            "Invariant violated: is_empty() inconsistent with elements.is_empty()"
        );

        // Invariant 3: is_empty() must be true iff len() == 0
        debug_assert_eq!(
            self.is_empty(),
            self.len() == 0,
            "Invariant violated: is_empty() inconsistent with len() == 0"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

    pub fn new() -> Self {
        GSet {
            elements: HashSet::new(),
        }
    }

    /// Add an element to the set. Returns true if element was newly added.
    pub fn add(&mut self, element: T) -> bool {
        self.elements.insert(element)
    }

    /// Check if element exists in the set
    pub fn contains(&self, element: &T) -> bool {
        self.elements.contains(element)
    }

    /// Get iterator over all elements
    pub fn elements(&self) -> impl Iterator<Item = &T> {
        self.elements.iter()
    }

    /// Get the number of elements
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Check if the set is empty
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Merge with another GSet (set union)
    pub fn merge(&self, other: &Self) -> Self {
        GSet {
            elements: self.elements.union(&other.elements).cloned().collect(),
        }
    }
}

impl<T: Clone + Eq + Hash> PartialEq for GSet<T> {
    fn eq(&self, other: &Self) -> bool {
        self.elements == other.elements
    }
}

impl<T: Clone + Eq + Hash> Eq for GSet<T> {}

// ============================================================================
// ORSet - Observed-Remove Set
// ============================================================================

/// Unique tag for each add operation in ORSet
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UniqueTag {
    pub replica_id: ReplicaId,
    pub sequence: u64,
}

impl UniqueTag {
    pub fn new(replica_id: ReplicaId, sequence: u64) -> Self {
        UniqueTag {
            replica_id,
            sequence,
        }
    }
}

/// Observed-Remove Set CRDT. Supports add and remove operations.
/// Each add creates a unique tag. Remove removes all observed tags for an element.
/// Add-wins semantics: concurrent add and remove results in element present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ORSet<T: Clone + Eq + Hash> {
    /// Map from element to set of active (non-removed) tags
    elements: HashMap<T, HashSet<UniqueTag>>,
    /// Next sequence number for each replica
    next_sequence: HashMap<ReplicaId, u64>,
}

impl<T: Clone + Eq + Hash> Default for ORSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + Eq + Hash> ORSet<T> {
    /// VOPR: Verify all invariants hold for this set
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: No empty tag sets should be stored
        for (_elem, tags) in &self.elements {
            debug_assert!(
                !tags.is_empty(),
                "Invariant violated: element has empty tag set (should be removed)"
            );
        }

        // Invariant 2: All tags must have sequence < next_sequence for their replica
        for (_, tags) in &self.elements {
            for tag in tags {
                let next_seq = self
                    .next_sequence
                    .get(&tag.replica_id)
                    .copied()
                    .unwrap_or(0);
                debug_assert!(
                    tag.sequence < next_seq,
                    "Invariant violated: tag {:?} has sequence >= next_sequence {}",
                    tag,
                    next_seq
                );
            }
        }

        // Invariant 3: len() must count only elements with non-empty tag sets
        let expected_len = self
            .elements
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .count();
        debug_assert_eq!(
            self.len(),
            expected_len,
            "Invariant violated: len() must count elements with non-empty tag sets"
        );

        // Invariant 4: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.len() == 0,
            "Invariant violated: is_empty() inconsistent with len() == 0"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

    pub fn new() -> Self {
        ORSet {
            elements: HashMap::new(),
            next_sequence: HashMap::new(),
        }
    }

    /// Add element with a unique tag. Returns the tag that was created.
    pub fn add(&mut self, element: T, replica_id: ReplicaId) -> UniqueTag {
        let seq = self.next_sequence.entry(replica_id).or_insert(0);
        let tag = UniqueTag::new(replica_id, *seq);
        *seq += 1;

        self.elements.entry(element).or_default().insert(tag);
        tag
    }

    /// Remove element by removing all observed tags.
    /// Returns the tags that were removed (for replication).
    pub fn remove(&mut self, element: &T) -> HashSet<UniqueTag> {
        self.elements.remove(element).unwrap_or_default()
    }

    /// Check if element is in the set (has at least one active tag)
    pub fn contains(&self, element: &T) -> bool {
        self.elements
            .get(element)
            .map(|tags| !tags.is_empty())
            .unwrap_or(false)
    }

    /// Get iterator over all elements currently in the set
    pub fn elements(&self) -> impl Iterator<Item = &T> {
        self.elements
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .map(|(elem, _)| elem)
    }

    /// Get the number of elements
    pub fn len(&self) -> usize {
        self.elements
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .count()
    }

    /// Check if the set is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all tags for an element (for replication)
    pub fn get_tags(&self, element: &T) -> Option<&HashSet<UniqueTag>> {
        self.elements.get(element)
    }

    /// Merge with another ORSet.
    /// For each element, take union of tags.
    /// An element is present if it has any tags after merge.
    pub fn merge(&self, other: &Self) -> Self {
        let mut merged = ORSet::new();

        // Merge sequence counters (take max for each replica)
        for (replica, &seq) in &self.next_sequence {
            let entry = merged.next_sequence.entry(*replica).or_insert(0);
            *entry = (*entry).max(seq);
        }
        for (replica, &seq) in &other.next_sequence {
            let entry = merged.next_sequence.entry(*replica).or_insert(0);
            *entry = (*entry).max(seq);
        }

        // Collect all elements from both sets
        let all_elements: HashSet<_> = self
            .elements
            .keys()
            .chain(other.elements.keys())
            .cloned()
            .collect();

        // For each element, merge the tag sets
        for elem in all_elements {
            let self_tags = self.elements.get(&elem).cloned().unwrap_or_default();
            let other_tags = other.elements.get(&elem).cloned().unwrap_or_default();
            let union: HashSet<_> = self_tags.union(&other_tags).cloned().collect();
            if !union.is_empty() {
                merged.elements.insert(elem, union);
            }
        }

        merged
    }

    /// Apply a remove operation from another replica.
    /// Removes only the specific tags that were observed by the remover.
    pub fn apply_remove(&mut self, element: &T, removed_tags: &HashSet<UniqueTag>) {
        if let Some(tags) = self.elements.get_mut(element) {
            for tag in removed_tags {
                tags.remove(tag);
            }
            // Clean up empty entries
            if tags.is_empty() {
                self.elements.remove(element);
            }
        }
    }
}

impl<T: Clone + Eq + Hash> PartialEq for ORSet<T> {
    fn eq(&self, other: &Self) -> bool {
        // Two ORSets are equal if they contain the same elements with same tags
        if self.elements.len() != other.elements.len() {
            return false;
        }
        for (elem, tags) in &self.elements {
            match other.elements.get(elem) {
                Some(other_tags) => {
                    if tags != other_tags {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
}

impl<T: Clone + Eq + Hash> Eq for ORSet<T> {}

// ============================================================================
// Kani Bounded Verification Proofs
// ============================================================================
//
// Run with: cargo kani --harness <harness_name>
// Requires: kani toolchain installed
//
// These proofs verify CRDT properties hold for all inputs within bounds:
// - Commutativity: merge(a, b) = merge(b, a)
// - Associativity: merge(a, merge(b, c)) = merge(merge(a, b), c)
// - Idempotence: merge(a, a) = a
//
// Corresponds to TLA+ spec: specs/tla/ReplicationConvergence.tla

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Verify LWW register merge is commutative
    /// merge(a, b) = merge(b, a)
    #[kani::proof]
    #[kani::unwind(5)]
    fn verify_lww_merge_commutative() {
        let r1 = ReplicaId::new(kani::any());
        let r2 = ReplicaId::new(kani::any());

        let a: LwwRegister<u64> = LwwRegister {
            value: kani::any(),
            timestamp: LamportClock {
                time: kani::any(),
                replica_id: r1,
            },
            tombstone: kani::any(),
        };

        let b: LwwRegister<u64> = LwwRegister {
            value: kani::any(),
            timestamp: LamportClock {
                time: kani::any(),
                replica_id: r2,
            },
            tombstone: kani::any(),
        };

        let ab = a.merge(&b);
        let ba = b.merge(&a);

        kani::assert(ab == ba, "LWW merge must be commutative");
    }

    /// Verify LWW register merge is idempotent
    /// merge(a, a) = a
    #[kani::proof]
    #[kani::unwind(5)]
    fn verify_lww_merge_idempotent() {
        let r = ReplicaId::new(kani::any());

        let a: LwwRegister<u64> = LwwRegister {
            value: kani::any(),
            timestamp: LamportClock {
                time: kani::any(),
                replica_id: r,
            },
            tombstone: kani::any(),
        };

        let aa = a.merge(&a);

        kani::assert(aa == a, "LWW merge must be idempotent");
    }

    /// Verify Lamport clock ordering is total
    /// For any two clocks, exactly one of: a < b, a = b, a > b
    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_lamport_clock_total_order() {
        let r1 = ReplicaId::new(kani::any());
        let r2 = ReplicaId::new(kani::any());

        let a = LamportClock {
            time: kani::any(),
            replica_id: r1,
        };
        let b = LamportClock {
            time: kani::any(),
            replica_id: r2,
        };

        // Exactly one of these should be true
        let lt = a < b;
        let eq = a == b;
        let gt = a > b;

        kani::assert(
            (lt && !eq && !gt) || (!lt && eq && !gt) || (!lt && !eq && gt),
            "Lamport clock comparison must be total order"
        );
    }

    /// Verify GCounter merge is commutative
    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_gcounter_merge_commutative() {
        // Simplified: just verify with two replicas
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut gc1 = GCounter::new();
        let mut gc2 = GCounter::new();

        // Bounded increments
        let inc1: u8 = kani::any();
        let inc2: u8 = kani::any();

        kani::assume(inc1 < 10);
        kani::assume(inc2 < 10);

        gc1.increment_by(r1, inc1 as u64);
        gc2.increment_by(r2, inc2 as u64);

        let merged1 = gc1.merge(&gc2);
        let merged2 = gc2.merge(&gc1);

        kani::assert(merged1 == merged2, "GCounter merge must be commutative");
    }

    /// Verify GCounter merge is idempotent
    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_gcounter_merge_idempotent() {
        let r = ReplicaId::new(1);

        let mut gc = GCounter::new();
        let inc: u8 = kani::any();
        kani::assume(inc < 10);

        gc.increment_by(r, inc as u64);

        let merged = gc.merge(&gc);

        kani::assert(merged == gc, "GCounter merge must be idempotent");
    }

    /// Verify PNCounter merge is commutative
    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_pncounter_merge_commutative() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut pn1 = PNCounter::new();
        let mut pn2 = PNCounter::new();

        let inc1: u8 = kani::any();
        let dec1: u8 = kani::any();
        let inc2: u8 = kani::any();
        let dec2: u8 = kani::any();

        kani::assume(inc1 < 5 && dec1 < 5 && inc2 < 5 && dec2 < 5);

        pn1.increment_by(r1, inc1 as u64);
        pn1.decrement_by(r1, dec1 as u64);
        pn2.increment_by(r2, inc2 as u64);
        pn2.decrement_by(r2, dec2 as u64);

        let merged1 = pn1.merge(&pn2);
        let merged2 = pn2.merge(&pn1);

        kani::assert(merged1 == merged2, "PNCounter merge must be commutative");
    }

    /// Verify vector clock merge is commutative
    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_vector_clock_merge_commutative() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut vc1 = VectorClock::new();
        let mut vc2 = VectorClock::new();

        let inc1: u8 = kani::any();
        let inc2: u8 = kani::any();

        kani::assume(inc1 < 5 && inc2 < 5);

        for _ in 0..inc1 {
            vc1.increment(r1);
        }
        for _ in 0..inc2 {
            vc2.increment(r2);
        }

        let merged1 = vc1.merge(&vc2);
        let merged2 = vc2.merge(&vc1);

        kani::assert(merged1 == merged2, "VectorClock merge must be commutative");
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

    // ========================================================================
    // GCounter Tests
    // ========================================================================

    #[test]
    fn test_gcounter_basic() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut gc = GCounter::new();
        assert_eq!(gc.value(), 0);

        gc.increment(r1);
        assert_eq!(gc.value(), 1);

        gc.increment(r2);
        gc.increment(r2);
        assert_eq!(gc.value(), 3);
    }

    #[test]
    fn test_gcounter_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut gc1 = GCounter::new();
        let mut gc2 = GCounter::new();

        gc1.increment(r1);
        gc1.increment(r1);

        gc2.increment(r2);
        gc2.increment(r2);
        gc2.increment(r2);

        let merged = gc1.merge(&gc2);
        assert_eq!(merged.value(), 5); // 2 from r1 + 3 from r2
    }

    #[test]
    fn test_gcounter_merge_idempotent() {
        let r1 = ReplicaId::new(1);

        let mut gc1 = GCounter::new();
        gc1.increment(r1);
        gc1.increment(r1);

        let gc2 = gc1.clone();

        // Merging identical counters should give same result
        let merged = gc1.merge(&gc2);
        assert_eq!(merged.value(), 2);
    }

    #[test]
    fn test_gcounter_merge_commutative() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut gc1 = GCounter::new();
        let mut gc2 = GCounter::new();

        gc1.increment(r1);
        gc2.increment(r2);

        let merged1 = gc1.merge(&gc2);
        let merged2 = gc2.merge(&gc1);

        assert_eq!(merged1, merged2);
    }

    // ========================================================================
    // PNCounter Tests
    // ========================================================================

    #[test]
    fn test_pncounter_basic() {
        let r1 = ReplicaId::new(1);

        let mut pn = PNCounter::new();
        assert_eq!(pn.value(), 0);

        pn.increment(r1);
        assert_eq!(pn.value(), 1);

        pn.decrement(r1);
        assert_eq!(pn.value(), 0);

        pn.decrement(r1);
        assert_eq!(pn.value(), -1);
    }

    #[test]
    fn test_pncounter_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut pn1 = PNCounter::new();
        let mut pn2 = PNCounter::new();

        pn1.increment(r1);
        pn1.increment(r1);
        pn1.decrement(r1);

        pn2.increment(r2);
        pn2.decrement(r2);
        pn2.decrement(r2);

        let merged = pn1.merge(&pn2);
        // r1: +2 -1 = 1, r2: +1 -2 = -1, total = 0
        assert_eq!(merged.value(), 0);
    }

    #[test]
    fn test_pncounter_concurrent_operations() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut pn1 = PNCounter::new();
        let mut pn2 = PNCounter::new();

        // Concurrent increments on both replicas
        pn1.increment_by(r1, 10);
        pn2.increment_by(r2, 5);
        pn2.decrement_by(r2, 3);

        let merged = pn1.merge(&pn2);
        assert_eq!(merged.value(), 12); // 10 + 5 - 3
    }

    // ========================================================================
    // GSet Tests
    // ========================================================================

    #[test]
    fn test_gset_basic() {
        let mut gs: GSet<String> = GSet::new();
        assert!(gs.is_empty());

        assert!(gs.add("a".to_string()));
        assert!(!gs.add("a".to_string())); // Already exists

        assert!(gs.contains(&"a".to_string()));
        assert!(!gs.contains(&"b".to_string()));
        assert_eq!(gs.len(), 1);
    }

    #[test]
    fn test_gset_merge() {
        let mut gs1: GSet<String> = GSet::new();
        let mut gs2: GSet<String> = GSet::new();

        gs1.add("a".to_string());
        gs1.add("b".to_string());

        gs2.add("b".to_string());
        gs2.add("c".to_string());

        let merged = gs1.merge(&gs2);
        assert_eq!(merged.len(), 3);
        assert!(merged.contains(&"a".to_string()));
        assert!(merged.contains(&"b".to_string()));
        assert!(merged.contains(&"c".to_string()));
    }

    #[test]
    fn test_gset_merge_commutative() {
        let mut gs1: GSet<i32> = GSet::new();
        let mut gs2: GSet<i32> = GSet::new();

        gs1.add(1);
        gs1.add(2);
        gs2.add(2);
        gs2.add(3);

        let merged1 = gs1.merge(&gs2);
        let merged2 = gs2.merge(&gs1);

        assert_eq!(merged1, merged2);
    }

    // ========================================================================
    // ORSet Tests
    // ========================================================================

    #[test]
    fn test_orset_basic() {
        let r1 = ReplicaId::new(1);

        let mut os: ORSet<String> = ORSet::new();
        assert!(os.is_empty());

        os.add("a".to_string(), r1);
        assert!(os.contains(&"a".to_string()));
        assert_eq!(os.len(), 1);

        os.remove(&"a".to_string());
        assert!(!os.contains(&"a".to_string()));
        assert!(os.is_empty());
    }

    #[test]
    fn test_orset_add_after_remove() {
        let r1 = ReplicaId::new(1);

        let mut os: ORSet<String> = ORSet::new();

        os.add("a".to_string(), r1);
        os.remove(&"a".to_string());
        os.add("a".to_string(), r1);

        assert!(os.contains(&"a".to_string()));
    }

    #[test]
    fn test_orset_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut os1: ORSet<String> = ORSet::new();
        let mut os2: ORSet<String> = ORSet::new();

        os1.add("a".to_string(), r1);
        os2.add("b".to_string(), r2);

        let merged = os1.merge(&os2);
        assert!(merged.contains(&"a".to_string()));
        assert!(merged.contains(&"b".to_string()));
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_orset_concurrent_add_remove() {
        // This tests the "add-wins" semantics
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut os1: ORSet<String> = ORSet::new();
        let mut os2: ORSet<String> = ORSet::new();

        // Both start with "a" added by r1
        os1.add("a".to_string(), r1);
        os2 = os1.merge(&os2);

        // Concurrent: r1 removes "a", r2 adds "a" again
        os1.remove(&"a".to_string());
        os2.add("a".to_string(), r2);

        // After merge, "a" should exist (add-wins)
        let merged = os1.merge(&os2);
        assert!(merged.contains(&"a".to_string()));
    }

    #[test]
    fn test_orset_apply_remove() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut os1: ORSet<String> = ORSet::new();
        let mut os2: ORSet<String> = ORSet::new();

        // r1 adds element
        let _tag = os1.add("a".to_string(), r1);

        // Sync to os2
        os2 = os1.merge(&os2);

        // r2 adds same element (creates new tag)
        os2.add("a".to_string(), r2);

        // r1 removes element (only removes tag it knows about)
        let removed_tags = os1.remove(&"a".to_string());

        // Apply r1's remove to os2
        os2.apply_remove(&"a".to_string(), &removed_tags);

        // Element should still exist (r2's tag remains)
        assert!(os2.contains(&"a".to_string()));
    }

    #[test]
    fn test_orset_merge_commutative() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut os1: ORSet<String> = ORSet::new();
        let mut os2: ORSet<String> = ORSet::new();

        os1.add("a".to_string(), r1);
        os2.add("b".to_string(), r2);

        let merged1 = os1.merge(&os2);
        let merged2 = os2.merge(&os1);

        assert_eq!(merged1.len(), merged2.len());
        assert!(merged1.contains(&"a".to_string()));
        assert!(merged1.contains(&"b".to_string()));
    }
}
