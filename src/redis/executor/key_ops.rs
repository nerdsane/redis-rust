//! Key command implementations for CommandExecutor.
//!
//! Handles: DEL, EXISTS, TYPE, KEYS, FLUSHDB, FLUSHALL, EXPIRE, EXPIREAT,
//! PEXPIREAT, TTL, PTTL, PERSIST

use super::CommandExecutor;
use crate::redis::data::Value;
use crate::redis::resp::RespValue;
use crate::simulator::VirtualTime;

impl CommandExecutor {
    pub(super) fn execute_del(&mut self, keys: &[String]) -> RespValue {
        let mut count = 0;
        for key in keys {
            if self.data.remove(key).is_some() {
                count += 1;
            }
            self.expirations.remove(key);
            self.access_times.remove(key);
        }
        RespValue::Integer(count)
    }

    pub(super) fn execute_exists(&self, keys: &[String]) -> RespValue {
        let count = keys
            .iter()
            .filter(|k| !self.is_expired(k) && self.data.contains_key(*k))
            .count();
        RespValue::Integer(count as i64)
    }

    pub(super) fn execute_typeof(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::String(_)) => RespValue::simple("string"),
            Some(Value::List(_)) => RespValue::simple("list"),
            Some(Value::Set(_)) => RespValue::simple("set"),
            Some(Value::Hash(_)) => RespValue::simple("hash"),
            Some(Value::SortedSet(_)) => RespValue::simple("zset"),
            Some(Value::Null) => RespValue::simple("none"),
            None => RespValue::simple("none"),
        }
    }

    pub(super) fn execute_keys(&self, pattern: &str) -> RespValue {
        let keys: Vec<RespValue> = self
            .data
            .keys()
            .filter(|k| !self.is_expired(k) && self.matches_glob_pattern(k, pattern))
            .map(|k| RespValue::BulkString(Some(k.as_bytes().to_vec())))
            .collect();
        RespValue::Array(Some(keys))
    }

    pub(super) fn execute_flush(&mut self) -> RespValue {
        self.data.clear();
        self.expirations.clear();
        self.access_times.clear();
        RespValue::simple("OK")
    }

    pub(super) fn execute_expire(&mut self, key: &str, seconds: i64) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else if seconds <= 0 {
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);
            RespValue::Integer(1)
        } else {
            let expiration =
                self.current_time + crate::simulator::Duration::from_secs(seconds as u64);
            self.expirations.insert(key.to_string(), expiration);
            RespValue::Integer(1)
        }
    }

    pub(super) fn execute_expireat(&mut self, key: &str, timestamp: i64) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else {
            let simulation_relative_secs = timestamp - self.simulation_start_epoch;
            if simulation_relative_secs <= 0 {
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                RespValue::Integer(1)
            } else {
                let expiration_millis = (simulation_relative_secs as u64).saturating_mul(1000);
                if expiration_millis <= self.current_time.as_millis() {
                    self.data.remove(key);
                    self.expirations.remove(key);
                    self.access_times.remove(key);
                    RespValue::Integer(1)
                } else {
                    let expiration = VirtualTime::from_millis(expiration_millis);
                    self.expirations.insert(key.to_string(), expiration);
                    RespValue::Integer(1)
                }
            }
        }
    }

    pub(super) fn execute_pexpireat(&mut self, key: &str, timestamp_millis: i64) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else {
            let simulation_relative_millis =
                timestamp_millis - (self.simulation_start_epoch * 1000);
            if simulation_relative_millis <= 0 {
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                RespValue::Integer(1)
            } else if (simulation_relative_millis as u64) <= self.current_time.as_millis() {
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                RespValue::Integer(1)
            } else {
                let expiration = VirtualTime::from_millis(simulation_relative_millis as u64);
                self.expirations.insert(key.to_string(), expiration);
                RespValue::Integer(1)
            }
        }
    }

    pub(super) fn execute_ttl(&self, key: &str) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(-2)
        } else if let Some(expiration) = self.expirations.get(key) {
            let remaining_ms = expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
            let remaining_secs = (remaining_ms / 1000).max(0);
            RespValue::Integer(remaining_secs)
        } else {
            RespValue::Integer(-1)
        }
    }

    pub(super) fn execute_pttl(&self, key: &str) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(-2)
        } else if let Some(expiration) = self.expirations.get(key) {
            let remaining = expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
            RespValue::Integer(remaining.max(0))
        } else {
            RespValue::Integer(-1)
        }
    }

    pub(super) fn execute_persist(&mut self, key: &str) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else if self.expirations.remove(key).is_some() {
            RespValue::Integer(1)
        } else {
            RespValue::Integer(0)
        }
    }
}
