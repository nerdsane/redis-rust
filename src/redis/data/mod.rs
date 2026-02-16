//! Redis data structures module
//!
//! This module provides core Redis data types:
//! - `SDS`: Simple Dynamic String with Small String Optimization
//! - `Value`: Union type for all Redis value types
//! - `RedisList`: Doubly-ended queue (LPUSH/RPUSH/LPOP/RPOP)
//! - `RedisSet`: Unordered set of unique strings
//! - `RedisHash`: Hash table of field-value pairs
//! - `RedisSortedSet`: Sorted set with scores (using skip list)
//! - `SkipList`: Probabilistic data structure for sorted sets

mod hash;
mod list;
mod sds;
mod set;
mod skiplist;
mod sorted_set;
mod value;

// Re-export all public types
pub use hash::RedisHash;
pub use list::RedisList;
pub use sds::SDS;
pub use set::RedisSet;
pub use skiplist::SkipList;
pub use sorted_set::RedisSortedSet;
pub use value::Value;
