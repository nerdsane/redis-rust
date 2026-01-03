use std::collections::{HashMap, HashSet, VecDeque};
use std::cmp::Ordering;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SDS {
    data: Vec<u8>,
}

impl SDS {
    pub fn new(data: Vec<u8>) -> Self {
        SDS { data }
    }

    pub fn from_str(s: &str) -> Self {
        SDS {
            data: s.as_bytes().to_vec(),
        }
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
        self.data.extend_from_slice(&other.data);
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

    pub fn lpush(&mut self, value: SDS) {
        self.items.push_front(value);
    }

    pub fn rpush(&mut self, value: SDS) {
        self.items.push_back(value);
    }

    pub fn lpop(&mut self) -> Option<SDS> {
        self.items.pop_front()
    }

    pub fn rpop(&mut self) -> Option<SDS> {
        self.items.pop_back()
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

    pub fn add(&mut self, member: SDS) -> bool {
        self.members.insert(member.to_string())
    }

    pub fn remove(&mut self, member: &SDS) -> bool {
        self.members.remove(&member.to_string())
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

    pub fn set(&mut self, field: SDS, value: SDS) {
        self.fields.insert(field.to_string(), value);
    }

    pub fn get(&self, field: &SDS) -> Option<&SDS> {
        self.fields.get(&field.to_string())
    }

    pub fn delete(&mut self, field: &SDS) -> bool {
        self.fields.remove(&field.to_string()).is_some()
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

    pub fn add(&mut self, member: SDS, score: f64) -> bool {
        let key = member.to_string();
        let is_new = !self.members.contains_key(&key);
        
        self.members.insert(key.clone(), score);
        self.rebuild_sorted();
        
        is_new
    }

    pub fn remove(&mut self, member: &SDS) -> bool {
        let removed = self.members.remove(&member.to_string()).is_some();
        if removed {
            self.rebuild_sorted();
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
    }
}

impl PartialEq for RedisSortedSet {
    fn eq(&self, other: &Self) -> bool {
        self.members == other.members
    }
}
