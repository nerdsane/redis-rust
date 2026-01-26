//! List command implementations for CommandExecutor.
//!
//! Handles: LPUSH, RPUSH, LPOP, RPOP, LLEN, LINDEX, LRANGE, LSET, LTRIM,
//! RPOPLPUSH, LMOVE
//!
//! # TigerStyle Invariants
//!
//! - LPUSH/RPUSH: result length = pre_length + pushed_count
//! - LPOP/RPOP: result length = pre_length - 1 (if non-empty)
//! - LLEN: always returns non-negative
//! - LINDEX: actual_index must be < list.len() when accessing
//! - LRANGE: result.len() <= end - start + 1

use super::CommandExecutor;
use crate::redis::data::{RedisList, Value, SDS};
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_lpush(&mut self, key: &str, values: &[SDS]) -> RespValue {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }
        let list = self
            .data
            .entry(key.to_string())
            .or_insert_with(|| Value::List(RedisList::new()));
        self.access_times.insert(key.to_string(), self.current_time);
        match list {
            Value::List(l) => {
                #[cfg(debug_assertions)]
                let pre_len = l.len();

                for value in values {
                    l.lpush(value.clone());
                }
                let new_len = l.len() as i64;

                // TigerStyle: Postcondition - length increased by pushed count
                debug_assert_eq!(
                    l.len(),
                    pre_len + values.len(),
                    "Postcondition violated: LPUSH length must increase by pushed count"
                );

                RespValue::Integer(new_len)
            }
            _ => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
        }
    }

    pub(super) fn execute_rpush(&mut self, key: &str, values: &[SDS]) -> RespValue {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }
        let list = self
            .data
            .entry(key.to_string())
            .or_insert_with(|| Value::List(RedisList::new()));
        self.access_times.insert(key.to_string(), self.current_time);
        match list {
            Value::List(l) => {
                #[cfg(debug_assertions)]
                let pre_len = l.len();

                for value in values {
                    l.rpush(value.clone());
                }
                let new_len = l.len() as i64;

                // TigerStyle: Postcondition - length increased by pushed count
                debug_assert_eq!(
                    l.len(),
                    pre_len + values.len(),
                    "Postcondition violated: RPUSH length must increase by pushed count"
                );

                RespValue::Integer(new_len)
            }
            _ => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
        }
    }

    pub(super) fn execute_lpop(&mut self, key: &str) -> RespValue {
        match self.get_value_mut(key) {
            Some(Value::List(l)) => match l.lpop() {
                Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                None => RespValue::BulkString(None),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_rpop(&mut self, key: &str) -> RespValue {
        match self.get_value_mut(key) {
            Some(Value::List(l)) => match l.rpop() {
                Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                None => RespValue::BulkString(None),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_llen(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::List(l)) => {
                let len = l.len() as i64;
                // TigerStyle: Postcondition - length must be non-negative
                debug_assert!(
                    len >= 0,
                    "Invariant violated: LLEN must return non-negative"
                );
                RespValue::Integer(len)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_lindex(&mut self, key: &str, index: isize) -> RespValue {
        match self.get_value(key) {
            Some(Value::List(l)) => {
                let len = l.len() as isize;
                // TigerStyle: Handle negative indices (Redis convention)
                let actual_index = if index < 0 {
                    let normalized = len + index;
                    if normalized < 0 {
                        return RespValue::BulkString(None);
                    }
                    normalized as usize
                } else if index >= len {
                    return RespValue::BulkString(None);
                } else {
                    index as usize
                };

                // TigerStyle: Precondition verified
                debug_assert!(
                    actual_index < l.len(),
                    "Invariant violated: index must be in bounds"
                );

                let range = l.range(actual_index as isize, actual_index as isize);
                if let Some(item) = range.first() {
                    RespValue::BulkString(Some(item.as_bytes().to_vec()))
                } else {
                    RespValue::BulkString(None)
                }
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_lrange(&mut self, key: &str, start: isize, stop: isize) -> RespValue {
        match self.get_value(key) {
            Some(Value::List(l)) => {
                let range = l.range(start, stop);
                let elements: Vec<RespValue> = range
                    .iter()
                    .map(|s| RespValue::BulkString(Some(s.as_bytes().to_vec())))
                    .collect();
                RespValue::Array(Some(elements))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(Vec::new())),
        }
    }

    pub(super) fn execute_lset(&mut self, key: &str, index: isize, value: &SDS) -> RespValue {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }
        match self.data.get_mut(key) {
            Some(Value::List(list)) => match list.set(index, value.clone()) {
                Ok(()) => {
                    self.access_times.insert(key.to_string(), self.current_time);
                    RespValue::simple("OK")
                }
                Err(e) => RespValue::err(e),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::err("ERR no such key"),
        }
    }

    pub(super) fn execute_ltrim(&mut self, key: &str, start: isize, stop: isize) -> RespValue {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }
        match self.data.get_mut(key) {
            Some(Value::List(list)) => {
                list.trim(start, stop);
                self.access_times.insert(key.to_string(), self.current_time);
                // Remove key if list becomes empty
                if list.is_empty() {
                    self.data.remove(key);
                    self.access_times.remove(key);
                    self.expirations.remove(key);
                }
                RespValue::simple("OK")
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::simple("OK"), // No-op if key doesn't exist
        }
    }

    pub(super) fn execute_rpoplpush(&mut self, source: &str, dest: &str) -> RespValue {
        if self.is_expired(source) {
            self.data.remove(source);
            self.expirations.remove(source);
        }
        // Pop from source
        let popped = match self.data.get_mut(source) {
            Some(Value::List(list)) => list.rpop(),
            Some(_) => {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                )
            }
            None => None,
        };

        match popped {
            Some(value) => {
                // Remove source if now empty
                if let Some(Value::List(list)) = self.data.get(source) {
                    if list.is_empty() {
                        self.data.remove(source);
                        self.access_times.remove(source);
                        self.expirations.remove(source);
                    }
                }

                // Push to dest
                if self.is_expired(dest) {
                    self.data.remove(dest);
                    self.expirations.remove(dest);
                }
                let dest_list = self
                    .data
                    .entry(dest.to_string())
                    .or_insert_with(|| Value::List(RedisList::new()));
                match dest_list {
                    Value::List(list) => {
                        list.lpush(value.clone());
                        self.access_times
                            .insert(dest.to_string(), self.current_time);
                        RespValue::BulkString(Some(value.as_bytes().to_vec()))
                    }
                    _ => RespValue::err(
                        "WRONGTYPE Operation against a key holding the wrong kind of value",
                    ),
                }
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_lmove(
        &mut self,
        source: &str,
        dest: &str,
        wherefrom: &str,
        whereto: &str,
    ) -> RespValue {
        if self.is_expired(source) {
            self.data.remove(source);
            self.expirations.remove(source);
        }
        // Pop from source
        let popped = match self.data.get_mut(source) {
            Some(Value::List(list)) => {
                if wherefrom == "LEFT" {
                    list.lpop()
                } else {
                    list.rpop()
                }
            }
            Some(_) => {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                )
            }
            None => None,
        };

        match popped {
            Some(value) => {
                // Remove source if now empty
                if let Some(Value::List(list)) = self.data.get(source) {
                    if list.is_empty() {
                        self.data.remove(source);
                        self.access_times.remove(source);
                        self.expirations.remove(source);
                    }
                }

                // Push to dest
                if self.is_expired(dest) {
                    self.data.remove(dest);
                    self.expirations.remove(dest);
                }
                let dest_list = self
                    .data
                    .entry(dest.to_string())
                    .or_insert_with(|| Value::List(RedisList::new()));
                match dest_list {
                    Value::List(list) => {
                        if whereto == "LEFT" {
                            list.lpush(value.clone());
                        } else {
                            list.rpush(value.clone());
                        }
                        self.access_times
                            .insert(dest.to_string(), self.current_time);
                        RespValue::BulkString(Some(value.as_bytes().to_vec()))
                    }
                    _ => RespValue::err(
                        "WRONGTYPE Operation against a key holding the wrong kind of value",
                    ),
                }
            }
            None => RespValue::BulkString(None),
        }
    }
}
