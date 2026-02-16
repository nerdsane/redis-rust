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

    pub(super) fn execute_expire(
        &mut self,
        key: &str,
        seconds: i64,
        nx: bool,
        xx: bool,
        gt: bool,
        lt: bool,
    ) -> RespValue {
        // Reject values that would overflow when converted to ms or when adding basetime
        let max_expire_secs = i64::MAX / 1000;
        if seconds > max_expire_secs || seconds < (i64::MIN / 1000) {
            return RespValue::err("ERR invalid expire time in 'expire' command");
        }
        // Also check if seconds*1000 + basetime_ms overflows
        let expire_ms = seconds.saturating_mul(1000);
        if expire_ms > 0 {
            let basetime_ms = self.simulation_start_epoch_ms + self.current_time.as_millis() as i64;
            if expire_ms > i64::MAX - basetime_ms {
                return RespValue::err("ERR invalid expire time in 'expire' command");
            }
        }
        if self.is_expired(key) || !self.data.contains_key(key) {
            return RespValue::Integer(0);
        }
        if seconds <= 0 {
            // Negative/zero TTL means delete immediately (skip flag checks for delete)
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);
            return RespValue::Integer(1);
        }

        let new_expiration =
            self.current_time + crate::simulator::Duration::from_secs(seconds as u64);
        let current_expiration = self.expirations.get(key).copied();
        let has_expiry = current_expiration.is_some();

        // NX: Only set if key has no expiry
        if nx && has_expiry {
            return RespValue::Integer(0);
        }
        // XX: Only set if key already has an expiry
        if xx && !has_expiry {
            return RespValue::Integer(0);
        }
        // GT: Only set if new expiry > current expiry (no expiry = persistent = infinity)
        if gt {
            match current_expiration {
                Some(current) if new_expiration.as_millis() <= current.as_millis() => {
                    return RespValue::Integer(0);
                }
                None => {
                    // No current expiry means key is persistent (infinite TTL)
                    // Any finite TTL < infinity, so GT fails
                    return RespValue::Integer(0);
                }
                _ => {}
            }
        }
        // LT: Only set if new expiry < current expiry (no expiry = persistent = infinity)
        if lt {
            if let Some(current) = current_expiration {
                if new_expiration.as_millis() >= current.as_millis() {
                    return RespValue::Integer(0);
                }
            }
            // No current expiry: any finite TTL < infinity, so LT succeeds — fall through
        }

        self.expirations.insert(key.to_string(), new_expiration);
        RespValue::Integer(1)
    }

    pub(super) fn execute_pexpire(
        &mut self,
        key: &str,
        milliseconds: i64,
        nx: bool,
        xx: bool,
        gt: bool,
        lt: bool,
    ) -> RespValue {
        // Reject values that would overflow when adding basetime
        if milliseconds > 0 {
            let basetime_ms = self.simulation_start_epoch_ms + self.current_time.as_millis() as i64;
            if milliseconds > i64::MAX - basetime_ms {
                return RespValue::err("ERR invalid expire time in 'pexpire' command");
            }
        } else if milliseconds < i64::MIN / 2 {
            return RespValue::err("ERR invalid expire time in 'pexpire' command");
        }
        if self.is_expired(key) || !self.data.contains_key(key) {
            return RespValue::Integer(0);
        }
        if milliseconds <= 0 {
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);
            return RespValue::Integer(1);
        }

        let new_expiration =
            self.current_time + crate::simulator::Duration::from_millis(milliseconds as u64);
        let current_expiration = self.expirations.get(key).copied();
        let has_expiry = current_expiration.is_some();

        if nx && has_expiry {
            return RespValue::Integer(0);
        }
        if xx && !has_expiry {
            return RespValue::Integer(0);
        }
        if gt {
            match current_expiration {
                Some(current) if new_expiration.as_millis() <= current.as_millis() => {
                    return RespValue::Integer(0);
                }
                None => return RespValue::Integer(0),
                _ => {}
            }
        }
        if lt {
            if let Some(current) = current_expiration {
                if new_expiration.as_millis() >= current.as_millis() {
                    return RespValue::Integer(0);
                }
            }
        }

        self.expirations.insert(key.to_string(), new_expiration);
        RespValue::Integer(1)
    }

    pub(super) fn execute_expiretime(&self, key: &str) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            return RespValue::Integer(-2);
        }
        match self.expirations.get(key) {
            Some(expiration) => {
                let epoch_secs = self
                    .simulation_start_epoch_ms
                    .saturating_add(expiration.as_millis() as i64)
                    / 1000;
                RespValue::Integer(epoch_secs)
            }
            None => RespValue::Integer(-1),
        }
    }

    pub(super) fn execute_pexpiretime(&self, key: &str) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            return RespValue::Integer(-2);
        }
        match self.expirations.get(key) {
            Some(expiration) => {
                let epoch_ms = self
                    .simulation_start_epoch_ms
                    .saturating_add(expiration.as_millis() as i64);
                RespValue::Integer(epoch_ms)
            }
            None => RespValue::Integer(-1),
        }
    }

    pub(super) fn execute_expireat(&mut self, key: &str, timestamp: i64) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else {
            // Convert to ms and use ms-precision epoch for accuracy
            let timestamp_ms = timestamp.saturating_mul(1000);
            let simulation_relative_ms =
                timestamp_ms.saturating_sub(self.simulation_start_epoch_ms);
            if simulation_relative_ms <= 0 {
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                RespValue::Integer(1)
            } else if (simulation_relative_ms as u64) <= self.current_time.as_millis() {
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                RespValue::Integer(1)
            } else {
                let expiration = VirtualTime::from_millis(simulation_relative_ms as u64);
                self.expirations.insert(key.to_string(), expiration);
                RespValue::Integer(1)
            }
        }
    }

    pub(super) fn execute_pexpireat(&mut self, key: &str, timestamp_millis: i64) -> RespValue {
        if self.is_expired(key) || !self.data.contains_key(key) {
            RespValue::Integer(0)
        } else {
            let simulation_relative_millis =
                timestamp_millis.saturating_sub(self.simulation_start_epoch_ms);
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
            ((remaining_ms + 999) / 1000).max(0)
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
