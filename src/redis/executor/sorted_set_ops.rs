//! Sorted set command implementations for CommandExecutor.
//!
//! Handles: ZADD, ZREM, ZRANGE, ZREVRANGE, ZSCORE, ZRANK, ZCARD, ZCOUNT, ZRANGEBYSCORE

use super::CommandExecutor;
use crate::redis::data::{RedisSortedSet, Value, SDS};
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_zadd(
        &mut self,
        key: &str,
        pairs: &[(f64, SDS)],
        nx: bool,
        xx: bool,
        gt: bool,
        lt: bool,
        ch: bool,
    ) -> RespValue {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
        }
        let zset = self
            .data
            .entry(key.to_string())
            .or_insert_with(|| Value::SortedSet(RedisSortedSet::new()));
        self.access_times.insert(key.to_string(), self.current_time);
        match zset {
            Value::SortedSet(zs) => {
                let mut added = 0i64;
                let mut changed = 0i64;

                // Fast path: no flags set (common case)
                if !nx && !xx && !gt && !lt && !ch {
                    for (score, member) in pairs {
                        if zs.add(member.clone(), *score) {
                            added += 1;
                        }
                    }
                    return RespValue::Integer(added);
                }

                // Slow path: flags are set
                for (score, member) in pairs {
                    // Single lookup for current score
                    let current_score = zs.score(member);
                    let exists = current_score.is_some();

                    // NX: only add new elements
                    if nx && exists {
                        continue;
                    }
                    // XX: only update existing elements
                    if xx && !exists {
                        continue;
                    }
                    // GT: only update when new score > current
                    if gt {
                        if let Some(cs) = current_score {
                            if *score <= cs {
                                continue;
                            }
                        }
                    }
                    // LT: only update when new score < current
                    if lt {
                        if let Some(cs) = current_score {
                            if *score >= cs {
                                continue;
                            }
                        }
                    }

                    let was_added = zs.add(member.clone(), *score);
                    if was_added {
                        added += 1;
                        changed += 1;
                    } else if current_score != Some(*score) {
                        // Score was updated
                        changed += 1;
                    }
                }
                // CH: return number changed, not just added
                if ch {
                    RespValue::Integer(changed)
                } else {
                    RespValue::Integer(added)
                }
            }
            _ => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
        }
    }

    pub(super) fn execute_zrem(&mut self, key: &str, members: &[SDS]) -> RespValue {
        match self.get_value_mut(key) {
            Some(Value::SortedSet(zs)) => {
                // TigerStyle: Capture pre-state for postcondition
                #[cfg(debug_assertions)]
                let pre_len = zs.len();

                let mut removed = 0i64;
                for member in members {
                    if zs.remove(member) {
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
                        zs.len(),
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

    pub(super) fn execute_zrange(&mut self, key: &str, start: isize, stop: isize) -> RespValue {
        match self.get_value(key) {
            Some(Value::SortedSet(zs)) => {
                let range = zs.range(start, stop);
                let elements: Vec<RespValue> = range
                    .iter()
                    .map(|(m, _)| RespValue::BulkString(Some(m.as_bytes().to_vec())))
                    .collect();
                RespValue::Array(Some(elements))
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(Vec::new())),
        }
    }

    pub(super) fn execute_zrevrange(
        &mut self,
        key: &str,
        start: isize,
        stop: isize,
        with_scores: bool,
    ) -> RespValue {
        match self.get_value(key) {
            Some(Value::SortedSet(zs)) => {
                let range = zs.rev_range(start, stop);
                if with_scores {
                    let mut elements = Vec::with_capacity(range.len() * 2);
                    for (m, s) in range {
                        elements.push(RespValue::BulkString(Some(m.as_bytes().to_vec())));
                        elements.push(RespValue::BulkString(Some(s.to_string().into_bytes())));
                    }
                    RespValue::Array(Some(elements))
                } else {
                    let elements: Vec<RespValue> = range
                        .iter()
                        .map(|(m, _)| RespValue::BulkString(Some(m.as_bytes().to_vec())))
                        .collect();
                    RespValue::Array(Some(elements))
                }
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(Vec::new())),
        }
    }

    pub(super) fn execute_zscore(&mut self, key: &str, member: &SDS) -> RespValue {
        match self.get_value(key) {
            Some(Value::SortedSet(zs)) => match zs.score(member) {
                Some(score) => {
                    let score_str = score.to_string();
                    RespValue::BulkString(Some(score_str.into_bytes()))
                }
                None => RespValue::BulkString(None),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_zrank(&mut self, key: &str, member: &SDS) -> RespValue {
        match self.get_value(key) {
            Some(Value::SortedSet(zs)) => match zs.rank(member) {
                Some(rank) => {
                    // TigerStyle: Postcondition - rank must be valid index
                    debug_assert!(
                        rank < zs.len(),
                        "Invariant violated: rank must be less than zset length"
                    );
                    RespValue::Integer(rank as i64)
                }
                None => RespValue::BulkString(None),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    pub(super) fn execute_zcard(&mut self, key: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::SortedSet(zs)) => {
                let card = zs.len() as i64;
                // TigerStyle: Postcondition
                debug_assert!(
                    card >= 0,
                    "Invariant violated: ZCARD must return non-negative"
                );
                RespValue::Integer(card)
            }
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_zcount(&mut self, key: &str, min: &str, max: &str) -> RespValue {
        match self.get_value(key) {
            Some(Value::SortedSet(zs)) => match zs.count_in_range(min, max) {
                Ok(count) => RespValue::Integer(count as i64),
                Err(e) => RespValue::err(e),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Integer(0),
        }
    }

    pub(super) fn execute_zrangebyscore(
        &mut self,
        key: &str,
        min: &str,
        max: &str,
        with_scores: bool,
        limit: &Option<(isize, usize)>,
    ) -> RespValue {
        match self.get_value(key) {
            Some(Value::SortedSet(zs)) => match zs.range_by_score(min, max, with_scores, *limit) {
                Ok(results) => {
                    let elements: Vec<RespValue> = results
                        .iter()
                        .flat_map(|(member, score)| {
                            let mut v =
                                vec![RespValue::BulkString(Some(member.as_bytes().to_vec()))];
                            if let Some(s) = score {
                                v.push(RespValue::BulkString(Some(s.to_string().into_bytes())));
                            }
                            v
                        })
                        .collect();
                    RespValue::Array(Some(elements))
                }
                Err(e) => RespValue::err(e),
            },
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::Array(Some(vec![])),
        }
    }
}
