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

    pub(super) fn execute_setnx(&mut self, key: &str, value: &SDS) -> RespValue {
        // Check if key exists (not expired)
        let key_exists = !self.is_expired(key) && self.data.contains_key(key);
        if key_exists {
            return RespValue::Integer(0);
        }
        // Key doesn't exist - insert and return 1
        self.data
            .insert(key.to_string(), Value::String(value.clone()));
        self.access_times.insert(key.to_string(), self.current_time);
        self.expirations.remove(key);
        RespValue::Integer(1)
    }

    pub(super) fn execute_set(
        &mut self,
        key: &str,
        value: &SDS,
        ex: &Option<i64>,
        px: &Option<i64>,
        exat: &Option<i64>,
        pxat: &Option<i64>,
        nx: &bool,
        xx: &bool,
        get: &bool,
        keepttl: &bool,
    ) -> RespValue {
        // Validate expiration values
        if let Some(seconds) = ex {
            if *seconds <= 0 || *seconds > i64::MAX / 1000 {
                return RespValue::err("ERR invalid expire time in 'set' command");
            }
        }
        if let Some(millis) = px {
            if *millis <= 0 {
                return RespValue::err("ERR invalid expire time in 'set' command");
            }
        }
        if let Some(ts) = exat {
            if *ts <= 0 {
                return RespValue::err("ERR invalid expire time in 'set' command");
            }
        }
        if let Some(ts_ms) = pxat {
            if *ts_ms <= 0 {
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
        } else if let Some(timestamp) = exat {
            // EXAT: absolute Unix timestamp in seconds — use ms-precision epoch
            let timestamp_ms = timestamp.saturating_mul(1000);
            let simulation_relative_ms = timestamp_ms.saturating_sub(self.simulation_start_epoch_ms);
            if simulation_relative_ms <= 0 {
                self.expirations.remove(key);
            } else {
                let expiration = crate::simulator::VirtualTime::from_millis(simulation_relative_ms as u64);
                self.expirations.insert(key.to_string(), expiration);
            }
        } else if let Some(timestamp_ms) = pxat {
            // PXAT: absolute Unix timestamp in milliseconds
            let simulation_relative_ms = timestamp_ms.saturating_sub(self.simulation_start_epoch_ms);
            if simulation_relative_ms <= 0 {
                self.expirations.remove(key);
            } else {
                let expiration =
                    crate::simulator::VirtualTime::from_millis(simulation_relative_ms as u64);
                self.expirations.insert(key.to_string(), expiration);
            }
        } else if !keepttl {
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

        // TigerStyle: Postcondition - result count matches input count
        debug_assert_eq!(
            values.len(),
            keys.len(),
            "Postcondition violated: MGET result count must match input key count"
        );

        RespValue::Array(Some(values))
    }

    pub(super) fn execute_mset(&mut self, pairs: &[(String, SDS)]) -> RespValue {
        for (key, value) in pairs {
            self.data.insert(key.clone(), Value::String(value.clone()));
            self.access_times.insert(key.clone(), self.current_time);
        }

        // TigerStyle: Postcondition - last value for each key is stored
        // (When duplicate keys appear in pairs, the last value wins per Redis semantics)
        #[cfg(debug_assertions)]
        {
            let mut last_values: std::collections::HashMap<&str, &SDS> =
                std::collections::HashMap::new();
            for (key, value) in pairs {
                last_values.insert(key.as_str(), value);
            }
            for (key, value) in &last_values {
                debug_assert!(
                    matches!(self.data.get(*key), Some(Value::String(v)) if v == *value),
                    "Postcondition violated: MSET must store last value for key '{}'",
                    key
                );
            }
        }

        RespValue::simple("OK")
    }

    pub(super) fn execute_msetnx(&mut self, pairs: &[(String, SDS)]) -> RespValue {
        // MSETNX: set all keys only if none of them exist
        for (key, _) in pairs {
            if !self.is_expired(key) && self.data.contains_key(key) {
                return RespValue::Integer(0);
            }
        }
        // All keys are new — set them all
        for (key, value) in pairs {
            self.data.insert(key.clone(), Value::String(value.clone()));
            self.access_times.insert(key.clone(), self.current_time);
        }
        RespValue::Integer(1)
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
                // MGET returns nil for non-string keys (not WRONGTYPE)
                Some(_) => RespValue::BulkString(None),
                None => RespValue::BulkString(None),
            });
        }

        // TigerStyle: Postcondition - result count matches input count
        debug_assert_eq!(
            results.len(),
            keys.len(),
            "Postcondition violated: batch_get result count must match input key count"
        );

        RespValue::Array(Some(results))
    }

    pub(super) fn execute_getrange(&mut self, key: &str, start: isize, end: isize) -> RespValue {
        match self.get_value(key) {
            Some(Value::String(s)) => {
                let bytes = s.as_bytes();
                let len = bytes.len() as isize;
                if len == 0 {
                    return RespValue::BulkString(Some(vec![]));
                }
                // Normalize negative indices
                let mut s_idx = if start < 0 { (len + start).max(0) } else { start.min(len) };
                let mut e_idx = if end < 0 { (len + end).max(0) } else { end.min(len - 1) };
                if s_idx > e_idx || s_idx >= len {
                    return RespValue::BulkString(Some(vec![]));
                }
                s_idx = s_idx.max(0);
                e_idx = e_idx.min(len - 1);
                let result = bytes[s_idx as usize..=e_idx as usize].to_vec();
                RespValue::BulkString(Some(result))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(Some(vec![])),
        }
    }

    pub(super) fn execute_setrange(&mut self, key: &str, offset: usize, value: &SDS) -> RespValue {
        let val_bytes = value.as_bytes();
        // Checked arithmetic: offset + value length could overflow usize
        let needed = match offset.checked_add(val_bytes.len()) {
            Some(n) => n,
            None => return RespValue::err("ERR string exceeds maximum allowed size"),
        };
        // Redis limits strings to 512MB
        if needed > 512 * 1024 * 1024 {
            return RespValue::err("ERR string exceeds maximum allowed size");
        }
        match self.get_value_mut(key) {
            Some(Value::String(s)) => {
                let mut bytes = s.as_bytes().to_vec();
                if needed > bytes.len() {
                    bytes.resize(needed, 0);
                }
                bytes[offset..needed].copy_from_slice(val_bytes);
                let new_len = bytes.len() as i64;
                self.data.insert(key.to_string(), Value::String(SDS::new(bytes)));
                RespValue::Integer(new_len)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => {
                let mut bytes = vec![0u8; needed];
                bytes[offset..needed].copy_from_slice(val_bytes);
                let new_len = bytes.len() as i64;
                self.data.insert(key.to_string(), Value::String(SDS::new(bytes)));
                self.access_times.insert(key.to_string(), self.current_time);
                RespValue::Integer(new_len)
            }
        }
    }

    pub(super) fn execute_getex(
        &mut self,
        key: &str,
        ex: &Option<i64>,
        px: &Option<i64>,
        exat: &Option<i64>,
        pxat: &Option<i64>,
        persist: bool,
    ) -> RespValue {
        // Get the value first
        let result = match self.get_value(key) {
            Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
            Some(_) => {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                )
            }
            None => return RespValue::BulkString(None),
        };

        // Key exists as string — apply expiry changes
        if let Some(seconds) = ex {
            if *seconds <= 0 || *seconds > i64::MAX / 1000 {
                return RespValue::err("ERR invalid expire time in 'getex' command");
            }
            let expiration =
                self.current_time + crate::simulator::Duration::from_secs(*seconds as u64);
            self.expirations.insert(key.to_string(), expiration);
        } else if let Some(millis) = px {
            if *millis <= 0 {
                return RespValue::err("ERR invalid expire time in 'getex' command");
            }
            let expiration =
                self.current_time + crate::simulator::Duration::from_millis(*millis as u64);
            self.expirations.insert(key.to_string(), expiration);
        } else if let Some(timestamp) = exat {
            // EXAT: use ms-precision epoch for accuracy
            let timestamp_ms = timestamp.saturating_mul(1000);
            let simulation_relative_ms = timestamp_ms.saturating_sub(self.simulation_start_epoch_ms);
            if simulation_relative_ms <= 0 {
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                return result;
            }
            let expiration = crate::simulator::VirtualTime::from_millis(simulation_relative_ms as u64);
            self.expirations.insert(key.to_string(), expiration);
        } else if let Some(timestamp_ms) = pxat {
            let simulation_relative_ms = timestamp_ms.saturating_sub(self.simulation_start_epoch_ms);
            if simulation_relative_ms <= 0 {
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                return result;
            }
            let expiration = crate::simulator::VirtualTime::from_millis(simulation_relative_ms as u64);
            self.expirations.insert(key.to_string(), expiration);
        } else if persist {
            self.expirations.remove(key);
        }

        result
    }

    pub(super) fn execute_getdel(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::String(s)) => {
                let result = RespValue::BulkString(Some(s.as_bytes().to_vec()));
                self.data.remove(key);
                self.expirations.remove(key);
                self.access_times.remove(key);
                result
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_incrbyfloat(&mut self, key: &str, increment: f64) -> RespValue {
        if increment.is_nan() || increment.is_infinite() {
            return RespValue::err("ERR increment would produce NaN or Infinity");
        }

        match self.get_value_mut(key) {
            Some(Value::String(s)) => {
                let current: f64 = match s.to_string().parse::<f64>() {
                    Ok(n) => n,
                    Err(_) => return RespValue::err("ERR value is not a valid float"),
                };
                let new_value = current + increment;
                if new_value.is_nan() || new_value.is_infinite() {
                    return RespValue::err("ERR increment would produce NaN or Infinity");
                }
                let new_str = format_float(new_value);
                let sds = SDS::from_str(&new_str);
                self.data.insert(key.to_string(), Value::String(sds));
                RespValue::BulkString(Some(new_str.into_bytes()))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => {
                let new_value = increment;
                if new_value.is_nan() || new_value.is_infinite() {
                    return RespValue::err("ERR increment would produce NaN or Infinity");
                }
                let new_str = format_float(new_value);
                let sds = SDS::from_str(&new_str);
                self.data.insert(key.to_string(), Value::String(sds));
                self.access_times.insert(key.to_string(), self.current_time);
                RespValue::BulkString(Some(new_str.into_bytes()))
            }
        }
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

/// Format a float value the way Redis does:
/// - 17 significant digits
/// - Trailing zeros kept (Redis uses `%.17Lg` C format)
fn format_float(value: f64) -> String {
    // Redis uses %.17Lg which prints 17 significant digits, removing trailing zeros
    // Handle negative zero → positive zero
    let value = if value == 0.0 { 0.0 } else { value };

    // Format with 17 significant digits
    let s = format!("{:.17e}", value);
    // Parse to get the clean representation
    let reparsed: f64 = s.parse().unwrap_or(value);

    // Use ryu or manual: format with enough precision then trim
    // Redis %.17Lg: up to 17 significant digits, no trailing zeros
    let formatted = format!("{:.17}", reparsed);

    // Trim trailing zeros after decimal point (like %g)
    if formatted.contains('.') {
        let trimmed = formatted.trim_end_matches('0');
        let trimmed = trimmed.trim_end_matches('.');
        trimmed.to_string()
    } else {
        formatted
    }
}
