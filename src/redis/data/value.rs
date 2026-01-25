//! Redis Value type enum

use super::{RedisHash, RedisList, RedisSet, RedisSortedSet, SDS};

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
