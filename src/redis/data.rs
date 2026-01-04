use std::collections::{HashMap, HashSet, VecDeque};
use std::cmp::Ordering;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SDS {
    data: Vec<u8>,
}

impl SDS {
    /// VOPR: Verify all invariants hold for this SDS
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must equal data.len()
        debug_assert_eq!(
            self.len(),
            self.data.len(),
            "Invariant violated: len() must equal data.len()"
        );

        // Invariant 2: is_empty() must be consistent with data.is_empty()
        debug_assert_eq!(
            self.is_empty(),
            self.data.is_empty(),
            "Invariant violated: is_empty() must equal data.is_empty()"
        );

        // Invariant 3: is_empty() iff len() == 0
        debug_assert_eq!(
            self.is_empty(),
            self.len() == 0,
            "Invariant violated: is_empty() must equal len() == 0"
        );

        // Invariant 4: as_bytes() must return data slice
        debug_assert_eq!(
            self.as_bytes().len(),
            self.data.len(),
            "Invariant violated: as_bytes().len() must equal data.len()"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn new(data: Vec<u8>) -> Self {
        let sds = SDS { data };

        // TigerStyle: Postcondition - verify construction succeeded
        sds.verify_invariants();
        sds
    }

    pub fn from_str(s: &str) -> Self {
        let sds = SDS {
            data: s.as_bytes().to_vec(),
        };

        // TigerStyle: Postconditions
        debug_assert_eq!(
            sds.len(),
            s.len(),
            "Postcondition violated: SDS len must equal source string len"
        );

        sds.verify_invariants();
        sds
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn to_string(&self) -> String {
        String::from_utf8_lossy(&self.data).to_string()
    }

    pub fn append(&mut self, other: &SDS) {
        // TigerStyle: Preconditions - capture state for postcondition check
        #[cfg(debug_assertions)]
        let pre_len = self.data.len();
        #[cfg(debug_assertions)]
        let other_len = other.data.len();

        self.data.extend_from_slice(&other.data);

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.data.len(),
                pre_len + other_len,
                "Postcondition violated: len must equal pre_len + other_len after append"
            );
        }

        self.verify_invariants();
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(SDS),
    List(RedisList),
    Set(RedisSet),
    Hash(RedisHash),
    SortedSet(RedisSortedSet),
    Null,
}

impl Value {
    pub fn as_string(&self) -> Option<&SDS> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&RedisList> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_set(&self) -> Option<&RedisSet> {
        match self {
            Value::Set(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_hash(&self) -> Option<&RedisHash> {
        match self {
            Value::Hash(h) => Some(h),
            _ => None,
        }
    }

    pub fn as_sorted_set(&self) -> Option<&RedisSortedSet> {
        match self {
            Value::SortedSet(zs) => Some(zs),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RedisList {
    items: VecDeque<SDS>,
}

impl RedisList {
    pub fn new() -> Self {
        RedisList {
            items: VecDeque::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this list
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual VecDeque size
        debug_assert_eq!(
            self.len(),
            self.items.len(),
            "Invariant violated: len() must equal items.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.items.is_empty(),
            "Invariant violated: is_empty() must equal items.is_empty()"
        );

        // Invariant 3: is_empty() iff len() == 0
        debug_assert_eq!(
            self.is_empty(),
            self.len() == 0,
            "Invariant violated: is_empty() must equal len() == 0"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn lpush(&mut self, value: SDS) {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();

        self.items.push_front(value.clone());

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                pre_len + 1,
                "Postcondition violated: len must increase by 1 after lpush"
            );
            debug_assert_eq!(
                self.items.front().map(|v| v.to_string()),
                Some(value.to_string()),
                "Postcondition violated: pushed value must be at front"
            );
        }

        self.verify_invariants();
    }

    pub fn rpush(&mut self, value: SDS) {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();

        self.items.push_back(value.clone());

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                pre_len + 1,
                "Postcondition violated: len must increase by 1 after rpush"
            );
            debug_assert_eq!(
                self.items.back().map(|v| v.to_string()),
                Some(value.to_string()),
                "Postcondition violated: pushed value must be at back"
            );
        }

        self.verify_invariants();
    }

    pub fn lpop(&mut self) -> Option<SDS> {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();
        #[cfg(debug_assertions)]
        let was_empty = self.items.is_empty();

        let result = self.items.pop_front();

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            if was_empty {
                debug_assert!(result.is_none(), "Postcondition violated: pop from empty must return None");
                debug_assert_eq!(self.items.len(), 0, "Postcondition violated: empty list must stay empty");
            } else {
                debug_assert!(result.is_some(), "Postcondition violated: pop from non-empty must return Some");
                debug_assert_eq!(self.items.len(), pre_len - 1, "Postcondition violated: len must decrease by 1");
            }
        }

        self.verify_invariants();
        result
    }

    pub fn rpop(&mut self) -> Option<SDS> {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();
        #[cfg(debug_assertions)]
        let was_empty = self.items.is_empty();

        let result = self.items.pop_back();

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            if was_empty {
                debug_assert!(result.is_none(), "Postcondition violated: pop from empty must return None");
                debug_assert_eq!(self.items.len(), 0, "Postcondition violated: empty list must stay empty");
            } else {
                debug_assert!(result.is_some(), "Postcondition violated: pop from non-empty must return Some");
                debug_assert_eq!(self.items.len(), pre_len - 1, "Postcondition violated: len must decrease by 1");
            }
        }

        self.verify_invariants();
        result
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn range(&self, start: isize, stop: isize) -> Vec<SDS> {
        let len = self.items.len() as isize;
        let start = if start < 0 { (len + start).max(0) } else { start.min(len) };
        let stop = if stop < 0 { (len + stop).max(-1) } else { stop.min(len - 1) };

        if start > stop || start >= len {
            return Vec::new();
        }

        self.items
            .iter()
            .skip(start as usize)
            .take((stop - start + 1) as usize)
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RedisSet {
    members: HashSet<String>,
}

impl RedisSet {
    pub fn new() -> Self {
        RedisSet {
            members: HashSet::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this set
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual HashSet size
        debug_assert_eq!(
            self.len(),
            self.members.len(),
            "Invariant violated: len() must equal members.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.members.is_empty(),
            "Invariant violated: is_empty() must equal members.is_empty()"
        );

        // Invariant 3: All members must be retrievable via contains()
        for member in &self.members {
            debug_assert!(
                self.members.contains(member),
                "Invariant violated: member '{}' must be in set",
                member
            );
        }

        // Invariant 4: members() count must equal len()
        debug_assert_eq!(
            self.members().len(),
            self.len(),
            "Invariant violated: members().len() must equal len()"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn add(&mut self, member: SDS) -> bool {
        let member_str = member.to_string();

        #[cfg(debug_assertions)]
        let pre_len = self.members.len();
        #[cfg(debug_assertions)]
        let already_exists = self.members.contains(&member_str);

        let inserted = self.members.insert(member_str.clone());

        // TigerStyle: Postconditions
        debug_assert!(
            self.members.contains(&member_str),
            "Postcondition violated: member must exist after add"
        );
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                inserted, !already_exists,
                "Postcondition violated: insert result must match prior non-existence"
            );
            let expected_len = if already_exists { pre_len } else { pre_len + 1 };
            debug_assert_eq!(
                self.members.len(),
                expected_len,
                "Postcondition violated: len must be correct after add"
            );
        }

        self.verify_invariants();
        inserted
    }

    pub fn remove(&mut self, member: &SDS) -> bool {
        let member_str = member.to_string();

        #[cfg(debug_assertions)]
        let pre_len = self.members.len();
        #[cfg(debug_assertions)]
        let existed = self.members.contains(&member_str);

        let removed = self.members.remove(&member_str);

        // TigerStyle: Postconditions
        debug_assert!(
            !self.members.contains(&member_str),
            "Postcondition violated: member must not exist after remove"
        );
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                removed, existed,
                "Postcondition violated: remove result must match prior existence"
            );
            let expected_len = if existed { pre_len - 1 } else { pre_len };
            debug_assert_eq!(
                self.members.len(),
                expected_len,
                "Postcondition violated: len must be correct after remove"
            );
        }

        self.verify_invariants();
        removed
    }

    pub fn contains(&self, member: &SDS) -> bool {
        self.members.contains(&member.to_string())
    }

    pub fn members(&self) -> Vec<SDS> {
        self.members
            .iter()
            .map(|s| SDS::from_str(s))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RedisHash {
    fields: HashMap<String, SDS>,
}

impl RedisHash {
    pub fn new() -> Self {
        RedisHash {
            fields: HashMap::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this hash
    /// Called in debug builds after every mutation
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual HashMap size
        debug_assert_eq!(
            self.len(),
            self.fields.len(),
            "Invariant violated: len() must equal fields.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.fields.is_empty(),
            "Invariant violated: is_empty() must equal fields.is_empty()"
        );

        // Invariant 3: All keys should be retrievable
        for (key, value) in &self.fields {
            let key_sds = SDS::from_str(key);
            debug_assert!(
                self.fields.get(key).is_some(),
                "Invariant violated: key '{}' must be retrievable",
                key
            );
            debug_assert_eq!(
                self.fields.get(key),
                Some(value),
                "Invariant violated: value for key '{}' must be consistent",
                key
            );
            // Invariant 4: Key converted to SDS and back should match
            debug_assert_eq!(
                key_sds.to_string(),
                *key,
                "Invariant violated: key roundtrip must be stable"
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn set(&mut self, field: SDS, value: SDS) {
        let field_str = field.to_string();

        // TigerStyle: Precondition - capture state for postcondition check
        #[cfg(debug_assertions)]
        let expected_len = if self.fields.contains_key(&field_str) {
            self.fields.len()
        } else {
            self.fields.len() + 1
        };

        self.fields.insert(field_str.clone(), value.clone());

        // TigerStyle: Postcondition - verify the set succeeded
        debug_assert!(
            self.fields.contains_key(&field_str),
            "Postcondition violated: field must exist after set"
        );
        debug_assert_eq!(
            self.fields.get(&field_str),
            Some(&value),
            "Postcondition violated: value must match after set"
        );
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.fields.len(),
            expected_len,
            "Postcondition violated: len must be correct after set"
        );

        self.verify_invariants();
    }

    pub fn get(&self, field: &SDS) -> Option<&SDS> {
        self.fields.get(&field.to_string())
    }

    pub fn delete(&mut self, field: &SDS) -> bool {
        let field_str = field.to_string();

        // TigerStyle: Precondition - capture state for postcondition check
        #[cfg(debug_assertions)]
        let pre_len = self.fields.len();
        #[cfg(debug_assertions)]
        let existed = self.fields.contains_key(&field_str);

        let removed = self.fields.remove(&field_str).is_some();

        // TigerStyle: Postconditions
        debug_assert!(
            !self.fields.contains_key(&field_str),
            "Postcondition violated: field must not exist after delete"
        );
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                removed, existed,
                "Postcondition violated: remove result must match existence"
            );
            let expected_len = if existed { pre_len - 1 } else { pre_len };
            debug_assert_eq!(
                self.fields.len(),
                expected_len,
                "Postcondition violated: len must be correct after delete"
            );
        }

        self.verify_invariants();
        removed
    }

    pub fn exists(&self, field: &SDS) -> bool {
        self.fields.contains_key(&field.to_string())
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn keys(&self) -> Vec<SDS> {
        self.fields.keys().map(|k| SDS::from_str(k)).collect()
    }

    pub fn values(&self) -> Vec<SDS> {
        self.fields.values().cloned().collect()
    }

    pub fn get_all(&self) -> Vec<(SDS, SDS)> {
        self.fields
            .iter()
            .map(|(k, v)| (SDS::from_str(k), v.clone()))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct RedisSortedSet {
    members: HashMap<String, f64>,
    sorted_members: Vec<(String, f64)>,
}

impl RedisSortedSet {
    pub fn new() -> Self {
        RedisSortedSet {
            members: HashMap::new(),
            sorted_members: Vec::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this sorted set
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual HashMap size
        debug_assert_eq!(
            self.len(),
            self.members.len(),
            "Invariant violated: len() must equal members.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.members.is_empty(),
            "Invariant violated: is_empty() must equal members.is_empty()"
        );

        // Invariant 3: members and sorted_members must have same length
        debug_assert_eq!(
            self.members.len(),
            self.sorted_members.len(),
            "Invariant violated: members and sorted_members must have same length"
        );

        // Invariant 4: sorted_members must be sorted by (score, member)
        debug_assert!(
            self.is_sorted(),
            "Invariant violated: sorted_members must be sorted by (score, member)"
        );

        // Invariant 5: Every member in HashMap must be in sorted_members with matching score
        for (member, score) in &self.members {
            let found = self.sorted_members.iter().find(|(m, _)| m == member);
            debug_assert!(
                found.is_some(),
                "Invariant violated: member '{}' in HashMap but not in sorted_members",
                member
            );
            debug_assert_eq!(
                found.map(|(_, s)| *s),
                Some(*score),
                "Invariant violated: score mismatch for member '{}'",
                member
            );
        }

        // Invariant 6: Every member in sorted_members must be in HashMap
        for (member, score) in &self.sorted_members {
            debug_assert!(
                self.members.contains_key(member),
                "Invariant violated: member '{}' in sorted_members but not in HashMap",
                member
            );
            debug_assert_eq!(
                self.members.get(member),
                Some(score),
                "Invariant violated: score mismatch for member '{}' in HashMap",
                member
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn add(&mut self, member: SDS, score: f64) -> bool {
        let key = member.to_string();

        // TigerStyle: Preconditions - capture state for postcondition check
        #[cfg(debug_assertions)]
        let pre_len = self.members.len();
        let is_new = !self.members.contains_key(&key);

        self.members.insert(key.clone(), score);
        self.rebuild_sorted();

        // TigerStyle: Postconditions
        debug_assert!(
            self.members.contains_key(&key),
            "Postcondition violated: member must exist after add"
        );
        debug_assert_eq!(
            self.members.get(&key),
            Some(&score),
            "Postcondition violated: score must match after add"
        );
        #[cfg(debug_assertions)]
        {
            let expected_len = if is_new { pre_len + 1 } else { pre_len };
            debug_assert_eq!(
                self.members.len(),
                expected_len,
                "Postcondition violated: len must be correct after add"
            );
        }

        self.verify_invariants();
        is_new
    }

    pub fn remove(&mut self, member: &SDS) -> bool {
        let key = member.to_string();

        // TigerStyle: Preconditions - capture state for postcondition check
        #[cfg(debug_assertions)]
        let pre_len = self.members.len();
        #[cfg(debug_assertions)]
        let existed = self.members.contains_key(&key);

        let removed = self.members.remove(&key).is_some();
        if removed {
            self.rebuild_sorted();
        }

        // TigerStyle: Postconditions
        debug_assert!(
            !self.members.contains_key(&key),
            "Postcondition violated: member must not exist after remove"
        );
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                removed, existed,
                "Postcondition violated: remove result must match prior existence"
            );
            let expected_len = if existed { pre_len - 1 } else { pre_len };
            debug_assert_eq!(
                self.members.len(),
                expected_len,
                "Postcondition violated: len must be correct after remove"
            );
        }

        self.verify_invariants();
        removed
    }

    pub fn score(&self, member: &SDS) -> Option<f64> {
        self.members.get(&member.to_string()).copied()
    }

    pub fn rank(&self, member: &SDS) -> Option<usize> {
        let key = member.to_string();
        self.sorted_members.iter().position(|(m, _)| m == &key)
    }

    pub fn range(&self, start: isize, stop: isize) -> Vec<(SDS, f64)> {
        let len = self.sorted_members.len() as isize;
        let start = if start < 0 { (len + start).max(0) } else { start.min(len) };
        let stop = if stop < 0 { (len + stop).max(-1) } else { stop.min(len - 1) };

        if start > stop || start >= len {
            return Vec::new();
        }

        self.sorted_members
            .iter()
            .skip(start as usize)
            .take((stop - start + 1) as usize)
            .map(|(m, s)| (SDS::from_str(m), *s))
            .collect()
    }

    pub fn rev_range(&self, start: isize, stop: isize) -> Vec<(SDS, f64)> {
        let len = self.sorted_members.len() as isize;
        let start = if start < 0 { (len + start).max(0) } else { start.min(len) };
        let stop = if stop < 0 { (len + stop).max(-1) } else { stop.min(len - 1) };

        if start > stop || start >= len {
            return Vec::new();
        }

        self.sorted_members
            .iter()
            .rev()
            .skip(start as usize)
            .take((stop - start + 1) as usize)
            .map(|(m, s)| (SDS::from_str(m), *s))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    fn rebuild_sorted(&mut self) {
        self.sorted_members = self.members.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.sorted_members.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        // TigerStyle: Assert invariants after mutation
        debug_assert_eq!(
            self.members.len(),
            self.sorted_members.len(),
            "Invariant violated: members and sorted_members must have same length"
        );
        debug_assert!(
            self.is_sorted(),
            "Invariant violated: sorted_members must be sorted by (score, member)"
        );
    }

    /// TigerStyle: Verify sorted invariant (used in debug_assert)
    fn is_sorted(&self) -> bool {
        self.sorted_members.windows(2).all(|w| {
            let cmp = w[0].1.partial_cmp(&w[1].1).unwrap_or(Ordering::Equal);
            cmp == Ordering::Less || (cmp == Ordering::Equal && w[0].0 <= w[1].0)
        })
    }
}

impl PartialEq for RedisSortedSet {
    fn eq(&self, other: &Self) -> bool {
        self.members == other.members
    }
}

#[cfg(test)]
mod sorted_set_tests {
    use super::*;

    fn create_test_set() -> RedisSortedSet {
        let mut zset = RedisSortedSet::new();
        zset.add(SDS::from_str("alice"), 100.0);
        zset.add(SDS::from_str("bob"), 200.0);
        zset.add(SDS::from_str("charlie"), 150.0);
        zset.add(SDS::from_str("dave"), 50.0);
        zset
    }

    #[test]
    fn test_sorted_set_ordering() {
        let zset = create_test_set();

        // Should be sorted by score: dave(50), alice(100), charlie(150), bob(200)
        let range = zset.range(0, -1);
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].0.to_string(), "dave");
        assert_eq!(range[0].1, 50.0);
        assert_eq!(range[1].0.to_string(), "alice");
        assert_eq!(range[2].0.to_string(), "charlie");
        assert_eq!(range[3].0.to_string(), "bob");
    }

    #[test]
    fn test_rev_range_full() {
        let zset = create_test_set();

        // rev_range should return highest scores first: bob(200), charlie(150), alice(100), dave(50)
        let range = zset.rev_range(0, -1);
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].0.to_string(), "bob");
        assert_eq!(range[0].1, 200.0);
        assert_eq!(range[1].0.to_string(), "charlie");
        assert_eq!(range[2].0.to_string(), "alice");
        assert_eq!(range[3].0.to_string(), "dave");
    }

    #[test]
    fn test_rev_range_subset() {
        let zset = create_test_set();

        // Get top 2 scores
        let range = zset.rev_range(0, 1);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].0.to_string(), "bob");
        assert_eq!(range[1].0.to_string(), "charlie");
    }

    #[test]
    fn test_rev_range_negative_indices() {
        let zset = create_test_set();

        // Last 2 elements in reverse order (lowest scores)
        let range = zset.rev_range(-2, -1);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].0.to_string(), "alice");
        assert_eq!(range[1].0.to_string(), "dave");
    }

    #[test]
    fn test_rev_range_empty_set() {
        let zset = RedisSortedSet::new();
        let range = zset.rev_range(0, -1);
        assert!(range.is_empty());
    }

    #[test]
    fn test_rev_range_out_of_bounds() {
        let zset = create_test_set();

        // Start beyond length
        let range = zset.rev_range(10, 20);
        assert!(range.is_empty());

        // Invalid range (start > stop after normalization)
        let range = zset.rev_range(3, 1);
        assert!(range.is_empty());
    }

    #[test]
    fn test_range_vs_rev_range_symmetry() {
        let zset = create_test_set();

        let forward = zset.range(0, -1);
        let reverse = zset.rev_range(0, -1);

        assert_eq!(forward.len(), reverse.len());

        // First element of forward should equal last element of reverse
        assert_eq!(forward[0].0.to_string(), reverse[3].0.to_string());
        assert_eq!(forward[3].0.to_string(), reverse[0].0.to_string());
    }

    #[test]
    fn test_sorted_set_with_equal_scores() {
        let mut zset = RedisSortedSet::new();
        zset.add(SDS::from_str("zebra"), 100.0);
        zset.add(SDS::from_str("apple"), 100.0);
        zset.add(SDS::from_str("mango"), 100.0);

        // Same score: should be sorted lexicographically
        let range = zset.range(0, -1);
        assert_eq!(range[0].0.to_string(), "apple");
        assert_eq!(range[1].0.to_string(), "mango");
        assert_eq!(range[2].0.to_string(), "zebra");

        // rev_range with equal scores: reverse lexicographic within same score
        let rev = zset.rev_range(0, -1);
        assert_eq!(rev[0].0.to_string(), "zebra");
        assert_eq!(rev[1].0.to_string(), "mango");
        assert_eq!(rev[2].0.to_string(), "apple");
    }

    #[test]
    fn test_sorted_set_invariants_maintained() {
        let mut zset = RedisSortedSet::new();

        // Add elements
        zset.add(SDS::from_str("a"), 1.0);
        assert_eq!(zset.len(), 1);
        assert!(zset.is_sorted());

        // Update score
        zset.add(SDS::from_str("a"), 5.0);
        assert_eq!(zset.len(), 1); // Should not add duplicate
        assert_eq!(zset.score(&SDS::from_str("a")), Some(5.0));
        assert!(zset.is_sorted());

        // Add more and remove
        zset.add(SDS::from_str("b"), 3.0);
        zset.add(SDS::from_str("c"), 7.0);
        assert_eq!(zset.len(), 3);
        assert!(zset.is_sorted());

        zset.remove(&SDS::from_str("b"));
        assert_eq!(zset.len(), 2);
        assert!(zset.is_sorted());
    }
}

#[cfg(test)]
mod hash_tests {
    use super::*;

    #[test]
    fn test_hash_set_and_get() {
        let mut hash = RedisHash::new();

        // Set a field
        hash.set(SDS::from_str("name"), SDS::from_str("Alice"));
        assert_eq!(hash.len(), 1);
        assert!(!hash.is_empty());

        // Get the field
        let value = hash.get(&SDS::from_str("name"));
        assert!(value.is_some());
        assert_eq!(value.unwrap().to_string(), "Alice");

        // Update the field
        hash.set(SDS::from_str("name"), SDS::from_str("Bob"));
        assert_eq!(hash.len(), 1); // Should not increase length
        assert_eq!(hash.get(&SDS::from_str("name")).unwrap().to_string(), "Bob");
    }

    #[test]
    fn test_hash_delete() {
        let mut hash = RedisHash::new();

        hash.set(SDS::from_str("a"), SDS::from_str("1"));
        hash.set(SDS::from_str("b"), SDS::from_str("2"));
        assert_eq!(hash.len(), 2);

        // Delete existing
        let removed = hash.delete(&SDS::from_str("a"));
        assert!(removed);
        assert_eq!(hash.len(), 1);
        assert!(!hash.exists(&SDS::from_str("a")));

        // Delete non-existing
        let removed = hash.delete(&SDS::from_str("nonexistent"));
        assert!(!removed);
        assert_eq!(hash.len(), 1);
    }

    #[test]
    fn test_hash_keys_and_values() {
        let mut hash = RedisHash::new();

        hash.set(SDS::from_str("k1"), SDS::from_str("v1"));
        hash.set(SDS::from_str("k2"), SDS::from_str("v2"));
        hash.set(SDS::from_str("k3"), SDS::from_str("v3"));

        let keys = hash.keys();
        assert_eq!(keys.len(), 3);

        let values = hash.values();
        assert_eq!(values.len(), 3);

        let all = hash.get_all();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_hash_invariants_maintained() {
        let mut hash = RedisHash::new();

        // Empty hash
        assert!(hash.is_empty());
        assert_eq!(hash.len(), 0);

        // Add elements
        hash.set(SDS::from_str("counter"), SDS::from_str("0"));
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 1);
        assert!(hash.exists(&SDS::from_str("counter")));

        // Update (invariants checked by verify_invariants in debug)
        hash.set(SDS::from_str("counter"), SDS::from_str("10"));
        assert_eq!(hash.len(), 1);
        assert_eq!(hash.get(&SDS::from_str("counter")).unwrap().to_string(), "10");

        // Delete (invariants checked by verify_invariants in debug)
        hash.delete(&SDS::from_str("counter"));
        assert!(hash.is_empty());
        assert_eq!(hash.len(), 0);
        assert!(!hash.exists(&SDS::from_str("counter")));
    }
}

#[cfg(test)]
mod list_tests {
    use super::*;

    #[test]
    fn test_list_lpush_and_lpop() {
        let mut list = RedisList::new();

        // lpush adds to front
        list.lpush(SDS::from_str("first"));
        assert_eq!(list.len(), 1);
        assert!(!list.is_empty());

        list.lpush(SDS::from_str("second"));
        assert_eq!(list.len(), 2);

        // lpop removes from front (LIFO for lpush/lpop)
        let val = list.lpop();
        assert_eq!(val.unwrap().to_string(), "second");
        assert_eq!(list.len(), 1);

        let val = list.lpop();
        assert_eq!(val.unwrap().to_string(), "first");
        assert!(list.is_empty());

        // lpop from empty
        let val = list.lpop();
        assert!(val.is_none());
    }

    #[test]
    fn test_list_rpush_and_rpop() {
        let mut list = RedisList::new();

        // rpush adds to back
        list.rpush(SDS::from_str("first"));
        list.rpush(SDS::from_str("second"));
        assert_eq!(list.len(), 2);

        // rpop removes from back (LIFO for rpush/rpop)
        let val = list.rpop();
        assert_eq!(val.unwrap().to_string(), "second");

        let val = list.rpop();
        assert_eq!(val.unwrap().to_string(), "first");
        assert!(list.is_empty());

        // rpop from empty
        let val = list.rpop();
        assert!(val.is_none());
    }

    #[test]
    fn test_list_queue_behavior() {
        let mut list = RedisList::new();

        // rpush + lpop = FIFO queue
        list.rpush(SDS::from_str("a"));
        list.rpush(SDS::from_str("b"));
        list.rpush(SDS::from_str("c"));

        assert_eq!(list.lpop().unwrap().to_string(), "a");
        assert_eq!(list.lpop().unwrap().to_string(), "b");
        assert_eq!(list.lpop().unwrap().to_string(), "c");
    }

    #[test]
    fn test_list_range() {
        let mut list = RedisList::new();

        list.rpush(SDS::from_str("a"));
        list.rpush(SDS::from_str("b"));
        list.rpush(SDS::from_str("c"));
        list.rpush(SDS::from_str("d"));

        // Full range
        let range = list.range(0, -1);
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].to_string(), "a");
        assert_eq!(range[3].to_string(), "d");

        // Subset
        let range = list.range(1, 2);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].to_string(), "b");
        assert_eq!(range[1].to_string(), "c");

        // Negative indices
        let range = list.range(-2, -1);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].to_string(), "c");
        assert_eq!(range[1].to_string(), "d");
    }

    #[test]
    fn test_list_invariants_maintained() {
        let mut list = RedisList::new();

        // Empty list invariants
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);

        // After lpush
        list.lpush(SDS::from_str("x"));
        assert!(!list.is_empty());
        assert_eq!(list.len(), 1);

        // After rpush
        list.rpush(SDS::from_str("y"));
        assert_eq!(list.len(), 2);

        // After lpop
        list.lpop();
        assert_eq!(list.len(), 1);

        // After rpop to empty
        list.rpop();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }
}

#[cfg(test)]
mod set_tests {
    use super::*;

    #[test]
    fn test_set_add_and_contains() {
        let mut set = RedisSet::new();

        // Add new member
        let added = set.add(SDS::from_str("apple"));
        assert!(added);
        assert_eq!(set.len(), 1);
        assert!(set.contains(&SDS::from_str("apple")));

        // Add duplicate
        let added = set.add(SDS::from_str("apple"));
        assert!(!added);
        assert_eq!(set.len(), 1);

        // Add another
        let added = set.add(SDS::from_str("banana"));
        assert!(added);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_set_remove() {
        let mut set = RedisSet::new();

        set.add(SDS::from_str("a"));
        set.add(SDS::from_str("b"));
        assert_eq!(set.len(), 2);

        // Remove existing
        let removed = set.remove(&SDS::from_str("a"));
        assert!(removed);
        assert_eq!(set.len(), 1);
        assert!(!set.contains(&SDS::from_str("a")));

        // Remove non-existing
        let removed = set.remove(&SDS::from_str("nonexistent"));
        assert!(!removed);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_set_members() {
        let mut set = RedisSet::new();

        set.add(SDS::from_str("x"));
        set.add(SDS::from_str("y"));
        set.add(SDS::from_str("z"));

        let members = set.members();
        assert_eq!(members.len(), 3);

        // Convert to strings for easier checking
        let member_strs: Vec<String> = members.iter().map(|m| m.to_string()).collect();
        assert!(member_strs.contains(&"x".to_string()));
        assert!(member_strs.contains(&"y".to_string()));
        assert!(member_strs.contains(&"z".to_string()));
    }

    #[test]
    fn test_set_invariants_maintained() {
        let mut set = RedisSet::new();

        // Empty set
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);

        // After add
        set.add(SDS::from_str("member1"));
        assert!(!set.is_empty());
        assert_eq!(set.len(), 1);

        // After duplicate add (no change)
        set.add(SDS::from_str("member1"));
        assert_eq!(set.len(), 1);

        // After second add
        set.add(SDS::from_str("member2"));
        assert_eq!(set.len(), 2);

        // After remove
        set.remove(&SDS::from_str("member1"));
        assert_eq!(set.len(), 1);
        assert!(!set.contains(&SDS::from_str("member1")));
        assert!(set.contains(&SDS::from_str("member2")));

        // After remove to empty
        set.remove(&SDS::from_str("member2"));
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }
}

#[cfg(test)]
mod sds_tests {
    use super::*;

    #[test]
    fn test_sds_new() {
        let data = vec![104, 101, 108, 108, 111]; // "hello"
        let sds = SDS::new(data.clone());

        assert_eq!(sds.len(), 5);
        assert!(!sds.is_empty());
        assert_eq!(sds.as_bytes(), &data);
        assert_eq!(sds.to_string(), "hello");
    }

    #[test]
    fn test_sds_from_str() {
        let sds = SDS::from_str("world");

        assert_eq!(sds.len(), 5);
        assert!(!sds.is_empty());
        assert_eq!(sds.as_bytes(), b"world");
        assert_eq!(sds.to_string(), "world");
    }

    #[test]
    fn test_sds_empty() {
        let sds = SDS::from_str("");

        assert_eq!(sds.len(), 0);
        assert!(sds.is_empty());
        assert_eq!(sds.as_bytes(), b"");
        assert_eq!(sds.to_string(), "");
    }

    #[test]
    fn test_sds_append() {
        let mut sds = SDS::from_str("hello");
        let other = SDS::from_str(" world");

        sds.append(&other);

        assert_eq!(sds.len(), 11);
        assert_eq!(sds.to_string(), "hello world");
    }

    #[test]
    fn test_sds_append_empty() {
        let mut sds = SDS::from_str("test");
        let empty = SDS::from_str("");

        sds.append(&empty);

        assert_eq!(sds.len(), 4);
        assert_eq!(sds.to_string(), "test");
    }

    #[test]
    fn test_sds_append_to_empty() {
        let mut sds = SDS::from_str("");
        let other = SDS::from_str("data");

        sds.append(&other);

        assert_eq!(sds.len(), 4);
        assert_eq!(sds.to_string(), "data");
    }

    #[test]
    fn test_sds_binary_data() {
        // Test with binary data including null bytes
        let data = vec![0, 1, 2, 0, 255];
        let sds = SDS::new(data.clone());

        assert_eq!(sds.len(), 5);
        assert_eq!(sds.as_bytes(), &data);
    }

    #[test]
    fn test_sds_invariants_maintained() {
        // Test invariants through various operations
        let mut sds = SDS::from_str("");
        assert!(sds.is_empty());
        assert_eq!(sds.len(), 0);

        // Append to empty
        sds.append(&SDS::from_str("a"));
        assert!(!sds.is_empty());
        assert_eq!(sds.len(), 1);

        // Multiple appends
        sds.append(&SDS::from_str("bc"));
        assert_eq!(sds.len(), 3);

        sds.append(&SDS::from_str("def"));
        assert_eq!(sds.len(), 6);
        assert_eq!(sds.to_string(), "abcdef");
    }
}
