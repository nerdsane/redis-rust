//! Scan command implementations for CommandExecutor.
//!
//! Handles: SCAN, HSCAN, ZSCAN
//!
//! # TigerStyle Invariants
//!
//! - SCAN returns [cursor, keys_array] where cursor is "0" when complete
//! - Result count never exceeds requested COUNT
//! - Keys are returned in sorted order for deterministic iteration

use super::CommandExecutor;
use crate::redis::data::Value;
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_scan(
        &mut self,
        cursor: u64,
        pattern: Option<&str>,
        count: Option<usize>,
    ) -> RespValue {
        let count = count.unwrap_or(10);

        // TigerStyle: Precondition - count must be reasonable
        debug_assert!(count > 0, "Precondition: SCAN count must be positive");

        // Collect all non-expired keys
        let mut keys: Vec<String> = self
            .data
            .keys()
            .filter(|k| !self.is_expired(k))
            .filter(|k| pattern.map_or(true, |p| self.matches_glob_pattern(k, p)))
            .cloned()
            .collect();
        // Sort for deterministic iteration
        keys.sort();

        // Skip to cursor position and take count+1 to know if there's more
        let results: Vec<String> = keys
            .into_iter()
            .skip(cursor as usize)
            .take(count + 1)
            .collect();

        let (next_cursor, result_keys) = if results.len() > count {
            (cursor + count as u64, &results[..count])
        } else {
            (0u64, &results[..])
        };

        // TigerStyle: Postconditions
        debug_assert!(
            result_keys.len() <= count,
            "Postcondition violated: SCAN result count must not exceed requested count"
        );
        debug_assert!(
            next_cursor == 0 || !result_keys.is_empty(),
            "Postcondition violated: non-zero cursor implies non-empty results"
        );

        RespValue::Array(Some(vec![
            RespValue::BulkString(Some(next_cursor.to_string().into_bytes())),
            RespValue::Array(Some(
                result_keys
                    .iter()
                    .map(|k| RespValue::BulkString(Some(k.as_bytes().to_vec())))
                    .collect(),
            )),
        ]))
    }

    pub(super) fn execute_hscan(
        &mut self,
        key: &str,
        cursor: u64,
        pattern: Option<&str>,
        count: Option<usize>,
    ) -> RespValue {
        // Handle expiration
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }

        // First collect all fields from the hash
        let raw_fields: Option<Vec<(String, String)>> = match self.get_value(key) {
            Some(Value::Hash(h)) => {
                Some(h.iter().map(|(f, v)| (f.clone(), v.to_string())).collect())
            }
            Some(_) => {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                )
            }
            None => None,
        };

        match raw_fields {
            Some(all_fields) => {
                let count = count.unwrap_or(10);

                // TigerStyle: Precondition
                debug_assert!(count > 0, "Precondition: HSCAN count must be positive");

                // Filter by pattern
                let mut fields: Vec<(String, String)> = all_fields
                    .into_iter()
                    .filter(|(f, _)| pattern.map_or(true, |p| self.matches_glob_pattern(f, p)))
                    .collect();
                // Sort for deterministic iteration
                fields.sort_by(|a, b| a.0.cmp(&b.0));

                // Skip to cursor position and take count+1
                let results: Vec<(String, String)> = fields
                    .into_iter()
                    .skip(cursor as usize)
                    .take(count + 1)
                    .collect();

                let (next_cursor, result_fields) = if results.len() > count {
                    (cursor + count as u64, &results[..count])
                } else {
                    (0u64, &results[..])
                };

                // TigerStyle: Postconditions
                debug_assert!(
                    result_fields.len() <= count,
                    "Postcondition violated: HSCAN result count must not exceed requested count"
                );

                // Flatten field-value pairs into array
                let elements: Vec<RespValue> = result_fields
                    .iter()
                    .flat_map(|(f, v)| {
                        vec![
                            RespValue::BulkString(Some(f.as_bytes().to_vec())),
                            RespValue::BulkString(Some(v.as_bytes().to_vec())),
                        ]
                    })
                    .collect();

                // TigerStyle: Postcondition - elements array has 2x field count (field + value)
                debug_assert_eq!(
                    elements.len(),
                    result_fields.len() * 2,
                    "Postcondition violated: HSCAN elements must have field-value pairs"
                );

                RespValue::Array(Some(vec![
                    RespValue::BulkString(Some(next_cursor.to_string().into_bytes())),
                    RespValue::Array(Some(elements)),
                ]))
            }
            None => {
                // Empty result for non-existent key
                RespValue::Array(Some(vec![
                    RespValue::BulkString(Some(b"0".to_vec())),
                    RespValue::Array(Some(vec![])),
                ]))
            }
        }
    }

    pub(super) fn execute_zscan(
        &mut self,
        key: &str,
        cursor: u64,
        pattern: Option<&str>,
        count: Option<usize>,
    ) -> RespValue {
        // Handle expiration
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }

        // First collect all members from the sorted set
        let raw_members: Option<Vec<(String, f64)>> = match self.get_value(key) {
            Some(Value::SortedSet(zs)) => {
                Some(zs.iter().map(|(m, s)| (m.to_string(), s)).collect())
            }
            Some(_) => {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                )
            }
            None => None,
        };

        match raw_members {
            Some(all_members) => {
                let count = count.unwrap_or(10);

                // TigerStyle: Precondition
                debug_assert!(count > 0, "Precondition: ZSCAN count must be positive");

                // Filter by pattern
                let mut members: Vec<(String, f64)> = all_members
                    .into_iter()
                    .filter(|(m, _)| pattern.map_or(true, |p| self.matches_glob_pattern(m, p)))
                    .collect();
                // Sort by member for deterministic iteration
                members.sort_by(|a, b| a.0.cmp(&b.0));

                // Skip to cursor position and take count+1
                let results: Vec<(String, f64)> = members
                    .into_iter()
                    .skip(cursor as usize)
                    .take(count + 1)
                    .collect();

                let (next_cursor, result_members) = if results.len() > count {
                    (cursor + count as u64, &results[..count])
                } else {
                    (0u64, &results[..])
                };

                // TigerStyle: Postconditions
                debug_assert!(
                    result_members.len() <= count,
                    "Postcondition violated: ZSCAN result count must not exceed requested count"
                );

                // Flatten member-score pairs into array
                let elements: Vec<RespValue> = result_members
                    .iter()
                    .flat_map(|(m, s)| {
                        vec![
                            RespValue::BulkString(Some(m.as_bytes().to_vec())),
                            RespValue::BulkString(Some(s.to_string().into_bytes())),
                        ]
                    })
                    .collect();

                // TigerStyle: Postcondition - elements array has 2x member count (member + score)
                debug_assert_eq!(
                    elements.len(),
                    result_members.len() * 2,
                    "Postcondition violated: ZSCAN elements must have member-score pairs"
                );

                RespValue::Array(Some(vec![
                    RespValue::BulkString(Some(next_cursor.to_string().into_bytes())),
                    RespValue::Array(Some(elements)),
                ]))
            }
            None => {
                // Empty result for non-existent key
                RespValue::Array(Some(vec![
                    RespValue::BulkString(Some(b"0".to_vec())),
                    RespValue::Array(Some(vec![])),
                ]))
            }
        }
    }
}
