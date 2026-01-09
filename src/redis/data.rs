use ahash::{AHashMap, AHashSet};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::VecDeque;

/// Small String Optimization threshold - strings up to this size are stored inline
const SSO_MAX_LEN: usize = 23;

/// Simple Dynamic String with Small String Optimization (SSO)
///
/// Strings ≤23 bytes are stored inline without heap allocation.
/// This matches Redis's approach and significantly reduces allocator pressure
/// since most Redis keys are short (e.g., "user:123", "session:abc").
#[derive(Clone, Debug)]
pub enum SDS {
    /// Inline storage for small strings (no heap allocation)
    Inline { len: u8, data: [u8; SSO_MAX_LEN] },
    /// Heap storage for larger strings
    Heap(Vec<u8>),
}

impl SDS {
    /// VOPR: Verify all invariants hold for this SDS
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        match self {
            SDS::Inline { len, data: _ } => {
                // Invariant 1: Inline len must be <= SSO_MAX_LEN
                debug_assert!(
                    (*len as usize) <= SSO_MAX_LEN,
                    "Invariant violated: inline len {} exceeds SSO_MAX_LEN {}",
                    len,
                    SSO_MAX_LEN
                );
                // Invariant 2: len() must be consistent
                debug_assert_eq!(
                    self.len(),
                    *len as usize,
                    "Invariant violated: len() must equal stored len"
                );
            }
            SDS::Heap(_data) => {
                // Invariant: Heap strings should be > SSO_MAX_LEN
                // (unless created via append that didn't optimize)
                // This is a soft invariant - we don't enforce it for simplicity
            }
        }

        // Invariant 3: is_empty() iff len() == 0
        debug_assert_eq!(
            self.is_empty(),
            self.len() == 0,
            "Invariant violated: is_empty() must equal len() == 0"
        );

        // Invariant 4: as_bytes().len() must equal len()
        debug_assert_eq!(
            self.as_bytes().len(),
            self.len(),
            "Invariant violated: as_bytes().len() must equal len()"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    /// Create SDS from bytes, using inline storage for small strings
    #[inline]
    pub fn new(data: Vec<u8>) -> Self {
        let sds = if data.len() <= SSO_MAX_LEN {
            let mut inline_data = [0u8; SSO_MAX_LEN];
            inline_data[..data.len()].copy_from_slice(&data);
            SDS::Inline {
                len: data.len() as u8,
                data: inline_data,
            }
        } else {
            SDS::Heap(data)
        };

        // TigerStyle: Postcondition - verify construction succeeded
        sds.verify_invariants();
        sds
    }

    /// Create SDS from string slice, using inline storage for small strings
    #[inline]
    pub fn from_str(s: &str) -> Self {
        let bytes = s.as_bytes();
        let sds = if bytes.len() <= SSO_MAX_LEN {
            let mut inline_data = [0u8; SSO_MAX_LEN];
            inline_data[..bytes.len()].copy_from_slice(bytes);
            SDS::Inline {
                len: bytes.len() as u8,
                data: inline_data,
            }
        } else {
            SDS::Heap(bytes.to_vec())
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

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            SDS::Inline { len, .. } => *len as usize,
            SDS::Heap(data) => data.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            SDS::Inline { len, data } => &data[..*len as usize],
            SDS::Heap(data) => data,
        }
    }

    pub fn to_string(&self) -> String {
        String::from_utf8_lossy(self.as_bytes()).to_string()
    }

    pub fn append(&mut self, other: &SDS) {
        // TigerStyle: Preconditions - capture state for postcondition check
        #[cfg(debug_assertions)]
        let pre_len = self.len();
        #[cfg(debug_assertions)]
        let other_len = other.len();

        let new_len = self.len() + other.len();

        // If result fits in inline, stay inline; otherwise convert to heap
        if new_len <= SSO_MAX_LEN {
            // Can stay inline
            match self {
                SDS::Inline { len, data } => {
                    let current_len = *len as usize;
                    data[current_len..current_len + other.len()].copy_from_slice(other.as_bytes());
                    *len = new_len as u8;
                }
                SDS::Heap(data) => {
                    // Heap but small enough - just extend
                    data.extend_from_slice(other.as_bytes());
                }
            }
        } else {
            // Need heap storage
            let mut new_data = Vec::with_capacity(new_len);
            new_data.extend_from_slice(self.as_bytes());
            new_data.extend_from_slice(other.as_bytes());
            *self = SDS::Heap(new_data);
        }

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.len(),
                pre_len + other_len,
                "Postcondition violated: len must equal pre_len + other_len after append"
            );
        }

        self.verify_invariants();
    }
}

impl PartialEq for SDS {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for SDS {}

impl std::hash::Hash for SDS {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

// Custom serialization to store as bytes (backwards compatible)
impl Serialize for SDS {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(self.as_bytes())
    }
}

impl<'de> Deserialize<'de> for SDS {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        Ok(SDS::new(bytes))
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
                debug_assert!(
                    result.is_none(),
                    "Postcondition violated: pop from empty must return None"
                );
                debug_assert_eq!(
                    self.items.len(),
                    0,
                    "Postcondition violated: empty list must stay empty"
                );
            } else {
                debug_assert!(
                    result.is_some(),
                    "Postcondition violated: pop from non-empty must return Some"
                );
                debug_assert_eq!(
                    self.items.len(),
                    pre_len - 1,
                    "Postcondition violated: len must decrease by 1"
                );
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
                debug_assert!(
                    result.is_none(),
                    "Postcondition violated: pop from empty must return None"
                );
                debug_assert_eq!(
                    self.items.len(),
                    0,
                    "Postcondition violated: empty list must stay empty"
                );
            } else {
                debug_assert!(
                    result.is_some(),
                    "Postcondition violated: pop from non-empty must return Some"
                );
                debug_assert_eq!(
                    self.items.len(),
                    pre_len - 1,
                    "Postcondition violated: len must decrease by 1"
                );
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
        let start = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let stop = if stop < 0 {
            (len + stop).max(-1)
        } else {
            stop.min(len - 1)
        };

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

    /// LINDEX - get element at index
    pub fn get(&self, index: isize) -> Option<&SDS> {
        let len = self.items.len() as isize;
        let idx = if index < 0 { len + index } else { index };

        if idx < 0 || idx >= len {
            return None;
        }

        self.items.get(idx as usize)
    }

    /// LSET - set element at index (for LSET command)
    pub fn set(&mut self, index: isize, value: SDS) -> Result<(), String> {
        let len = self.items.len() as isize;

        // TigerStyle: Preconditions
        debug_assert!(len > 0, "Precondition: list must not be empty for LSET");

        let idx = if index < 0 { len + index } else { index };

        if idx < 0 || idx >= len {
            return Err("ERR index out of range".to_string());
        }

        #[cfg(debug_assertions)]
        let pre_len = self.items.len();

        self.items[idx as usize] = value.clone();

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                pre_len,
                "Postcondition violated: length must not change after LSET"
            );
            debug_assert_eq!(
                self.items[idx as usize].to_string(),
                value.to_string(),
                "Postcondition violated: value must be set at index"
            );
        }

        self.verify_invariants();
        Ok(())
    }

    /// LTRIM - trim list to specified range
    pub fn trim(&mut self, start: isize, stop: isize) {
        let len = self.items.len() as isize;
        if len == 0 {
            return;
        }

        // Normalize indices
        let s = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let e = if stop < 0 {
            (len + stop).max(-1)
        } else {
            stop.min(len - 1)
        };

        if s > e || s >= len {
            self.items.clear();
            self.verify_invariants();
            return;
        }

        // Keep only elements in range
        let new_items: VecDeque<SDS> = self
            .items
            .iter()
            .skip(s as usize)
            .take((e - s + 1) as usize)
            .cloned()
            .collect();

        #[cfg(debug_assertions)]
        let expected_len = (e - s + 1) as usize;

        self.items = new_items;

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                expected_len,
                "Postcondition violated: length must equal trimmed range size"
            );
        }

        self.verify_invariants();
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RedisSet {
    members: AHashSet<String>,
}

impl RedisSet {
    pub fn new() -> Self {
        RedisSet {
            members: AHashSet::new(),
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
        self.members.iter().map(|s| SDS::from_str(s)).collect()
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
    fields: AHashMap<String, SDS>,
}

impl RedisHash {
    pub fn new() -> Self {
        RedisHash {
            fields: AHashMap::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this hash
    /// Called in debug builds after every mutation
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual AHashMap size
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

    /// Iterate over field-value pairs (for HSCAN)
    pub fn iter(&self) -> impl Iterator<Item = (&String, &SDS)> {
        self.fields.iter()
    }
}

#[derive(Clone, Debug)]
pub struct RedisSortedSet {
    members: AHashMap<String, f64>,
    sorted_members: Vec<(String, f64)>,
}

impl RedisSortedSet {
    pub fn new() -> Self {
        RedisSortedSet {
            members: AHashMap::new(),
            sorted_members: Vec::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this sorted set
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual AHashMap size
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

        // Invariant 5: Every member in AHashMap must be in sorted_members with matching score
        for (member, score) in &self.members {
            let found = self.sorted_members.iter().find(|(m, _)| m == member);
            debug_assert!(
                found.is_some(),
                "Invariant violated: member '{}' in AHashMap but not in sorted_members",
                member
            );
            debug_assert_eq!(
                found.map(|(_, s)| *s),
                Some(*score),
                "Invariant violated: score mismatch for member '{}'",
                member
            );
        }

        // Invariant 6: Every member in sorted_members must be in AHashMap
        for (member, score) in &self.sorted_members {
            debug_assert!(
                self.members.contains_key(member),
                "Invariant violated: member '{}' in sorted_members but not in AHashMap",
                member
            );
            debug_assert_eq!(
                self.members.get(member),
                Some(score),
                "Invariant violated: score mismatch for member '{}' in AHashMap",
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

        // Check if member already exists
        if let Some(&old_score) = self.members.get(&key) {
            if (old_score - score).abs() < f64::EPSILON {
                // Score unchanged, nothing to do
                return false;
            }
            // Update score - remove old position, insert at new position
            self.members.insert(key.clone(), score);
            // Remove from sorted_members (linear scan, but updates are less common)
            if let Some(pos) = self.sorted_members.iter().position(|(m, _)| m == &key) {
                self.sorted_members.remove(pos);
            }
            // Binary search insert at new position
            let insert_pos = self.sorted_members
                .binary_search_by(|probe| {
                    probe.1.partial_cmp(&score)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| probe.0.cmp(&key))
                })
                .unwrap_or_else(|pos| pos);
            self.sorted_members.insert(insert_pos, (key.clone(), score));

            #[cfg(debug_assertions)]
            self.verify_invariants();
            return false; // Not a new member
        }

        // New member - insert into both structures
        self.members.insert(key.clone(), score);

        // Binary search to find insertion point - O(log n)
        let insert_pos = self.sorted_members
            .binary_search_by(|probe| {
                probe.1.partial_cmp(&score)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| probe.0.cmp(&key))
            })
            .unwrap_or_else(|pos| pos);
        self.sorted_members.insert(insert_pos, (key.clone(), score));

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
            let expected_len = pre_len + 1;
            debug_assert_eq!(
                self.members.len(),
                expected_len,
                "Postcondition violated: len must be correct after add"
            );
            self.verify_invariants();
        }

        true
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
            // Remove from sorted_members - linear scan O(n) but removes are less common
            if let Some(pos) = self.sorted_members.iter().position(|(m, _)| m == &key) {
                self.sorted_members.remove(pos);
            }
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
            self.verify_invariants();
        }

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
        let start = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let stop = if stop < 0 {
            (len + stop).max(-1)
        } else {
            stop.min(len - 1)
        };

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
        let start = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let stop = if stop < 0 {
            (len + stop).max(-1)
        } else {
            stop.min(len - 1)
        };

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

    /// Parse score bound (handles -inf, +inf, exclusive with parenthesis)
    fn parse_score_bound(s: &str, _is_min: bool) -> Result<(f64, bool), String> {
        let s = s.trim();
        if s == "-inf" {
            return Ok((f64::NEG_INFINITY, false));
        }
        if s == "+inf" || s == "inf" {
            return Ok((f64::INFINITY, false));
        }

        let (exclusive, num_str) = if s.starts_with('(') {
            (true, &s[1..])
        } else {
            (false, s)
        };

        let score = num_str
            .parse::<f64>()
            .map_err(|_| "ERR min or max is not a float".to_string())?;

        Ok((score, exclusive))
    }

    /// ZCOUNT - count elements in score range
    pub fn count_in_range(&self, min: &str, max: &str) -> Result<usize, String> {
        let (min_score, min_exclusive) = Self::parse_score_bound(min, true)?;
        let (max_score, max_exclusive) = Self::parse_score_bound(max, false)?;

        let count = self
            .sorted_members
            .iter()
            .filter(|(_, score)| {
                let above_min = if min_exclusive {
                    *score > min_score
                } else {
                    *score >= min_score
                };
                let below_max = if max_exclusive {
                    *score < max_score
                } else {
                    *score <= max_score
                };
                above_min && below_max
            })
            .count();

        Ok(count)
    }

    /// ZRANGEBYSCORE - get elements by score range
    pub fn range_by_score(
        &self,
        min: &str,
        max: &str,
        with_scores: bool,
        limit: Option<(isize, usize)>,
    ) -> Result<Vec<(String, Option<f64>)>, String> {
        let (min_score, min_exclusive) = Self::parse_score_bound(min, true)?;
        let (max_score, max_exclusive) = Self::parse_score_bound(max, false)?;

        let mut results: Vec<_> = self
            .sorted_members
            .iter()
            .filter(|(_, score)| {
                let above_min = if min_exclusive {
                    *score > min_score
                } else {
                    *score >= min_score
                };
                let below_max = if max_exclusive {
                    *score < max_score
                } else {
                    *score <= max_score
                };
                above_min && below_max
            })
            .map(|(member, score)| {
                (
                    member.clone(),
                    if with_scores { Some(*score) } else { None },
                )
            })
            .collect();

        // Apply LIMIT
        if let Some((offset, count)) = limit {
            let start = offset.max(0) as usize;
            results = results.into_iter().skip(start).take(count).collect();
        }

        Ok(results)
    }

    /// Iterate over member-score pairs (for ZSCAN)
    pub fn iter(&self) -> impl Iterator<Item = (&String, &f64)> {
        self.sorted_members.iter().map(|(m, s)| (m, s))
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
        assert_eq!(
            hash.get(&SDS::from_str("counter")).unwrap().to_string(),
            "10"
        );

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

    // === Small String Optimization (SSO) Tests ===

    #[test]
    fn test_sso_small_string_is_inline() {
        // Strings <= 23 bytes should use inline storage
        let sds = SDS::from_str("short");
        assert_eq!(sds.len(), 5);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 5),
            SDS::Heap(_) => panic!("Expected inline storage for short string"),
        }
    }

    #[test]
    fn test_sso_max_inline_string() {
        // 23 bytes exactly should still be inline
        let s = "12345678901234567890123"; // 23 chars
        assert_eq!(s.len(), 23);
        let sds = SDS::from_str(s);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 23),
            SDS::Heap(_) => panic!("Expected inline storage for 23-byte string"),
        }
        assert_eq!(sds.to_string(), s);
    }

    #[test]
    fn test_sso_large_string_is_heap() {
        // 24+ bytes should use heap storage
        let s = "123456789012345678901234"; // 24 chars
        assert_eq!(s.len(), 24);
        let sds = SDS::from_str(s);
        match &sds {
            SDS::Inline { .. } => panic!("Expected heap storage for 24-byte string"),
            SDS::Heap(data) => assert_eq!(data.len(), 24),
        }
        assert_eq!(sds.to_string(), s);
    }

    #[test]
    fn test_sso_append_stays_inline() {
        // Append that keeps total <= 23 bytes should stay inline
        let mut sds = SDS::from_str("hello"); // 5 bytes
        sds.append(&SDS::from_str(" world")); // +6 = 11 bytes

        assert_eq!(sds.len(), 11);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 11),
            SDS::Heap(_) => panic!("Expected inline storage after append"),
        }
        assert_eq!(sds.to_string(), "hello world");
    }

    #[test]
    fn test_sso_append_transitions_to_heap() {
        // Append that exceeds 23 bytes should transition to heap
        let mut sds = SDS::from_str("12345678901234567890"); // 20 bytes
        sds.append(&SDS::from_str("12345")); // +5 = 25 bytes

        assert_eq!(sds.len(), 25);
        match &sds {
            SDS::Inline { .. } => panic!("Expected heap storage after append exceeds SSO"),
            SDS::Heap(data) => assert_eq!(data.len(), 25),
        }
        assert_eq!(sds.to_string(), "1234567890123456789012345");
    }

    #[test]
    fn test_sso_empty_string() {
        let sds = SDS::from_str("");
        assert!(sds.is_empty());
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 0),
            SDS::Heap(_) => panic!("Expected inline storage for empty string"),
        }
    }

    #[test]
    fn test_sso_new_from_vec() {
        // Small vec should be inline
        let sds = SDS::new(vec![1, 2, 3, 4, 5]);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 5),
            SDS::Heap(_) => panic!("Expected inline storage for small vec"),
        }

        // Large vec should be heap
        let sds = SDS::new(vec![0u8; 30]);
        match &sds {
            SDS::Inline { .. } => panic!("Expected heap storage for large vec"),
            SDS::Heap(data) => assert_eq!(data.len(), 30),
        }
    }

    #[test]
    fn test_sso_equality() {
        // Inline == Inline
        let a = SDS::from_str("hello");
        let b = SDS::from_str("hello");
        assert_eq!(a, b);

        // Different inline strings
        let c = SDS::from_str("world");
        assert_ne!(a, c);

        // Heap == Heap
        let d = SDS::from_str("this is a very long string that exceeds the SSO limit");
        let e = SDS::from_str("this is a very long string that exceeds the SSO limit");
        assert_eq!(d, e);
    }

    #[test]
    fn test_sso_typical_redis_keys() {
        // Most Redis keys are short - test typical patterns
        let keys = [
            "user:123",           // 8 bytes - inline
            "session:abc123",     // 14 bytes - inline
            "cache:homepage",     // 14 bytes - inline
            "counter",            // 7 bytes - inline
            "rate_limit:user:42", // 18 bytes - inline
        ];

        for key in &keys {
            let sds = SDS::from_str(key);
            assert!(key.len() <= 23, "Test key should be <= 23 bytes");
            match &sds {
                SDS::Inline { .. } => {} // Expected
                SDS::Heap(_) => panic!("Key '{}' should use inline storage", key),
            }
        }
    }
}
