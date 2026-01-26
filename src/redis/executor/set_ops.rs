//! Set command implementations for CommandExecutor.
//!
//! Handles: SADD, SREM, SMEMBERS, SISMEMBER, SCARD, SPOP

use super::CommandExecutor;
use crate::redis::data::{RedisSet, Value, SDS};
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_sadd(&mut self, key: &str, members: &[SDS]) -> RespValue {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }
        let set = self
            .data
            .entry(key.to_string())
            .or_insert_with(|| Value::Set(RedisSet::new()));
        self.access_times.insert(key.to_string(), self.current_time);
        match set {
            Value::Set(s) => {
                let mut added = 0;
                for member in members {
                    if s.add(member.clone()) {
                        added += 1;
                    }
                }
                RespValue::Integer(added)
            }
            _ => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
        }
    }

    pub(super) fn execute_srem(&mut self, key: &str, members: &[SDS]) -> RespValue {
        match self.get_value_mut(key) {
            Some(Value::Set(s)) => {
                // TigerStyle: Capture pre-state for postcondition
                #[cfg(debug_assertions)]
                let pre_len = s.len();

                let mut removed = 0i64;
                for member in members {
                    if s.remove(member) {
                        removed += 1;
                    }
                }

                // TigerStyle: Postconditions
                #[cfg(debug_assertions)]
                {
                    debug_assert!(
                        removed >= 0,
                        "Invariant violated: removed count must be non-negative"
                    );
                    debug_assert!(
                        removed <= members.len() as i64,
                        "Invariant violated: can't remove more than requested"
                    );
                    debug_assert_eq!(
                        s.len(),
                        pre_len - removed as usize,
                        "Invariant violated: len must decrease by removed count"
                    );
                }

                RespValue::Integer(removed)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_smembers(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::Set(s)) => {
                let members: Vec<RespValue> = s
                    .members()
                    .iter()
                    .map(|m| RespValue::BulkString(Some(m.as_bytes().to_vec())))
                    .collect();
                RespValue::Array(Some(members))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(Vec::new())),
        }
    }

    pub(super) fn execute_sismember(&mut self, key: &str, member: &SDS) -> RespValue {
        match self.get_value(key) {
            Some(Value::Set(s)) => RespValue::Integer(if s.contains(member) { 1 } else { 0 }),
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_scard(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::Set(s)) => {
                let card = s.len() as i64;
                // TigerStyle: Postcondition
                debug_assert!(
                    card >= 0,
                    "Invariant violated: SCARD must return non-negative"
                );
                RespValue::Integer(card)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_spop(&mut self, key: &str, count: Option<usize>) -> RespValue {
        match self.get_value_mut(key) {
            Some(Value::Set(s)) => match count {
                None => {
                    // SPOP key - return single element or nil
                    match s.pop() {
                        Some(member) => {
                            RespValue::BulkString(Some(member.to_string().into_bytes()))
                        }
                        None => RespValue::BulkString(None),
                    }
                }
                Some(n) => {
                    // SPOP key count - return array of elements
                    let members = s.pop_count(n);
                    RespValue::Array(Some(
                        members
                            .into_iter()
                            .map(|m| RespValue::BulkString(Some(m.to_string().into_bytes())))
                            .collect(),
                    ))
                }
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => {
                // Key doesn't exist
                match count {
                    None => RespValue::BulkString(None),
                    Some(_) => RespValue::Array(Some(vec![])),
                }
            }
        }
    }
}
