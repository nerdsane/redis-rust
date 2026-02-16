//! Redis command executor module.
//!
//! This module implements the `CommandExecutor` state machine that executes Redis commands.
//! The implementation is split across multiple files for maintainability:
//!
//! - `mod.rs` (this file): Core struct, constructor, and execute dispatch
//! - `string_ops.rs`: String command implementations (GET, SET, APPEND, etc.)
//! - `key_ops.rs`: Key command implementations (DEL, EXISTS, EXPIRE, TTL, etc.)
//! - `list_ops.rs`: List command implementations (LPUSH, RPUSH, LRANGE, etc.)
//! - `set_ops.rs`: Set command implementations (SADD, SREM, SMEMBERS, etc.)
//! - `hash_ops.rs`: Hash command implementations (HSET, HGET, HGETALL, etc.)
//! - `sorted_set_ops.rs`: Sorted set implementations (ZADD, ZRANGE, ZSCORE, etc.)
//! - `scan_ops.rs`: Scan command implementations (SCAN, HSCAN, ZSCAN)
//! - `transaction_ops.rs`: Transaction implementations (MULTI, EXEC, DISCARD)
//! - `script_ops.rs`: Lua scripting implementations (EVAL, EVALSHA, SCRIPT)
//! - `acl_ops.rs`: ACL command implementations

mod acl_ops;
mod config_ops;
mod hash_ops;
mod key_ops;
mod list_ops;
mod scan_ops;
mod script_ops;
mod set_ops;
mod sorted_set_ops;
mod string_ops;
mod transaction_ops;

use super::command::Command;
use super::data::*;
use super::resp::RespValue;
use crate::simulator::VirtualTime;
use ahash::AHashMap;

/// Redis command executor - the state machine that processes commands.
///
/// This struct maintains the key-value store state including:
/// - Data storage (strings, lists, sets, hashes, sorted sets)
/// - Key expiration tracking
/// - Transaction state (MULTI/EXEC)
/// - Script cache for Lua scripting
pub struct CommandExecutor {
    pub(crate) data: AHashMap<String, Value>,
    pub(crate) expirations: AHashMap<String, VirtualTime>,
    pub(crate) current_time: VirtualTime,
    pub(crate) access_times: AHashMap<String, VirtualTime>,
    #[allow(dead_code)]
    pub(crate) key_count: usize,
    pub(crate) commands_processed: usize,
    pub(crate) simulation_start_epoch: i64,
    /// Exact server start time in milliseconds (for precise PEXPIREAT/PXAT)
    pub(crate) simulation_start_epoch_ms: i64,
    // Transaction state
    pub(crate) in_transaction: bool,
    pub(crate) queued_commands: Vec<Command>,
    pub(crate) watched_keys: AHashMap<String, Option<Value>>,
    // Lua scripting - local cache for single-shard mode
    pub(crate) script_cache: super::lua::ScriptCache,
    // Shared script cache for multi-shard mode (all shards share one cache)
    pub(crate) shared_script_cache: Option<super::lua::SharedScriptCache>,
    // Server configuration for CONFIG GET/SET
    pub(crate) config: config_ops::ServerConfig,
}

impl CommandExecutor {
    pub fn new() -> Self {
        CommandExecutor {
            data: AHashMap::new(),
            expirations: AHashMap::new(),
            current_time: VirtualTime::from_millis(0),
            access_times: AHashMap::new(),
            key_count: 0,
            commands_processed: 0,
            simulation_start_epoch: 0,
            simulation_start_epoch_ms: 0,
            in_transaction: false,
            queued_commands: Vec::new(),
            watched_keys: AHashMap::new(),
            script_cache: super::lua::ScriptCache::new(),
            shared_script_cache: None,
            config: config_ops::ServerConfig::new(),
        }
    }

    /// Create a new CommandExecutor with a shared script cache
    pub fn with_shared_script_cache(shared_cache: super::lua::SharedScriptCache) -> Self {
        CommandExecutor {
            data: AHashMap::new(),
            expirations: AHashMap::new(),
            current_time: VirtualTime::from_millis(0),
            access_times: AHashMap::new(),
            key_count: 0,
            commands_processed: 0,
            simulation_start_epoch: 0,
            simulation_start_epoch_ms: 0,
            in_transaction: false,
            queued_commands: Vec::new(),
            watched_keys: AHashMap::new(),
            script_cache: super::lua::ScriptCache::new(),
            shared_script_cache: Some(shared_cache),
            config: config_ops::ServerConfig::new(),
        }
    }

    /// Set the shared script cache (for updating after creation)
    pub fn set_shared_script_cache(&mut self, shared_cache: super::lua::SharedScriptCache) {
        self.shared_script_cache = Some(shared_cache);
    }

    pub fn set_simulation_start_epoch(&mut self, epoch: i64) {
        self.simulation_start_epoch = epoch;
        // Default ms value from seconds if not set separately
        if self.simulation_start_epoch_ms == 0 {
            self.simulation_start_epoch_ms = epoch * 1000;
        }
    }

    pub fn set_simulation_start_epoch_ms(&mut self, epoch_ms: i64) {
        self.simulation_start_epoch_ms = epoch_ms;
    }

    pub fn set_time(&mut self, time: VirtualTime) {
        self.current_time = time;
        self.evict_expired_keys();
    }

    pub fn get_current_time(&self) -> VirtualTime {
        self.current_time
    }

    /// Update time without evicting keys (for read-only operations)
    pub fn update_time_readonly(&mut self, time: VirtualTime) {
        self.current_time = time;
    }

    pub(crate) fn is_expired(&self, key: &str) -> bool {
        if let Some(expiration) = self.expirations.get(key) {
            *expiration <= self.current_time
        } else {
            false
        }
    }

    /// Fast path GET - avoids Command enum overhead
    #[inline]
    pub fn get_direct(&mut self, key: &str) -> RespValue {
        self.commands_processed += 1;
        match self.get_value(key) {
            Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
            Some(_) => {
                RespValue::err("WRONGTYPE Operation against a key holding the wrong kind of value")
            }
            None => RespValue::BulkString(None),
        }
    }

    /// Fast path SET - avoids Command enum overhead
    #[inline]
    pub fn set_direct(&mut self, key: &str, value: &[u8]) -> RespValue {
        self.commands_processed += 1;

        #[cfg(feature = "opt-single-key-alloc")]
        {
            let key_owned = key.to_string();
            self.data
                .insert(key_owned.clone(), Value::String(SDS::new(value.to_vec())));
            self.expirations.remove(key);
            self.access_times.insert(key_owned, self.current_time);
        }

        #[cfg(not(feature = "opt-single-key-alloc"))]
        {
            self.data
                .insert(key.to_string(), Value::String(SDS::new(value.to_vec())));
            self.expirations.remove(key);
            self.access_times.insert(key.to_string(), self.current_time);
        }

        #[cfg(debug_assertions)]
        {
            debug_assert!(self.data.contains_key(key), "Postcondition: set_direct must store key");
            debug_assert!(!self.expirations.contains_key(key), "Postcondition: set_direct must clear expiration");
        }

        RespValue::ok()
    }

    /// Direct expiration eviction - call this from TTL manager
    pub fn evict_expired_direct(&mut self, current_time: VirtualTime) -> usize {
        #[cfg(debug_assertions)]
        let pre_data_len = self.data.len();
        #[cfg(debug_assertions)]
        let pre_exp_len = self.expirations.len();

        self.current_time = current_time;

        let expired_keys: Vec<String> = self
            .expirations
            .iter()
            .filter(|(_, &exp_time)| exp_time <= self.current_time)
            .map(|(k, _)| k.clone())
            .collect();

        let count = expired_keys.len();
        for key in expired_keys {
            self.data.remove(&key);
            self.expirations.remove(&key);
            self.access_times.remove(&key);
        }

        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.data.len(),
                pre_data_len.saturating_sub(count),
                "Postcondition: data size must decrease by evicted count"
            );
            debug_assert_eq!(
                self.expirations.len(),
                pre_exp_len.saturating_sub(count),
                "Postcondition: expirations size must decrease by evicted count"
            );
        }

        count
    }

    pub(crate) fn evict_expired_keys(&mut self) {
        let expired_keys: Vec<String> = self
            .expirations
            .iter()
            .filter(|(_, &exp_time)| exp_time <= self.current_time)
            .map(|(k, _)| k.clone())
            .collect();

        for key in expired_keys {
            self.data.remove(&key);
            self.expirations.remove(&key);
            self.access_times.remove(&key);
        }
    }

    pub(crate) fn get_value(&mut self, key: &str) -> Option<&Value> {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);
            None
        } else {
            self.access_times.insert(key.to_string(), self.current_time);
            self.data.get(key)
        }
    }

    pub(crate) fn get_value_mut(&mut self, key: &str) -> Option<&mut Value> {
        if self.is_expired(key) {
            self.data.remove(key);
            self.expirations.remove(key);
            self.access_times.remove(key);
            None
        } else {
            self.access_times.insert(key.to_string(), self.current_time);
            self.data.get_mut(key)
        }
    }

    /// Get read-only access to the data store
    pub fn get_data(&self) -> &AHashMap<String, Value> {
        &self.data
    }

    /// Execute a read-only command
    pub fn execute_read(&mut self, cmd: &Command) -> RespValue {
        debug_assert!(cmd.is_read_only(), "execute_read called with write command");
        self.execute(cmd)
    }

    /// Execute a read-only command without updating access times
    pub fn execute_readonly(&self, cmd: &Command) -> RespValue {
        match cmd {
            Command::Get(key) => {
                if self.is_expired(key) {
                    return RespValue::BulkString(None);
                }
                match self.data.get(key) {
                    Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                    Some(_) => RespValue::err("WRONGTYPE"),
                    None => RespValue::BulkString(None),
                }
            }
            Command::Exists(keys) => {
                let count = keys
                    .iter()
                    .filter(|k| !self.is_expired(k) && self.data.contains_key(*k))
                    .count();
                RespValue::Integer(count as i64)
            }
            Command::Keys(pattern) => {
                let matching: Vec<RespValue> = self
                    .data
                    .keys()
                    .filter(|k| !self.is_expired(k) && self.matches_glob_pattern(k, pattern))
                    .map(|k| RespValue::BulkString(Some(k.as_bytes().to_vec())))
                    .collect();
                RespValue::Array(Some(matching))
            }
            Command::Ping(None) => RespValue::simple("PONG"),
            Command::Ping(Some(msg)) => RespValue::BulkString(Some(msg.as_bytes().to_vec())),
            _ => RespValue::err("ERR command not supported in readonly mode"),
        }
    }

    /// Main command execution entry point
    pub fn execute(&mut self, cmd: &Command) -> RespValue {
        self.commands_processed += 1;

        // BUGGIFY: Fault injection at execute() boundary (simulation only)
        #[cfg(feature = "simulation")]
        {
            use crate::buggify::faults;
            // process::SLOW - simulate processing delay (counted, not actually delayed)
            if crate::buggify::should_buggify(
                &mut crate::io::simulation::SimulatedRng::new(self.current_time.as_millis()),
                faults::process::SLOW,
            ) {
                self.commands_processed += 0; // no-op marker for stats
            }

            // timer::JUMP_FORWARD - advance time for expiry-dependent ops
            if crate::buggify::should_buggify(
                &mut crate::io::simulation::SimulatedRng::new(self.current_time.as_millis()),
                faults::timer::JUMP_FORWARD,
            ) {
                let jump_ms = 5000; // 5 second jump
                let new_time = crate::simulator::VirtualTime::from_millis(
                    self.current_time.as_millis() + jump_ms,
                );
                self.current_time = new_time;
                self.evict_expired_keys();
            }
        }

        // Handle command queueing when in transaction
        if self.in_transaction {
            match cmd {
                // These commands are executed immediately even in transaction
                Command::Exec | Command::Discard | Command::Multi => {}
                // WATCH inside MULTI is an error (not queued)
                Command::Watch(_) => {
                    return RespValue::err("ERR WATCH inside MULTI is not allowed");
                }
                // All other commands get queued
                _ => {
                    self.queued_commands.push(cmd.clone());
                    return RespValue::simple("QUEUED");
                }
            }
        }

        match cmd {
            // Server commands
            Command::Ping(None) => RespValue::simple("PONG"),
            Command::Ping(Some(msg)) => RespValue::BulkString(Some(msg.as_bytes().to_vec())),
            Command::Info => self.execute_info(),
            Command::DbSize => self.execute_dbsize(),
            Command::Time => {
                // Return real wall-clock time as [seconds, microseconds]
                let epoch_secs = self.simulation_start_epoch
                    .saturating_add(self.current_time.as_millis() as i64 / 1000);
                let remaining_us = (self.current_time.as_millis() % 1000) * 1000;
                RespValue::Array(Some(vec![
                    RespValue::BulkString(Some(epoch_secs.to_string().into_bytes())),
                    RespValue::BulkString(Some(remaining_us.to_string().into_bytes())),
                ]))
            }

            // String commands
            Command::Get(key) => self.execute_get(key),
            Command::Set {
                key,
                value,
                ex,
                px,
                exat,
                pxat,
                nx,
                xx,
                get,
                keepttl,
            } => self.execute_set(key, value, ex, px, exat, pxat, nx, xx, get, keepttl),
            Command::SetNx(key, value) => self.execute_setnx(key, value),
            Command::Append(key, value) => self.execute_append(key, value),
            Command::GetSet(key, value) => self.execute_getset(key, value),
            Command::StrLen(key) => self.execute_strlen(key),
            Command::MGet(keys) => self.execute_mget(keys),
            Command::MSet(pairs) => self.execute_mset(pairs),
            Command::MSetNx(pairs) => self.execute_msetnx(pairs),
            Command::BatchSet(pairs) => self.execute_batch_set(pairs),
            Command::BatchGet(keys) => self.execute_batch_get(keys),
            Command::GetRange(key, start, end) => self.execute_getrange(key, *start, *end),
            Command::SetRange(key, offset, value) => self.execute_setrange(key, *offset, value),
            Command::GetEx {
                key,
                ex,
                px,
                exat,
                pxat,
                persist,
            } => self.execute_getex(key, ex, px, exat, pxat, *persist),
            Command::GetDel(key) => self.execute_getdel(key),
            Command::Incr(key) => self.incr_by_impl(key, 1),
            Command::Decr(key) => self.incr_by_impl(key, -1),
            Command::IncrBy(key, increment) => self.incr_by_impl(key, *increment),
            Command::DecrBy(key, decrement) => decrement
                .checked_neg()
                .map(|neg| self.incr_by_impl(key, neg))
                .unwrap_or_else(|| RespValue::err("ERR value is out of range")),
            Command::IncrByFloat(key, increment) => self.execute_incrbyfloat(key, *increment),

            // Key commands
            Command::Del(keys) => self.execute_del(keys),
            Command::Exists(keys) => self.execute_exists(keys),
            Command::TypeOf(key) => self.execute_typeof(key),
            Command::Keys(pattern) => self.execute_keys(pattern),
            Command::FlushDb | Command::FlushAll => self.execute_flush(),
            Command::Expire {
                key,
                seconds,
                nx,
                xx,
                gt,
                lt,
            } => self.execute_expire(key, *seconds, *nx, *xx, *gt, *lt),
            Command::ExpireAt(key, timestamp) => self.execute_expireat(key, *timestamp),
            Command::PExpire {
                key,
                milliseconds,
                nx,
                xx,
                gt,
                lt,
            } => self.execute_pexpire(key, *milliseconds, *nx, *xx, *gt, *lt),
            Command::PExpireAt(key, timestamp_millis) => {
                self.execute_pexpireat(key, *timestamp_millis)
            }
            Command::Ttl(key) => self.execute_ttl(key),
            Command::Pttl(key) => self.execute_pttl(key),
            Command::ExpireTime(key) => self.execute_expiretime(key),
            Command::PExpireTime(key) => self.execute_pexpiretime(key),
            Command::Persist(key) => self.execute_persist(key),

            // List commands
            Command::LPush(key, values) => self.execute_lpush(key, values),
            Command::RPush(key, values) => self.execute_rpush(key, values),
            Command::LPop(key) => self.execute_lpop(key),
            Command::RPop(key) => self.execute_rpop(key),
            Command::LLen(key) => self.execute_llen(key),
            Command::LIndex(key, index) => self.execute_lindex(key, *index),
            Command::LRange(key, start, stop) => self.execute_lrange(key, *start, *stop),
            Command::LSet(key, index, value) => self.execute_lset(key, *index, value),
            Command::LTrim(key, start, stop) => self.execute_ltrim(key, *start, *stop),
            Command::RPopLPush(source, dest) => self.execute_rpoplpush(source, dest),
            Command::LMove {
                source,
                dest,
                wherefrom,
                whereto,
            } => self.execute_lmove(source, dest, wherefrom, whereto),

            // Set commands
            Command::SAdd(key, members) => self.execute_sadd(key, members),
            Command::SRem(key, members) => self.execute_srem(key, members),
            Command::SMembers(key) => self.execute_smembers(key),
            Command::SIsMember(key, member) => self.execute_sismember(key, member),
            Command::SCard(key) => self.execute_scard(key),
            Command::SPop(key, count) => self.execute_spop(key, *count),

            // Hash commands
            Command::HSet(key, pairs) => self.execute_hset(key, pairs),
            Command::HGet(key, field) => self.execute_hget(key, field),
            Command::HDel(key, fields) => self.execute_hdel(key, fields),
            Command::HGetAll(key) => self.execute_hgetall(key),
            Command::HKeys(key) => self.execute_hkeys(key),
            Command::HVals(key) => self.execute_hvals(key),
            Command::HLen(key) => self.execute_hlen(key),
            Command::HExists(key, field) => self.execute_hexists(key, field),
            Command::HIncrBy(key, field, increment) => self.execute_hincrby(key, field, *increment),

            // Sorted set commands
            Command::ZAdd {
                key,
                pairs,
                nx,
                xx,
                gt,
                lt,
                ch,
            } => self.execute_zadd(key, pairs, *nx, *xx, *gt, *lt, *ch),
            Command::ZRem(key, members) => self.execute_zrem(key, members),
            Command::ZRange(key, start, stop, with_scores) => {
                self.execute_zrange(key, *start, *stop, *with_scores)
            }
            Command::ZRevRange(key, start, stop, with_scores) => {
                self.execute_zrevrange(key, *start, *stop, *with_scores)
            }
            Command::ZScore(key, member) => self.execute_zscore(key, member),
            Command::ZRank(key, member) => self.execute_zrank(key, member),
            Command::ZCard(key) => self.execute_zcard(key),
            Command::ZCount(key, min, max) => self.execute_zcount(key, min, max),
            Command::ZRangeByScore {
                key,
                min,
                max,
                with_scores,
                limit,
            } => self.execute_zrangebyscore(key, min, max, *with_scores, limit),

            // Scan commands
            Command::Scan {
                cursor,
                pattern,
                count,
            } => self.execute_scan(*cursor, pattern.as_deref(), *count),
            Command::HScan {
                key,
                cursor,
                pattern,
                count,
            } => self.execute_hscan(key, *cursor, pattern.as_deref(), *count),
            Command::ZScan {
                key,
                cursor,
                pattern,
                count,
            } => self.execute_zscan(key, *cursor, pattern.as_deref(), *count),

            // Transaction commands
            Command::Multi => self.execute_multi(),
            Command::Exec => self.execute_exec(),
            Command::Discard => self.execute_discard(),
            Command::Watch(keys) => self.execute_watch(keys),
            Command::Unwatch => self.execute_unwatch(),

            // Script commands
            Command::Eval { script, keys, args } => self.execute_eval(script, keys, args),
            Command::EvalSha { sha1, keys, args } => self.execute_evalsha(sha1, keys, args),
            Command::ScriptLoad(script) => self.execute_script_load(script),
            Command::ScriptExists(sha1s) => self.execute_script_exists(sha1s),
            Command::ScriptFlush => self.execute_script_flush(),

            // ACL commands
            Command::Auth { .. } => self.execute_auth(),
            Command::AclWhoami => self.execute_acl_whoami(),
            Command::AclList => self.execute_acl_list(),
            Command::AclUsers => self.execute_acl_users(),
            Command::AclGetUser { username } => self.execute_acl_getuser(username),
            Command::AclSetUser { .. } => self.execute_acl_setuser(),
            Command::AclDelUser { .. } => self.execute_acl_deluser(),
            Command::AclCat { category } => self.execute_acl_cat(category.as_deref()),
            Command::AclGenPass { bits } => self.execute_acl_genpass(*bits),

            // Config commands
            Command::ConfigGet(pattern) => self.execute_config_get(pattern),
            Command::ConfigSet(param, value) => self.execute_config_set(param, value),
            Command::ConfigResetStat => self.execute_config_resetstat(),

            // Select command
            Command::Select(_db) => {
                debug_assert!(*_db <= 15, "Precondition: database index must be 0-15");
                RespValue::ok()
            }

            // Echo command
            Command::Echo(msg) => {
                let resp = RespValue::BulkString(Some(msg.as_bytes().to_vec()));
                debug_assert!(
                    matches!(&resp, RespValue::BulkString(Some(data)) if data == msg.as_bytes()),
                    "Postcondition: ECHO response must equal input"
                );
                resp
            }

            // Function commands (stubs for Tcl harness)
            Command::FunctionFlush => RespValue::ok(),

            // Command introspection (stubs for Tcl harness)
            Command::CommandCommand => {
                // Return empty array — tests just need it to not crash
                RespValue::Array(Some(vec![]))
            }
            Command::CommandCount => {
                // Return a plausible count
                RespValue::Integer(200)
            }

            // Client commands (stubs)
            Command::ClientSetName(_) => RespValue::ok(),
            Command::ClientGetName => RespValue::BulkString(None),
            Command::ClientId => RespValue::Integer(1),
            Command::ClientInfo => {
                RespValue::BulkString(Some(b"id=1 fd=5 name= db=0 flags=N".to_vec()))
            }

            // Object commands
            Command::ObjectHelp => {
                let help = vec![
                    RespValue::BulkString(Some(b"OBJECT ENCODING <key>".to_vec())),
                    RespValue::BulkString(Some(b"OBJECT REFCOUNT <key>".to_vec())),
                    RespValue::BulkString(Some(b"OBJECT IDLETIME <key>".to_vec())),
                    RespValue::BulkString(Some(b"OBJECT FREQ <key>".to_vec())),
                    RespValue::BulkString(Some(b"OBJECT HELP".to_vec())),
                ];
                RespValue::Array(Some(help))
            }
            Command::ObjectEncoding(key) => {
                match self.get_value(key) {
                    Some(Value::String(s)) => {
                        if std::str::from_utf8(s.as_bytes())
                            .unwrap_or("")
                            .parse::<i64>()
                            .is_ok()
                        {
                            RespValue::BulkString(Some(b"int".to_vec()))
                        } else if s.as_bytes().len() <= 44 {
                            RespValue::BulkString(Some(b"embstr".to_vec()))
                        } else {
                            RespValue::BulkString(Some(b"raw".to_vec()))
                        }
                    }
                    Some(Value::List(l)) => {
                        if l.len() <= 128 {
                            RespValue::BulkString(Some(b"listpack".to_vec()))
                        } else {
                            RespValue::BulkString(Some(b"quicklist".to_vec()))
                        }
                    }
                    Some(Value::Set(s)) => {
                        if s.len() <= 128 {
                            RespValue::BulkString(Some(b"listpack".to_vec()))
                        } else {
                            RespValue::BulkString(Some(b"hashtable".to_vec()))
                        }
                    }
                    Some(Value::Hash(h)) => {
                        if h.len() <= 128 {
                            RespValue::BulkString(Some(b"listpack".to_vec()))
                        } else {
                            RespValue::BulkString(Some(b"hashtable".to_vec()))
                        }
                    }
                    Some(Value::SortedSet(z)) => {
                        if z.len() <= 128 {
                            RespValue::BulkString(Some(b"listpack".to_vec()))
                        } else {
                            RespValue::BulkString(Some(b"skiplist".to_vec()))
                        }
                    }
                    None | Some(Value::Null) => RespValue::err("ERR no such key"),
                }
            }
            Command::ObjectRefCount(key) => {
                if self.get_value(key).is_some() {
                    RespValue::Integer(1)
                } else {
                    RespValue::err("ERR no such key")
                }
            }
            Command::ObjectIdleTime(key) => {
                if self.get_value(key).is_some() {
                    RespValue::Integer(0)
                } else {
                    RespValue::err("ERR no such key")
                }
            }
            Command::ObjectFreq(key) => {
                if self.get_value(key).is_some() {
                    RespValue::Integer(0)
                } else {
                    RespValue::err("ERR no such key")
                }
            }

            // Debug commands (stubs)
            Command::DebugSleep(_) => RespValue::ok(),
            Command::DebugSet(_, _) => RespValue::ok(),
            Command::DebugObject(key) => {
                match self.get_value(key) {
                    Some(_) => {
                        RespValue::BulkString(Some(
                            b"Value at:0x0 refcount:1 encoding:raw serializedlength:0 lru:0 lru_seconds_idle:0 type:string".to_vec()
                        ))
                    }
                    None => RespValue::err("ERR no such key"),
                }
            }

            // RANDOMKEY
            Command::RandomKey => {
                // Return a random non-expired key, or nil
                let key = self.data.keys().find(|k| !self.is_expired(k)).cloned();
                match key {
                    Some(k) => RespValue::BulkString(Some(k.into_bytes())),
                    None => RespValue::BulkString(None),
                }
            }

            // RENAME
            Command::Rename(src, dst) => {
                if self.is_expired(src) || !self.data.contains_key(src) {
                    return RespValue::err("ERR no such key");
                }
                let val = self.data.remove(src).expect("checked key exists above");
                let exp = self.expirations.remove(src);
                self.data.insert(dst.clone(), val);
                if let Some(exp_time) = exp {
                    self.expirations.insert(dst.clone(), exp_time);
                } else {
                    self.expirations.remove(dst);
                }
                self.access_times.remove(src);
                self.access_times.insert(dst.clone(), self.current_time);
                #[cfg(debug_assertions)]
                {
                    debug_assert!(self.data.contains_key(dst.as_str()), "Postcondition: RENAME dst must exist");
                    debug_assert!(!self.data.contains_key(src.as_str()), "Postcondition: RENAME src must not exist");
                }
                RespValue::ok()
            }
            Command::RenameNx(src, dst) => {
                if self.is_expired(src) || !self.data.contains_key(src) {
                    return RespValue::err("ERR no such key");
                }
                if !self.is_expired(dst) && self.data.contains_key(dst) {
                    return RespValue::Integer(0);
                }
                let val = self.data.remove(src).expect("checked key exists above");
                let exp = self.expirations.remove(src);
                self.data.insert(dst.clone(), val);
                if let Some(exp_time) = exp {
                    self.expirations.insert(dst.clone(), exp_time);
                } else {
                    self.expirations.remove(dst);
                }
                self.access_times.remove(src);
                self.access_times.insert(dst.clone(), self.current_time);
                #[cfg(debug_assertions)]
                {
                    debug_assert!(self.data.contains_key(dst.as_str()), "Postcondition: RENAMENX dst must exist");
                    debug_assert!(!self.data.contains_key(src.as_str()), "Postcondition: RENAMENX src must not exist");
                }
                RespValue::Integer(1)
            }

            // WAIT - no replicas in simulation
            Command::Wait(_, _) => RespValue::Integer(0),

            // SORT - minimal stub (returns sorted elements, stores if STORE)
            Command::Sort { key, store } => {
                let members: Vec<SDS> = match self.get_value(key) {
                    Some(Value::List(l)) => {
                        l.range(0, l.len() as isize - 1)
                    }
                    Some(Value::Set(s)) => s.members(),
                    None => vec![],
                    Some(_) => {
                        return RespValue::err(
                            "WRONGTYPE Operation against a key holding the wrong kind of value",
                        )
                    }
                };
                // Sort lexicographically (basic sort; Redis sorts numerically by default but this is a stub)
                let mut sorted = members;
                sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                let items: Vec<RespValue> = sorted
                    .iter()
                    .map(|s| RespValue::BulkString(Some(s.as_bytes().to_vec())))
                    .collect();
                if let Some(dest) = store {
                    let count = items.len() as i64;
                    if sorted.is_empty() {
                        self.data.remove(dest);
                        self.expirations.remove(dest);
                    } else {
                        use crate::redis::data::RedisList;
                        let mut list = RedisList::new();
                        for s in &sorted {
                            list.rpush(s.clone());
                        }
                        self.data.insert(dest.clone(), Value::List(list));
                        self.expirations.remove(dest);
                    }
                    RespValue::Integer(count)
                } else {
                    RespValue::Array(Some(items))
                }
            }

            // Unknown
            Command::Unknown(cmd) => RespValue::err(format!("ERR unknown command '{}'", cmd)),
        }
    }

    // Server command implementations
    fn execute_info(&self) -> RespValue {
        let info = format!(
            "# Server\r\n\
             redis_mode:simulator\r\n\
             \r\n\
             # Stats\r\n\
             total_commands_processed:{}\r\n\
             total_keys:{}\r\n\
             keys_with_expiration:{}\r\n\
             current_time_ms:{}\r\n",
            self.commands_processed,
            self.data.len(),
            self.expirations.len(),
            self.current_time.as_millis()
        );
        RespValue::BulkString(Some(info.into_bytes()))
    }

    fn execute_dbsize(&self) -> RespValue {
        let valid_keys = self.data.keys().filter(|k| !self.is_expired(k)).count();
        RespValue::Integer(valid_keys as i64)
    }

    /// Helper for glob pattern matching
    pub(crate) fn matches_glob_pattern(&self, key: &str, pattern: &str) -> bool {
        let key_bytes = key.as_bytes();
        let pattern_bytes = pattern.as_bytes();
        self.glob_match(key_bytes, pattern_bytes, 0, 0)
    }

    fn glob_match(&self, key: &[u8], pattern: &[u8], k_idx: usize, p_idx: usize) -> bool {
        if p_idx >= pattern.len() {
            return k_idx >= key.len();
        }

        let p_char = pattern[p_idx];

        if p_char == b'*' {
            // Try matching zero or more characters
            for i in k_idx..=key.len() {
                if self.glob_match(key, pattern, i, p_idx + 1) {
                    return true;
                }
            }
            false
        } else if p_char == b'?' {
            if k_idx >= key.len() {
                false
            } else {
                self.glob_match(key, pattern, k_idx + 1, p_idx + 1)
            }
        } else if p_char == b'[' {
            // Character class
            let mut bracket_end = p_idx + 1;
            while bracket_end < pattern.len() && pattern[bracket_end] != b']' {
                bracket_end += 1;
            }
            if bracket_end >= pattern.len() {
                return false;
            }

            let char_set = &pattern[p_idx + 1..bracket_end];
            let (negate, char_set) = if !char_set.is_empty() && char_set[0] == b'^' {
                (true, &char_set[1..])
            } else {
                (false, char_set)
            };

            if k_idx >= key.len() {
                return false;
            }

            // Handle ranges like [a-z]
            let mut chars_to_check: Vec<u8> = Vec::new();
            let mut i = 0;
            while i < char_set.len() {
                if i + 2 < char_set.len() && char_set[i + 1] == b'-' {
                    let start = char_set[i];
                    let end = char_set[i + 2];
                    for c in start..=end {
                        chars_to_check.push(c);
                    }
                    i += 3;
                } else {
                    chars_to_check.push(char_set[i]);
                    i += 1;
                }
            }

            let chars_to_check = if chars_to_check.is_empty() {
                char_set.to_vec()
            } else {
                chars_to_check
            };

            let mut matched = false;
            for c in &chars_to_check {
                if *c == key[k_idx] {
                    matched = true;
                    break;
                }
            }

            if negate {
                matched = !matched;
            }

            if matched {
                self.glob_match(key, pattern, k_idx + 1, bracket_end + 1)
            } else {
                false
            }
        } else {
            if k_idx >= key.len() || key[k_idx] != p_char {
                false
            } else {
                self.glob_match(key, pattern, k_idx + 1, p_idx + 1)
            }
        }
    }
}

impl Default for CommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}
