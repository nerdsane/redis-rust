//! String command implementations for CommandExecutor.
//!
//! Handles: GET, SET, APPEND, GETSET, STRLEN, MGET, MSET, BATCHSET, BATCHGET,
//! INCR, DECR, INCRBY, DECRBY

use super::CommandExecutor;
use crate::redis::data::{Value, SDS};
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_get(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_set(
        &mut self,
        key: &str,
        value: &SDS,
        ex: &Option<i64>,
        px: &Option<i64>,
        nx: &bool,
        xx: &bool,
        get: &bool,
    ) -> RespValue {
        // Validate expiration values
        if let Some(seconds) = ex {
            if *seconds <= 0 {
                return RespValue::err("ERR invalid expire time in 'set' command");
            }
        }
        if let Some(millis) = px {
            if *millis <= 0 {
                return RespValue::err("ERR invalid expire time in 'set' command");
            }
        }

        // Get old value if GET option specified
        let old_value = if *get {
            match self.get_value(key) {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            }
        } else {
            None
        };

        // Check key existence for NX/XX
        let key_exists = !self.is_expired(key) && self.data.contains_key(key);

        // NX: only set if key doesn't exist
        if *nx && key_exists {
            return match old_value {
                Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                None => RespValue::BulkString(None),
            };
        }

        // XX: only set if key exists
        if *xx && !key_exists {
            return RespValue::BulkString(None);
        }

        // Set the value
        self.data
            .insert(key.to_string(), Value::String(value.clone()));
        self.access_times.insert(key.to_string(), self.current_time);

        // Handle expiration
        if let Some(seconds) = ex {
            let expiration =
                self.current_time + crate::simulator::Duration::from_secs(*seconds as u64);
            self.expirations.insert(key.to_string(), expiration);
        } else if let Some(millis) = px {
            let expiration =
                self.current_time + crate::simulator::Duration::from_millis(*millis as u64);
            self.expirations.insert(key.to_string(), expiration);
        } else {
            self.expirations.remove(key);
        }

        // Return appropriate response
        if *get {
            match old_value {
                Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                None => RespValue::BulkString(None),
            }
        } else {
            RespValue::simple("OK")
        }
    }

    pub(super) fn execute_append(&mut self, key: &str, value: &SDS) -> RespValue {
        match self.get_value_mut(key) {
            Some(Value::String(s)) => {
                s.append(value);
                RespValue::Integer(s.len() as i64)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => {
                let len = value.len();
                self.data
                    .insert(key.to_string(), Value::String(value.clone()));
                self.access_times.insert(key.to_string(), self.current_time);
                RespValue::Integer(len as i64)
            }
        }
    }

    pub(super) fn execute_getset(&mut self, key: &str, value: &SDS) -> RespValue {
        let old_value = match self.get_value(key) {
            Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
            Some(_) => {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                )
            }
            None => RespValue::BulkString(None),
        };
        self.data
            .insert(key.to_string(), Value::String(value.clone()));
        self.access_times.insert(key.to_string(), self.current_time);
        old_value
    }

    pub(super) fn execute_strlen(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::String(s)) => {
                let len = s.len() as i64;
                // TigerStyle: Postcondition - length must be non-negative
                debug_assert!(
                    len >= 0,
                    "Invariant violated: STRLEN must return non-negative"
                );
                RespValue::Integer(len)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_mget(&mut self, keys: &[String]) -> RespValue {
        let values: Vec<RespValue> = keys
            .iter()
            .map(|k| match self.get_value(k) {
                Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                _ => RespValue::BulkString(None),
            })
            .collect();
        RespValue::Array(Some(values))
    }

    pub(super) fn execute_mset(&mut self, pairs: &[(String, SDS)]) -> RespValue {
        for (key, value) in pairs {
            self.data.insert(key.clone(), Value::String(value.clone()));
            self.access_times.insert(key.clone(), self.current_time);
        }
        RespValue::simple("OK")
    }

    pub(super) fn execute_batch_set(&mut self, pairs: &[(String, SDS)]) -> RespValue {
        // Optimized batch set - all keys are guaranteed to be on this shard
        for (key, value) in pairs {
            self.data.insert(key.clone(), Value::String(value.clone()));
            self.access_times.insert(key.clone(), self.current_time);
        }
        RespValue::simple("OK")
    }

    pub(super) fn execute_batch_get(&mut self, keys: &[String]) -> RespValue {
        // Optimized batch get - all keys are guaranteed to be on this shard
        let mut results = Vec::with_capacity(keys.len());
        for key in keys {
            let value = self.get_value(key);
            results.push(match value {
                Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                Some(_) => RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                ),
                None => RespValue::BulkString(None),
            });
        }
        RespValue::Array(Some(results))
    }

    pub(super) fn incr_by_impl(&mut self, key: &str, increment: i64) -> RespValue {
        // TigerStyle: Precondition
        debug_assert!(!key.is_empty(), "Precondition: key must not be empty");

        let response = match self.get_value_mut(key) {
            Some(Value::String(s)) => {
                let current = match s.to_string().parse::<i64>() {
                    Ok(n) => n,
                    Err(_) => return RespValue::err("ERR value is not an integer or out of range"),
                };
                let new_value = match current.checked_add(increment) {
                    Some(n) => n,
                    None => return RespValue::err("ERR increment or decrement would overflow"),
                };
                let new_str = SDS::from_str(&new_value.to_string());
                self.data.insert(key.to_string(), Value::String(new_str));
                RespValue::Integer(new_value)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => {
                self.data.insert(
                    key.to_string(),
                    Value::String(SDS::from_str(&increment.to_string())),
                );
                self.access_times.insert(key.to_string(), self.current_time);
                RespValue::Integer(increment)
            }
        };

        // TigerStyle: Postcondition - verify stored value matches returned value
        #[cfg(debug_assertions)]
        if let RespValue::Integer(result) = &response {
            if let Some(Value::String(s)) = self.data.get(key) {
                debug_assert_eq!(
                    s.to_string().parse::<i64>().ok(),
                    Some(*result),
                    "Postcondition: stored value must equal returned value"
                );
            }
        }

        response
    }
}
