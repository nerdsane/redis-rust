//! Hash command implementations for CommandExecutor.
//!
//! Handles: HSET, HGET, HDEL, HGETALL, HKEYS, HVALS, HLEN, HEXISTS, HINCRBY

use super::CommandExecutor;
use crate::redis::data::{RedisHash, Value, SDS};
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_hset(&mut self, key: &str, pairs: &[(SDS, SDS)]) -> RespValue {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }
        let hash = self
            .data
            .entry(key.to_string())
            .or_insert_with(|| Value::Hash(RedisHash::new()));
        self.access_times.insert(key.to_string(), self.current_time);
        match hash {
            Value::Hash(h) => {
                let mut new_fields = 0i64;
                for (field, value) in pairs {
                    if !h.exists(field) {
                        new_fields += 1;
                    }
                    h.set(field.clone(), value.clone());
                }
                RespValue::Integer(new_fields)
            }
            _ => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
        }
    }

    pub(super) fn execute_hget(&mut self, key: &str, field: &SDS) -> RespValue {
        match self.get_value(key) {
            Some(Value::Hash(h)) => match h.get(field) {
                Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                None => RespValue::BulkString(None),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_hdel(&mut self, key: &str, fields: &[SDS]) -> RespValue {
        let result = match self.get_value_mut(key) {
            Some(Value::Hash(h)) => {
                // TigerStyle: Capture pre-state for postcondition
                #[cfg(debug_assertions)]
                let pre_len = h.len();

                let mut deleted = 0i64;
                for field in fields {
                    if h.delete(field) {
                        deleted += 1;
                    }
                }

                // TigerStyle: Postconditions
                #[cfg(debug_assertions)]
                {
                    debug_assert!(
                        deleted >= 0,
                        "Invariant violated: deleted count must be non-negative"
                    );
                    debug_assert!(
                        deleted <= fields.len() as i64,
                        "Invariant violated: can't delete more than requested"
                    );
                    debug_assert_eq!(
                        h.len(),
                        pre_len - deleted as usize,
                        "Invariant violated: len must decrease by deleted count"
                    );
                }

                RespValue::Integer(deleted)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        };
        // Redis auto-deletes empty hashes
        if matches!(self.data.get(key), Some(Value::Hash(h)) if h.is_empty()) {
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);
        }
        result
    }

    pub(super) fn execute_hgetall(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::Hash(h)) => {
                // Pre-allocate capacity: each field has key and value
                let mut elements = Vec::with_capacity(h.len() * 2);
                for (k, v) in h.get_all() {
                    elements.push(RespValue::BulkString(Some(k.as_bytes().to_vec())));
                    elements.push(RespValue::BulkString(Some(v.as_bytes().to_vec())));
                }
                RespValue::Array(Some(elements))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(Vec::new())),
        }
    }

    pub(super) fn execute_hkeys(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::Hash(h)) => {
                let hash_keys = h.keys();
                // TigerStyle: Postcondition - keys count must equal len
                debug_assert_eq!(
                    hash_keys.len(),
                    h.len(),
                    "Invariant violated: HKEYS count must equal HLEN"
                );

                let keys: Vec<RespValue> = hash_keys
                    .iter()
                    .map(|k| RespValue::BulkString(Some(k.as_bytes().to_vec())))
                    .collect();
                RespValue::Array(Some(keys))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(Vec::new())),
        }
    }

    pub(super) fn execute_hvals(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::Hash(h)) => {
                let hash_vals = h.values();
                // TigerStyle: Postcondition - values count must equal len
                debug_assert_eq!(
                    hash_vals.len(),
                    h.len(),
                    "Invariant violated: HVALS count must equal HLEN"
                );

                let vals: Vec<RespValue> = hash_vals
                    .iter()
                    .map(|v| RespValue::BulkString(Some(v.as_bytes().to_vec())))
                    .collect();
                RespValue::Array(Some(vals))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(Vec::new())),
        }
    }

    pub(super) fn execute_hlen(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::Hash(h)) => {
                let len = h.len() as i64;
                // TigerStyle: Postcondition
                debug_assert!(
                    len >= 0,
                    "Invariant violated: HLEN must return non-negative"
                );
                RespValue::Integer(len)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_hexists(&mut self, key: &str, field: &SDS) -> RespValue {
        match self.get_value(key) {
            Some(Value::Hash(h)) => {
                let exists = h.exists(field);
                // TigerStyle: Postcondition - result must be 0 or 1
                let result = if exists { 1i64 } else { 0i64 };
                debug_assert!(
                    result == 0 || result == 1,
                    "Invariant violated: HEXISTS must return 0 or 1"
                );
                RespValue::Integer(result)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_hincrby(&mut self, key: &str, field: &SDS, increment: i64) -> RespValue {
        // Handle expiration first
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);
        }

        // Check if key exists and is wrong type before inserting
        if let Some(existing) = self.data.get(key) {
            if !matches!(existing, Value::Hash(_)) {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                );
            }
        }

        let hash = self
            .data
            .entry(key.to_string())
            .or_insert_with(|| Value::Hash(RedisHash::new()));
        self.access_times.insert(key.to_string(), self.current_time);

        match hash {
            Value::Hash(h) => {
                // Get current value, parse as i64, return error if not an integer
                let current: i64 = match h.get(field) {
                    Some(v) => {
                        let s = v.to_string();
                        match s.parse::<i64>() {
                            Ok(n) => n,
                            Err(_) => return RespValue::err("ERR hash value is not an integer"),
                        }
                    }
                    None => 0,
                };

                // TigerStyle: Use checked arithmetic to detect overflow
                let new_value = match current.checked_add(increment) {
                    Some(v) => v,
                    None => return RespValue::err("ERR increment or decrement would overflow"),
                };

                h.set(field.clone(), SDS::from_str(&new_value.to_string()));

                // TigerStyle: Assert invariants after mutation
                debug_assert!(
                    h.get(field).is_some(),
                    "Invariant violated: field must exist after HINCRBY"
                );
                debug_assert_eq!(
                    h.get(field)
                        .map(|v| v.to_string().parse::<i64>().ok())
                        .flatten(),
                    Some(new_value),
                    "Invariant violated: field value must equal computed value after HINCRBY"
                );

                RespValue::Integer(new_value)
            }
            _ => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
        }
    }
}
