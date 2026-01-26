//! Script command implementations for CommandExecutor.
//!
//! Handles: EVAL, EVALSHA, SCRIPT LOAD, SCRIPT EXISTS, SCRIPT FLUSH

use super::CommandExecutor;
use crate::redis::command::Command;
use crate::redis::data::SDS;
use crate::redis::resp::RespValue;

impl CommandExecutor {
    // Script cache helper methods

    /// Cache a script and return its SHA1
    pub(crate) fn cache_script_internal(&mut self, script: &str) -> String {
        if let Some(ref shared) = self.shared_script_cache {
            shared.cache_script(script)
        } else {
            self.script_cache.cache_script(script)
        }
    }

    /// Get a script by SHA1
    pub(crate) fn get_script_internal(&self, sha1: &str) -> Option<String> {
        if let Some(ref shared) = self.shared_script_cache {
            shared.get_script(sha1)
        } else {
            self.script_cache.get_script(sha1).cloned()
        }
    }

    /// Check if a script exists
    pub(crate) fn has_script_internal(&self, sha1: &str) -> bool {
        if let Some(ref shared) = self.shared_script_cache {
            shared.has_script(sha1)
        } else {
            self.script_cache.has_script(sha1)
        }
    }

    /// Flush all scripts
    pub(crate) fn flush_scripts_internal(&mut self) {
        if let Some(ref shared) = self.shared_script_cache {
            shared.flush()
        } else {
            self.script_cache.flush()
        }
    }

    pub(super) fn execute_eval(
        &mut self,
        script: &str,
        keys: &[String],
        args: &[SDS],
    ) -> RespValue {
        #[cfg(feature = "lua")]
        {
            self.execute_lua_script(script, keys, args)
        }
        #[cfg(not(feature = "lua"))]
        {
            let _ = (script, keys, args);
            RespValue::err("ERR Lua scripting not compiled in")
        }
    }

    pub(super) fn execute_evalsha(
        &mut self,
        sha1: &str,
        keys: &[String],
        args: &[SDS],
    ) -> RespValue {
        #[cfg(feature = "lua")]
        {
            // Look up script in cache (uses shared cache if available)
            match self.get_script_internal(sha1) {
                Some(script) => {
                    let script = script.clone();
                    self.execute_lua_script(&script, keys, args)
                }
                None => RespValue::err("NOSCRIPT No matching script. Please use EVAL."),
            }
        }
        #[cfg(not(feature = "lua"))]
        {
            let _ = (sha1, keys, args);
            RespValue::err("ERR Lua scripting not compiled in")
        }
    }

    pub(super) fn execute_script_load(&mut self, script: &str) -> RespValue {
        #[cfg(feature = "lua")]
        {
            let sha1 = self.cache_script_internal(script);
            RespValue::BulkString(Some(sha1.into_bytes()))
        }
        #[cfg(not(feature = "lua"))]
        {
            let _ = script;
            RespValue::err("ERR Lua scripting not compiled in")
        }
    }

    pub(super) fn execute_script_exists(&self, sha1s: &[String]) -> RespValue {
        #[cfg(feature = "lua")]
        {
            let results: Vec<RespValue> = sha1s
                .iter()
                .map(|sha1| {
                    if self.has_script_internal(sha1) {
                        RespValue::Integer(1)
                    } else {
                        RespValue::Integer(0)
                    }
                })
                .collect();
            RespValue::Array(Some(results))
        }
        #[cfg(not(feature = "lua"))]
        {
            let _ = sha1s;
            RespValue::err("ERR Lua scripting not compiled in")
        }
    }

    pub(super) fn execute_script_flush(&mut self) -> RespValue {
        #[cfg(feature = "lua")]
        {
            self.flush_scripts_internal();
            RespValue::simple("OK")
        }
        #[cfg(not(feature = "lua"))]
        {
            RespValue::err("ERR Lua scripting not compiled in")
        }
    }

    /// Execute a Lua script with KEYS and ARGV
    #[cfg(feature = "lua")]
    pub(crate) fn execute_lua_script(
        &mut self,
        script: &str,
        keys: &[String],
        args: &[SDS],
    ) -> RespValue {
        use mlua::{Lua, MultiValue, Result as LuaResult, Value as LuaValue};
        use std::cell::RefCell;

        // TigerStyle: Preconditions
        debug_assert!(!script.is_empty(), "Precondition: script must not be empty");

        // Cache the script for EVALSHA (uses shared cache if available)
        #[allow(unused_variables)]
        let script_sha = self.cache_script_internal(script);

        // Create a new Lua instance for this execution
        let lua = Lua::new();

        // DST: Seed math.random deterministically using current_time
        let seed = self.current_time.as_millis();
        if let Err(e) = lua.load(format!("math.randomseed({})", seed)).exec() {
            return RespValue::err(format!("ERR Failed to seed RNG: {}", e));
        }

        // Sandbox: Remove dangerous/non-deterministic functions
        if let Err(e) = (|| -> LuaResult<()> {
            lua.globals().set("os", LuaValue::Nil)?;
            lua.globals().set("io", LuaValue::Nil)?;
            lua.globals().set("loadfile", LuaValue::Nil)?;
            lua.globals().set("dofile", LuaValue::Nil)?;
            lua.globals().set("debug", LuaValue::Nil)?;
            Ok(())
        })() {
            return RespValue::err(format!("ERR Lua sandbox error: {}", e));
        }

        // Create KEYS table
        if let Err(e) = (|| -> LuaResult<()> {
            let keys_table = lua.create_table()?;
            for (i, key) in keys.iter().enumerate() {
                keys_table.set(i + 1, key.as_str())?;
            }
            lua.globals().set("KEYS", keys_table)?;
            Ok(())
        })() {
            return RespValue::err(format!("ERR Failed to set KEYS: {}", e));
        }

        // Create ARGV table - use binary-safe Lua strings
        if let Err(e) = (|| -> LuaResult<()> {
            let argv_table = lua.create_table()?;
            for (i, arg) in args.iter().enumerate() {
                let lua_str = lua.create_string(arg.as_bytes())?;
                argv_table.set(i + 1, lua_str)?;
            }
            lua.globals().set("ARGV", argv_table)?;
            Ok(())
        })() {
            return RespValue::err(format!("ERR Failed to set ARGV: {}", e));
        }

        // Use RefCell to allow mutable borrow from within Lua callbacks
        let executor = RefCell::new(&mut *self);

        // Execute script within a scope that allows borrowing executor
        let result = lua.scope(|scope| {
            // Create redis.call - executes command immediately, propagates errors
            let executor_call = &executor;
            let call_fn = scope.create_function_mut(|lua, args: MultiValue| {
                let cmd_parts = Self::parse_multivalue_to_bytes(args)?;
                if cmd_parts.is_empty() {
                    return Err(mlua::Error::RuntimeError(
                        "redis.call requires at least one argument".to_string(),
                    ));
                }

                let mut exec = executor_call.borrow_mut();
                match exec.parse_lua_command_bytes(&cmd_parts) {
                    Ok(cmd) => {
                        let resp = exec.execute(&cmd);
                        // redis.call propagates errors
                        if let RespValue::Error(e) = &resp {
                            return Err(mlua::Error::RuntimeError(e.to_string()));
                        }
                        Self::resp_to_lua_value(lua, resp)
                    }
                    Err(e) => Err(mlua::Error::RuntimeError(e)),
                }
            })?;

            // Create redis.pcall - executes command immediately, returns errors as tables
            let executor_pcall = &executor;
            let pcall_fn = scope.create_function_mut(|lua, args: MultiValue| {
                let cmd_parts = Self::parse_multivalue_to_bytes(args)?;
                if cmd_parts.is_empty() {
                    return Err(mlua::Error::RuntimeError(
                        "redis.pcall requires at least one argument".to_string(),
                    ));
                }

                let mut exec = executor_pcall.borrow_mut();
                match exec.parse_lua_command_bytes(&cmd_parts) {
                    Ok(cmd) => {
                        let resp = exec.execute(&cmd);
                        // redis.pcall returns errors as {err = "message"} tables
                        if let RespValue::Error(e) = &resp {
                            let err_table = lua.create_table()?;
                            err_table.set("err", e.as_ref())?;
                            return Ok(LuaValue::Table(err_table));
                        }
                        Self::resp_to_lua_value(lua, resp)
                    }
                    Err(e) => {
                        let err_table = lua.create_table()?;
                        err_table.set("err", e.as_str())?;
                        Ok(LuaValue::Table(err_table))
                    }
                }
            })?;

            // Set up redis table with call/pcall
            let redis_table = lua.create_table()?;
            redis_table.set("call", call_fn)?;
            redis_table.set("pcall", pcall_fn)?;
            lua.globals().set("redis", redis_table)?;

            // Execute the script
            lua.load(script).eval::<LuaValue>()
        });

        // Convert result
        match result {
            Ok(lua_value) => self.lua_to_resp(&lua, lua_value),
            Err(e) => RespValue::err(format!("ERR {}", e)),
        }
    }

    /// Parse MultiValue arguments to bytes for redis.call/pcall
    #[cfg(feature = "lua")]
    fn parse_multivalue_to_bytes(args: mlua::MultiValue) -> mlua::Result<Vec<Vec<u8>>> {
        use mlua::Value as LuaValue;

        let mut cmd_parts = Vec::new();
        for arg in args {
            match arg {
                LuaValue::String(s) => cmd_parts.push(s.as_bytes().to_vec()),
                LuaValue::Integer(i) => cmd_parts.push(i.to_string().into_bytes()),
                LuaValue::Number(n) => cmd_parts.push(n.to_string().into_bytes()),
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "Invalid argument type for redis command".to_string(),
                    ))
                }
            }
        }
        Ok(cmd_parts)
    }

    /// Convert RespValue to Lua Value
    #[cfg(feature = "lua")]
    fn resp_to_lua_value(lua: &mlua::Lua, resp: RespValue) -> mlua::Result<mlua::Value> {
        use mlua::Value as LuaValue;

        Ok(match resp {
            RespValue::SimpleString(s) => {
                let t = lua.create_table()?;
                t.set("ok", s.as_ref())?;
                LuaValue::Table(t)
            }
            RespValue::Error(e) => {
                let t = lua.create_table()?;
                t.set("err", e.as_ref())?;
                LuaValue::Table(t)
            }
            RespValue::Integer(i) => LuaValue::Integer(i),
            RespValue::BulkString(Some(bytes)) => LuaValue::String(lua.create_string(&bytes)?),
            RespValue::BulkString(None) => LuaValue::Nil,
            RespValue::Array(Some(elements)) => {
                let t = lua.create_table()?;
                for (i, elem) in elements.into_iter().enumerate() {
                    let lua_val = Self::resp_to_lua_value(lua, elem)?;
                    t.set(i + 1, lua_val)?;
                }
                LuaValue::Table(t)
            }
            RespValue::Array(None) => LuaValue::Nil,
        })
    }

    /// Parse a command from Lua script arguments (bytes version)
    #[cfg(feature = "lua")]
    fn parse_lua_command_bytes(&self, parts: &[Vec<u8>]) -> Result<Command, String> {
        if parts.is_empty() {
            return Err("Empty command".to_string());
        }

        let cmd_name = String::from_utf8_lossy(&parts[0]).to_uppercase();
        let args = &parts[1..];
        let to_string = |b: &[u8]| String::from_utf8_lossy(b).to_string();
        let to_sds = |b: &[u8]| SDS::new(b.to_vec());

        match cmd_name.as_str() {
            "GET" => {
                if args.len() != 1 {
                    return Err("GET requires 1 argument".to_string());
                }
                Ok(Command::Get(to_string(&args[0])))
            }
            "SET" => {
                if args.len() < 2 {
                    return Err("SET requires at least 2 arguments".to_string());
                }
                let key = to_string(&args[0]);
                let value = to_sds(&args[1]);
                let mut ex = None;
                let mut px = None;
                let mut nx = false;
                let mut xx = false;
                let mut get = false;

                let mut i = 2;
                while i < args.len() {
                    let opt = to_string(&args[i]).to_uppercase();
                    match opt.as_str() {
                        "NX" => nx = true,
                        "XX" => xx = true,
                        "GET" => get = true,
                        "EX" => {
                            i += 1;
                            if i >= args.len() {
                                return Err("SET EX requires value".to_string());
                            }
                            ex = Some(
                                to_string(&args[i])
                                    .parse()
                                    .map_err(|_| "SET EX must be integer")?,
                            );
                        }
                        "PX" => {
                            i += 1;
                            if i >= args.len() {
                                return Err("SET PX requires value".to_string());
                            }
                            px = Some(
                                to_string(&args[i])
                                    .parse()
                                    .map_err(|_| "SET PX must be integer")?,
                            );
                        }
                        _ => return Err(format!("Unknown SET option: {}", opt)),
                    }
                    i += 1;
                }
                Ok(Command::Set {
                    key,
                    value,
                    ex,
                    px,
                    nx,
                    xx,
                    get,
                })
            }
            "DEL" => {
                if args.is_empty() {
                    return Err("DEL requires at least 1 argument".to_string());
                }
                Ok(Command::Del(args.iter().map(|a| to_string(a)).collect()))
            }
            "INCR" => {
                if args.len() != 1 {
                    return Err("INCR requires 1 argument".to_string());
                }
                Ok(Command::Incr(to_string(&args[0])))
            }
            "DECR" => {
                if args.len() != 1 {
                    return Err("DECR requires 1 argument".to_string());
                }
                Ok(Command::Decr(to_string(&args[0])))
            }
            "INCRBY" => {
                if args.len() != 2 {
                    return Err("INCRBY requires 2 arguments".to_string());
                }
                let incr: i64 = to_string(&args[1])
                    .parse()
                    .map_err(|_| "INCRBY increment must be integer")?;
                Ok(Command::IncrBy(to_string(&args[0]), incr))
            }
            "HGET" => {
                if args.len() != 2 {
                    return Err("HGET requires 2 arguments".to_string());
                }
                Ok(Command::HGet(to_string(&args[0]), to_sds(&args[1])))
            }
            "HSET" => {
                if args.len() < 3 || args.len() % 2 == 0 {
                    return Err("HSET requires key and field-value pairs".to_string());
                }
                let key = to_string(&args[0]);
                let pairs: Vec<(SDS, SDS)> = args[1..]
                    .chunks(2)
                    .map(|chunk| (to_sds(&chunk[0]), to_sds(&chunk[1])))
                    .collect();
                Ok(Command::HSet(key, pairs))
            }
            "HDEL" => {
                if args.len() < 2 {
                    return Err("HDEL requires key and at least 1 field".to_string());
                }
                let key = to_string(&args[0]);
                let fields: Vec<SDS> = args[1..].iter().map(|a| to_sds(a)).collect();
                Ok(Command::HDel(key, fields))
            }
            "LPUSH" => {
                if args.len() < 2 {
                    return Err("LPUSH requires key and at least 1 value".to_string());
                }
                let key = to_string(&args[0]);
                let values: Vec<SDS> = args[1..].iter().map(|a| to_sds(a)).collect();
                Ok(Command::LPush(key, values))
            }
            "RPUSH" => {
                if args.len() < 2 {
                    return Err("RPUSH requires key and at least 1 value".to_string());
                }
                let key = to_string(&args[0]);
                let values: Vec<SDS> = args[1..].iter().map(|a| to_sds(a)).collect();
                Ok(Command::RPush(key, values))
            }
            "LPOP" => {
                if args.len() != 1 {
                    return Err("LPOP requires 1 argument".to_string());
                }
                Ok(Command::LPop(to_string(&args[0])))
            }
            "RPOP" => {
                if args.len() != 1 {
                    return Err("RPOP requires 1 argument".to_string());
                }
                Ok(Command::RPop(to_string(&args[0])))
            }
            "LLEN" => {
                if args.len() != 1 {
                    return Err("LLEN requires 1 argument".to_string());
                }
                Ok(Command::LLen(to_string(&args[0])))
            }
            "SADD" => {
                if args.len() < 2 {
                    return Err("SADD requires key and at least 1 member".to_string());
                }
                let key = to_string(&args[0]);
                let members: Vec<SDS> = args[1..].iter().map(|a| to_sds(a)).collect();
                Ok(Command::SAdd(key, members))
            }
            "SREM" => {
                if args.len() < 2 {
                    return Err("SREM requires key and at least 1 member".to_string());
                }
                let key = to_string(&args[0]);
                let members: Vec<SDS> = args[1..].iter().map(|a| to_sds(a)).collect();
                Ok(Command::SRem(key, members))
            }
            "SMEMBERS" => {
                if args.len() != 1 {
                    return Err("SMEMBERS requires 1 argument".to_string());
                }
                Ok(Command::SMembers(to_string(&args[0])))
            }
            "EXISTS" => {
                if args.is_empty() {
                    return Err("EXISTS requires at least 1 argument".to_string());
                }
                Ok(Command::Exists(args.iter().map(|a| to_string(a)).collect()))
            }
            "EXPIRE" => {
                if args.len() != 2 {
                    return Err("EXPIRE requires 2 arguments".to_string());
                }
                let seconds: i64 = to_string(&args[1])
                    .parse()
                    .map_err(|_| "EXPIRE seconds must be integer")?;
                Ok(Command::Expire(to_string(&args[0]), seconds))
            }
            "TTL" => {
                if args.len() != 1 {
                    return Err("TTL requires 1 argument".to_string());
                }
                Ok(Command::Ttl(to_string(&args[0])))
            }
            "TYPE" => {
                if args.len() != 1 {
                    return Err("TYPE requires 1 argument".to_string());
                }
                Ok(Command::TypeOf(to_string(&args[0])))
            }
            "HINCRBY" => {
                if args.len() != 3 {
                    return Err("HINCRBY requires 3 arguments".to_string());
                }
                let incr: i64 = to_string(&args[2])
                    .parse()
                    .map_err(|_| "HINCRBY increment must be integer")?;
                Ok(Command::HIncrBy(
                    to_string(&args[0]),
                    to_sds(&args[1]),
                    incr,
                ))
            }
            "LRANGE" => {
                if args.len() != 3 {
                    return Err("LRANGE requires 3 arguments".to_string());
                }
                let start: isize = to_string(&args[1])
                    .parse()
                    .map_err(|_| "LRANGE start must be integer")?;
                let stop: isize = to_string(&args[2])
                    .parse()
                    .map_err(|_| "LRANGE stop must be integer")?;
                Ok(Command::LRange(to_string(&args[0]), start, stop))
            }
            "RPOPLPUSH" => {
                if args.len() != 2 {
                    return Err("RPOPLPUSH requires 2 arguments".to_string());
                }
                Ok(Command::RPopLPush(to_string(&args[0]), to_string(&args[1])))
            }
            "LMOVE" => {
                if args.len() != 4 {
                    return Err("LMOVE requires 4 arguments".to_string());
                }
                let wherefrom = to_string(&args[2]).to_uppercase();
                let whereto = to_string(&args[3]).to_uppercase();
                if wherefrom != "LEFT" && wherefrom != "RIGHT" {
                    return Err("LMOVE wherefrom must be LEFT or RIGHT".to_string());
                }
                if whereto != "LEFT" && whereto != "RIGHT" {
                    return Err("LMOVE whereto must be LEFT or RIGHT".to_string());
                }
                Ok(Command::LMove {
                    source: to_string(&args[0]),
                    dest: to_string(&args[1]),
                    wherefrom,
                    whereto,
                })
            }
            "HGETALL" => {
                if args.len() != 1 {
                    return Err("HGETALL requires 1 argument".to_string());
                }
                Ok(Command::HGetAll(to_string(&args[0])))
            }
            "SISMEMBER" => {
                if args.len() != 2 {
                    return Err("SISMEMBER requires 2 arguments".to_string());
                }
                Ok(Command::SIsMember(to_string(&args[0]), to_sds(&args[1])))
            }
            "ZADD" => {
                if args.len() < 3 {
                    return Err("ZADD requires key and score-member pairs".to_string());
                }
                let key = to_string(&args[0]);
                let mut nx = false;
                let mut xx = false;
                let mut gt = false;
                let mut lt = false;
                let mut ch = false;
                let mut i = 1;

                while i < args.len() {
                    let opt = to_string(&args[i]).to_uppercase();
                    match opt.as_str() {
                        "NX" => {
                            nx = true;
                            i += 1;
                        }
                        "XX" => {
                            xx = true;
                            i += 1;
                        }
                        "GT" => {
                            gt = true;
                            i += 1;
                        }
                        "LT" => {
                            lt = true;
                            i += 1;
                        }
                        "CH" => {
                            ch = true;
                            i += 1;
                        }
                        _ => break,
                    }
                }

                if (args.len() - i) % 2 != 0 || i >= args.len() {
                    return Err("ZADD requires score-member pairs".to_string());
                }

                let mut pairs: Vec<(f64, SDS)> = Vec::new();
                while i < args.len() {
                    let score: f64 = to_string(&args[i])
                        .parse()
                        .map_err(|_| "ZADD score must be a number")?;
                    pairs.push((score, to_sds(&args[i + 1])));
                    i += 2;
                }
                Ok(Command::ZAdd {
                    key,
                    pairs,
                    nx,
                    xx,
                    gt,
                    lt,
                    ch,
                })
            }
            "ZREM" => {
                if args.len() < 2 {
                    return Err("ZREM requires key and at least 1 member".to_string());
                }
                let key = to_string(&args[0]);
                let members: Vec<SDS> = args[1..].iter().map(|a| to_sds(a)).collect();
                Ok(Command::ZRem(key, members))
            }
            "ZRANGE" => {
                if args.len() != 3 {
                    return Err("ZRANGE requires 3 arguments".to_string());
                }
                let start: isize = to_string(&args[1])
                    .parse()
                    .map_err(|_| "ZRANGE start must be integer")?;
                let stop: isize = to_string(&args[2])
                    .parse()
                    .map_err(|_| "ZRANGE stop must be integer")?;
                Ok(Command::ZRange(to_string(&args[0]), start, stop))
            }
            "ZSCORE" => {
                if args.len() != 2 {
                    return Err("ZSCORE requires 2 arguments".to_string());
                }
                Ok(Command::ZScore(to_string(&args[0]), to_sds(&args[1])))
            }
            "ZCARD" => {
                if args.len() != 1 {
                    return Err("ZCARD requires 1 argument".to_string());
                }
                Ok(Command::ZCard(to_string(&args[0])))
            }
            "ZCOUNT" => {
                if args.len() != 3 {
                    return Err("ZCOUNT requires 3 arguments".to_string());
                }
                Ok(Command::ZCount(
                    to_string(&args[0]),
                    to_string(&args[1]),
                    to_string(&args[2]),
                ))
            }
            "ZRANGEBYSCORE" => {
                if args.len() < 3 {
                    return Err("ZRANGEBYSCORE requires at least 3 arguments".to_string());
                }
                let key = to_string(&args[0]);
                let min = to_string(&args[1]);
                let max = to_string(&args[2]);
                let mut with_scores = false;
                let mut limit = None;

                let mut i = 3;
                while i < args.len() {
                    let opt = to_string(&args[i]).to_uppercase();
                    match opt.as_str() {
                        "WITHSCORES" => with_scores = true,
                        "LIMIT" => {
                            if i + 2 >= args.len() {
                                return Err(
                                    "ZRANGEBYSCORE LIMIT requires offset and count".to_string()
                                );
                            }
                            let offset: isize = to_string(&args[i + 1])
                                .parse()
                                .map_err(|_| "ZRANGEBYSCORE LIMIT offset must be integer")?;
                            let count: usize = to_string(&args[i + 2])
                                .parse()
                                .map_err(|_| "ZRANGEBYSCORE LIMIT count must be integer")?;
                            limit = Some((offset, count));
                            i += 2;
                        }
                        _ => return Err(format!("Unknown ZRANGEBYSCORE option: {}", opt)),
                    }
                    i += 1;
                }
                Ok(Command::ZRangeByScore {
                    key,
                    min,
                    max,
                    with_scores,
                    limit,
                })
            }
            _ => Err(format!(
                "ERR Unknown Redis command '{}' called from Lua",
                cmd_name
            )),
        }
    }

    /// Convert Lua value to RespValue
    #[cfg(feature = "lua")]
    fn lua_to_resp(&self, lua: &mlua::Lua, value: mlua::Value) -> RespValue {
        use mlua::Value as LuaValue;

        match value {
            LuaValue::Nil => RespValue::BulkString(None),
            LuaValue::Boolean(b) => {
                if b {
                    RespValue::Integer(1)
                } else {
                    RespValue::BulkString(None)
                }
            }
            LuaValue::Integer(i) => RespValue::Integer(i),
            LuaValue::Number(n) => RespValue::BulkString(Some(n.to_string().into_bytes())),
            LuaValue::String(s) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
            LuaValue::Table(t) => {
                // Check for Redis error/ok convention first
                if let Ok(err) = t.get::<String>("err") {
                    return RespValue::err(err);
                }
                if let Ok(ok) = t.get::<String>("ok") {
                    return RespValue::simple(ok);
                }

                // Otherwise treat as array
                let mut elements = Vec::new();
                let mut i = 1;
                loop {
                    match t.get::<mlua::Value>(i) {
                        Ok(val) if !matches!(val, LuaValue::Nil) => {
                            elements.push(self.lua_to_resp(lua, val));
                            i += 1;
                        }
                        _ => break,
                    }
                }
                RespValue::Array(Some(elements))
            }
            _ => RespValue::BulkString(None),
        }
    }
}
