//! Key command implementations for CommandExecutor.
//!
//! Handles: DEL, EXISTS, TYPE, KEYS, FLUSHDB, FLUSHALL, EXPIRE, EXPIREAT,
//! PEXPIREAT, TTL, PTTL, PERSIST
//!
//! # TigerStyle Invariants
//!
//! - DEL removes keys from data, expirations, AND access_times
//! - EXISTS count is always in range [0, keys.len()]
//! - TTL/PTTL returns -2 (not exists), -1 (no expiry), or >= 0 (remaining)
//! - FLUSH clears all three maps completely

use super::CommandExecutor;
use crate::redis::data::Value;
use crate::redis::resp::RespValue;
use crate::simulator::VirtualTime;

impl CommandExecutor {
    pub(super) fn execute_del(&mut self, keys: &[String]) -> RespValue {
        // TigerStyle: Capture pre-state for postcondition
        #[cfg(debug_assertions)]
        let pre_data_len = self.data.len();

        let mut count = 0i64;
        for key in keys {
            if self.data.remove(key).is_some() {
                count += 1;
            }
            self.expirations.remove(key);
            self.access_times.remove(key);
        }

        // TigerStyle: Postconditions
        debug_assert!(
            count >= 0 && count <= keys.len() as i64,
            "Postcondition violated: DEL count must be in [0, keys.len()]"
        );
        #[cfg(debug_assertions)]
        {
            // Verify deleted keys are truly gone from all maps
            for key in keys {
                if !self.data.contains_key(key) {
                    debug_assert!(
                        !self.expirations.contains_key(key),
                        "Postcondition violated: deleted key must not have expiration"
                    );
                }
            }
            // Data length should have decreased by exactly count
            debug_assert_eq!(
                self.data.len(),
                pre_data_len - count as usize,
                "Postcondition violated: data.len() must decrease by deleted count"
            );
        }

        RespValue::Integer(count)
    }

    pub(super) fn execute_exists(&self, keys: &[String]) -> RespValue {
        let count = keys
            .iter()
            .filter(|k| !self.is_expired(k) && self.data.contains_key(*k))
            .count();

        // TigerStyle: Postcondition - count must be valid
        debug_assert!(
            count <= keys.len(),
            "Postcondition violated: EXISTS count cannot exceed input keys count"
        );

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

        // TigerStyle: Postconditions - all state must be cleared
        debug_assert!(
            self.data.is_empty(),
            "Postcondition violated: data must be empty after FLUSH"
        );
        debug_assert!(
            self.expirations.is_empty(),
            "Postcondition violated: expirations must be empty after FLUSH"
        );
        debug_assert!(
            self.access_times.is_empty(),
            "Postcondition violated: access_times must be empty after FLUSH"
        );

        RespValue::simple("OK")
    }

    pub(super) fn execute_expire(&mut self, key: &str, seconds: i64) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else if seconds <= 0 {
            // Negative/zero TTL means delete immediately
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);

            // TigerStyle: Postcondition - key must be fully removed
            debug_assert!(
                !self.data.contains_key(key),
                "Postcondition violated: key must be deleted when EXPIRE <= 0"
            );

            RespValue::Integer(1)
        } else {
            let expiration =
                self.current_time + crate::simulator::Duration::from_secs(seconds as u64);
            self.expirations.insert(key.to_string(), expiration);

            // TigerStyle: Postcondition - expiration must be set
            debug_assert!(
                self.expirations.contains_key(key),
                "Postcondition violated: key must have expiration after EXPIRE"
            );
            debug_assert!(
                self.expirations.get(key).map(|e| e.as_millis())
                    > Some(self.current_time.as_millis()),
                "Postcondition violated: expiration must be in the future"
            );

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
        let result = if self.is_expired(key) || !self.data.contains_key(key) {
            -2i64 // Key does not exist
        } else if let Some(expiration) = self.expirations.get(key) {
            let remaining_ms = expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
            (remaining_ms / 1000).max(0)
        } else {
            -1i64 // Key exists but has no associated expire
        };

        // TigerStyle: Postcondition - result must be valid Redis TTL value
        debug_assert!(result >= -2, "Postcondition violated: TTL must be >= -2");

        RespValue::Integer(result)
    }

    pub(super) fn execute_pttl(&self, key: &str) -> RespValue {
        let result = if self.is_expired(key) || !self.data.contains_key(key) {
            -2i64 // Key does not exist
        } else if let Some(expiration) = self.expirations.get(key) {
            let remaining = expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
            remaining.max(0)
        } else {
            -1i64 // Key exists but has no associated expire
        };

        // TigerStyle: Postcondition - result must be valid Redis PTTL value
        debug_assert!(result >= -2, "Postcondition violated: PTTL must be >= -2");

        RespValue::Integer(result)
    }

    pub(super) fn execute_persist(&mut self, key: &str) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else if self.expirations.remove(key).is_some() {
            // TigerStyle: Postcondition - key must no longer have expiration
            debug_assert!(
                !self.expirations.contains_key(key),
                "Postcondition violated: key must not have expiration after PERSIST"
            );
            debug_assert!(
                self.data.contains_key(key),
                "Postcondition violated: key data must still exist after PERSIST"
            );

            RespValue::Integer(1)
        } else {
            RespValue::Integer(0)
        }
    }
}
