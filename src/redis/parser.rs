//! RESP protocol parser for Redis commands.
//!
//! This module implements the `from_resp` method on `Command` which parses
//! `RespValue` into typed `Command` instances.
//!
//! # Architecture Note
//!
//! This file exceeds the 500-line guideline because it contains a single large
//! match statement that maps RESP protocol commands to typed Command variants.
//! Splitting this match statement would reduce readability without meaningful
//! benefit. See DEV-001 for file size deviation tracking.

use super::command::Command;
use super::data::SDS;
use super::resp::RespValue;

impl Command {
    /// Parse a RESP protocol value into a Command.
    ///
    /// # Errors
    ///
    /// Returns an error string if:
    /// - The input is not an array
    /// - The command name is not a bulk string
    /// - Required arguments are missing
    /// - Argument types don't match expected types
    pub fn from_resp(value: &RespValue) -> Result<Command, String> {
        match value {
            RespValue::Array(Some(elements)) if !elements.is_empty() => {
                let cmd_name = match &elements[0] {
                    RespValue::BulkString(Some(data)) => {
                        String::from_utf8_lossy(data).to_uppercase()
                    }
                    _ => return Err("Invalid command format".to_string()),
                };

                match cmd_name.as_str() {
                    "PING" => {
                        let msg = if elements.len() > 1 {
                            Some(Self::extract_sds(&elements[1])?)
                        } else {
                            None
                        };
                        Ok(Command::Ping(msg))
                    }
                    "INFO" => Ok(Command::Info),
                    "TIME" => Ok(Command::Time),
                    "DBSIZE" => Ok(Command::DbSize),
                    "CONFIG" => {
                        if elements.len() < 2 {
                            return Err("ERR wrong number of arguments for 'config' command".to_string());
                        }
                        let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "GET" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'config|get' command".to_string());
                                }
                                let pattern = Self::extract_string(&elements[2])?;
                                Ok(Command::ConfigGet(pattern))
                            }
                            "SET" => {
                                if elements.len() != 4 {
                                    return Err("ERR wrong number of arguments for 'config|set' command".to_string());
                                }
                                let param = Self::extract_string(&elements[2])?;
                                let value = Self::extract_string(&elements[3])?;
                                Ok(Command::ConfigSet(param, value))
                            }
                            "RESETSTAT" => Ok(Command::ConfigResetStat),
                            _ => Err(format!("ERR unknown subcommand or wrong number of arguments for 'config|{}' command", subcommand.to_lowercase())),
                        }
                    }
                    "SELECT" => {
                        if elements.len() != 2 {
                            return Err("ERR wrong number of arguments for 'select' command".to_string());
                        }
                        let db = Self::extract_u64(&elements[1])?;
                        if db > 15 {
                            return Err("ERR DB index is out of range".to_string());
                        }
                        Ok(Command::Select(db))
                    }
                    "ECHO" => {
                        if elements.len() != 2 {
                            return Err("ERR wrong number of arguments for 'echo' command".to_string());
                        }
                        let msg = Self::extract_sds(&elements[1])?;
                        Ok(Command::Echo(msg))
                    }
                    "AUTH" => {
                        match elements.len() {
                            2 => {
                                // AUTH password (authenticate as default user)
                                let password = Self::extract_string(&elements[1])?;
                                Ok(Command::Auth {
                                    username: None,
                                    password,
                                })
                            }
                            3 => {
                                // AUTH username password
                                let username = Self::extract_string(&elements[1])?;
                                let password = Self::extract_string(&elements[2])?;
                                Ok(Command::Auth {
                                    username: Some(username),
                                    password,
                                })
                            }
                            _ => Err("AUTH requires 1 or 2 arguments".to_string()),
                        }
                    }
                    "ACL" => {
                        if elements.len() < 2 {
                            return Err("ACL requires a subcommand".to_string());
                        }
                        let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "WHOAMI" => Ok(Command::AclWhoami),
                            "LIST" => Ok(Command::AclList),
                            "USERS" => Ok(Command::AclUsers),
                            "GETUSER" => {
                                if elements.len() != 3 {
                                    return Err("ACL GETUSER requires 1 argument".to_string());
                                }
                                let username = Self::extract_string(&elements[2])?;
                                Ok(Command::AclGetUser { username })
                            }
                            "SETUSER" => {
                                if elements.len() < 3 {
                                    return Err(
                                        "ACL SETUSER requires at least 1 argument".to_string()
                                    );
                                }
                                let username = Self::extract_string(&elements[2])?;
                                let rules: Vec<String> = elements[3..]
                                    .iter()
                                    .map(Self::extract_string)
                                    .collect::<Result<Vec<_>, _>>()?;
                                Ok(Command::AclSetUser { username, rules })
                            }
                            "DELUSER" => {
                                if elements.len() < 3 {
                                    return Err(
                                        "ACL DELUSER requires at least 1 argument".to_string()
                                    );
                                }
                                let usernames: Vec<String> = elements[2..]
                                    .iter()
                                    .map(Self::extract_string)
                                    .collect::<Result<Vec<_>, _>>()?;
                                Ok(Command::AclDelUser { usernames })
                            }
                            "CAT" => {
                                let category = if elements.len() > 2 {
                                    Some(Self::extract_string(&elements[2])?)
                                } else {
                                    None
                                };
                                Ok(Command::AclCat { category })
                            }
                            "GENPASS" => {
                                let bits = if elements.len() > 2 {
                                    Some(
                                        Self::extract_string(&elements[2])?
                                            .parse::<u32>()
                                            .map_err(|_| "Invalid bits value")?,
                                    )
                                } else {
                                    None
                                };
                                Ok(Command::AclGenPass { bits })
                            }
                            _ => Err(format!("Unknown ACL subcommand '{}'", subcommand)),
                        }
                    }
                    "FLUSHDB" => Ok(Command::FlushDb),
                    "FLUSHALL" => Ok(Command::FlushAll),
                    "MULTI" => Ok(Command::Multi),
                    "EXEC" => Ok(Command::Exec),
                    "DISCARD" => Ok(Command::Discard),
                    "WATCH" => {
                        if elements.len() < 2 {
                            return Err("WATCH requires at least 1 argument".to_string());
                        }
                        let keys: Vec<String> = elements[1..]
                            .iter()
                            .map(Self::extract_string)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::Watch(keys))
                    }
                    "UNWATCH" => Ok(Command::Unwatch),
                    "EVAL" => {
                        if elements.len() < 3 {
                            return Err("EVAL requires at least 2 arguments".to_string());
                        }
                        let script = Self::extract_string(&elements[1])?;
                        let numkeys = Self::extract_integer(&elements[2])? as usize;

                        // Validate we have enough arguments
                        if elements.len() < 3 + numkeys {
                            return Err("EVAL wrong number of keys".to_string());
                        }

                        let keys: Vec<String> = elements[3..3 + numkeys]
                            .iter()
                            .map(Self::extract_string)
                            .collect::<Result<Vec<_>, _>>()?;

                        let args: Vec<SDS> = elements[3 + numkeys..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;

                        Ok(Command::Eval { script, keys, args })
                    }
                    "EVALSHA" => {
                        if elements.len() < 3 {
                            return Err("EVALSHA requires at least 2 arguments".to_string());
                        }
                        let sha1 = Self::extract_string(&elements[1])?;
                        let numkeys = Self::extract_integer(&elements[2])? as usize;

                        // Validate we have enough arguments
                        if elements.len() < 3 + numkeys {
                            return Err("EVALSHA wrong number of keys".to_string());
                        }

                        let keys: Vec<String> = elements[3..3 + numkeys]
                            .iter()
                            .map(Self::extract_string)
                            .collect::<Result<Vec<_>, _>>()?;

                        let args: Vec<SDS> = elements[3 + numkeys..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;

                        Ok(Command::EvalSha { sha1, keys, args })
                    }
                    "SCRIPT" => {
                        if elements.len() < 2 {
                            return Err("SCRIPT requires a subcommand".to_string());
                        }
                        let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "LOAD" => {
                                if elements.len() != 3 {
                                    return Err("SCRIPT LOAD requires 1 argument".to_string());
                                }
                                let script = Self::extract_string(&elements[2])?;
                                Ok(Command::ScriptLoad(script))
                            }
                            "EXISTS" => {
                                if elements.len() < 3 {
                                    return Err(
                                        "SCRIPT EXISTS requires at least 1 argument".to_string()
                                    );
                                }
                                let sha1s: Vec<String> = elements[2..]
                                    .iter()
                                    .map(Self::extract_string)
                                    .collect::<Result<Vec<_>, _>>()?;
                                Ok(Command::ScriptExists(sha1s))
                            }
                            "FLUSH" => Ok(Command::ScriptFlush),
                            _ => Err(format!("Unknown SCRIPT subcommand '{}'", subcommand)),
                        }
                    }
                    "GET" => {
                        if elements.len() != 2 {
                            return Err("ERR wrong number of arguments for 'get' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Get(key))
                    }
                    "SET" => {
                        if elements.len() < 3 {
                            return Err("SET requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let value = Self::extract_sds(&elements[2])?;

                        let mut ex = None;
                        let mut px = None;
                        let mut exat = None;
                        let mut pxat = None;
                        let mut nx = false;
                        let mut xx = false;
                        let mut get = false;
                        let mut keepttl = false;

                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "NX" => nx = true,
                                "XX" => xx = true,
                                "GET" => get = true,
                                "EX" => {
                                    i += 1;
                                    if i >= elements.len() {
                                        return Err("SET EX requires a value".to_string());
                                    }
                                    ex = Some(Self::extract_i64(&elements[i])?);
                                }
                                "PX" => {
                                    i += 1;
                                    if i >= elements.len() {
                                        return Err("SET PX requires a value".to_string());
                                    }
                                    px = Some(Self::extract_i64(&elements[i])?);
                                }
                                "EXAT" => {
                                    i += 1;
                                    if i >= elements.len() {
                                        return Err("SET EXAT requires a value".to_string());
                                    }
                                    exat = Some(Self::extract_i64(&elements[i])?);
                                }
                                "PXAT" => {
                                    i += 1;
                                    if i >= elements.len() {
                                        return Err("SET PXAT requires a value".to_string());
                                    }
                                    pxat = Some(Self::extract_i64(&elements[i])?);
                                }
                                "KEEPTTL" => keepttl = true,
                                "IFEQ" | "IFGT" => {
                                    return Err(format!("SET {} option not yet supported", opt));
                                }
                                _ => return Err("ERR syntax error".to_string()),
                            }
                            i += 1;
                        }

                        // NX and XX are mutually exclusive
                        if nx && xx {
                            return Err(
                                "ERR XX and NX options at the same time are not compatible"
                                    .to_string(),
                            );
                        }

                        Ok(Command::Set {
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
                        })
                    }
                    "SETEX" => {
                        if elements.len() != 4 {
                            return Err("SETEX requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let seconds = Self::extract_integer(&elements[2])? as i64;
                        let value = Self::extract_sds(&elements[3])?;
                        Ok(Command::Set {
                            key,
                            value,
                            ex: Some(seconds),
                            px: None,
                            exat: None,
                            pxat: None,
                            nx: false,
                            xx: false,
                            get: false,
                            keepttl: false,
                        })
                    }
                    "SETNX" => {
                        if elements.len() != 3 {
                            return Err("SETNX requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let value = Self::extract_sds(&elements[2])?;
                        Ok(Command::SetNx(key, value))
                    }
                    "DEL" => {
                        if elements.len() < 2 {
                            return Err("DEL requires at least 1 argument".to_string());
                        }
                        let keys: Vec<String> = elements[1..]
                            .iter()
                            .map(Self::extract_string)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::Del(keys))
                    }
                    "EXISTS" => {
                        if elements.len() < 2 {
                            return Err("EXISTS requires at least 1 argument".to_string());
                        }
                        let keys = elements[1..]
                            .iter()
                            .map(Self::extract_string)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::Exists(keys))
                    }
                    "TYPE" => {
                        if elements.len() != 2 {
                            return Err("TYPE requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::TypeOf(key))
                    }
                    "KEYS" => {
                        if elements.len() != 2 {
                            return Err("KEYS requires 1 argument".to_string());
                        }
                        let pattern = Self::extract_string(&elements[1])?;
                        Ok(Command::Keys(pattern))
                    }
                    "EXPIRE" => {
                        if elements.len() < 3 {
                            return Err("EXPIRE requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let seconds = Self::extract_integer(&elements[2])? as i64;
                        let mut nx = false;
                        let mut xx = false;
                        let mut gt = false;
                        let mut lt = false;
                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "NX" => nx = true,
                                "XX" => xx = true,
                                "GT" => gt = true,
                                "LT" => lt = true,
                                _ => return Err(format!("ERR Unsupported option {}", opt)),
                            }
                            i += 1;
                        }
                        if nx && (xx || gt || lt) {
                            return Err("ERR NX and XX, GT or LT options at the same time are not compatible".to_string());
                        }
                        if gt && lt {
                            return Err("ERR GT and LT options at the same time are not compatible".to_string());
                        }
                        Ok(Command::Expire { key, seconds, nx, xx, gt, lt })
                    }
                    "PEXPIRE" => {
                        if elements.len() < 3 {
                            return Err("PEXPIRE requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let milliseconds = Self::extract_integer(&elements[2])? as i64;
                        let mut nx = false;
                        let mut xx = false;
                        let mut gt = false;
                        let mut lt = false;
                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "NX" => nx = true,
                                "XX" => xx = true,
                                "GT" => gt = true,
                                "LT" => lt = true,
                                _ => return Err(format!("ERR Unsupported option {}", opt)),
                            }
                            i += 1;
                        }
                        if nx && (xx || gt || lt) {
                            return Err("ERR NX and XX, GT or LT options at the same time are not compatible".to_string());
                        }
                        if gt && lt {
                            return Err("ERR GT and LT options at the same time are not compatible".to_string());
                        }
                        Ok(Command::PExpire { key, milliseconds, nx, xx, gt, lt })
                    }
                    "EXPIREAT" => {
                        if elements.len() != 3 {
                            return Err("EXPIREAT requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let timestamp = Self::extract_integer(&elements[2])? as i64;
                        Ok(Command::ExpireAt(key, timestamp))
                    }
                    "PEXPIREAT" => {
                        if elements.len() != 3 {
                            return Err("PEXPIREAT requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let timestamp_millis = Self::extract_integer(&elements[2])? as i64;
                        Ok(Command::PExpireAt(key, timestamp_millis))
                    }
                    "TTL" => {
                        if elements.len() != 2 {
                            return Err("TTL requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Ttl(key))
                    }
                    "PTTL" => {
                        if elements.len() != 2 {
                            return Err("PTTL requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Pttl(key))
                    }
                    "PERSIST" => {
                        if elements.len() != 2 {
                            return Err("PERSIST requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Persist(key))
                    }
                    "INCR" => {
                        if elements.len() != 2 {
                            return Err("ERR wrong number of arguments for 'incr' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Incr(key))
                    }
                    "DECR" => {
                        if elements.len() != 2 {
                            return Err("ERR wrong number of arguments for 'decr' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Decr(key))
                    }
                    "INCRBY" => {
                        if elements.len() != 3 {
                            return Err("ERR wrong number of arguments for 'incrby' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let increment = Self::extract_integer(&elements[2])? as i64;
                        Ok(Command::IncrBy(key, increment))
                    }
                    "DECRBY" => {
                        if elements.len() != 3 {
                            return Err("ERR wrong number of arguments for 'decrby' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let decrement = Self::extract_integer(&elements[2])? as i64;
                        Ok(Command::DecrBy(key, decrement))
                    }
                    "APPEND" => {
                        if elements.len() != 3 {
                            return Err("APPEND requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let value = Self::extract_sds(&elements[2])?;
                        Ok(Command::Append(key, value))
                    }
                    "GETSET" => {
                        if elements.len() != 3 {
                            return Err("GETSET requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let value = Self::extract_sds(&elements[2])?;
                        Ok(Command::GetSet(key, value))
                    }
                    "STRLEN" => {
                        if elements.len() != 2 {
                            return Err("STRLEN requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::StrLen(key))
                    }
                    "MGET" => {
                        if elements.len() < 2 {
                            return Err("MGET requires at least 1 argument".to_string());
                        }
                        let keys = elements[1..]
                            .iter()
                            .map(Self::extract_string)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::MGet(keys))
                    }
                    "MSET" => {
                        if elements.len() < 3 || (elements.len() - 1) % 2 != 0 {
                            return Err("ERR wrong number of arguments for 'mset' command".to_string());
                        }
                        // Pre-allocate capacity (Abseil Tip #19)
                        let mut pairs = Vec::with_capacity((elements.len() - 1) / 2);
                        for i in (1..elements.len()).step_by(2) {
                            let key = Self::extract_string(&elements[i])?;
                            let value = Self::extract_sds(&elements[i + 1])?;
                            pairs.push((key, value));
                        }
                        Ok(Command::MSet(pairs))
                    }
                    "MSETNX" => {
                        if elements.len() < 3 || (elements.len() - 1) % 2 != 0 {
                            return Err("ERR wrong number of arguments for 'msetnx' command".to_string());
                        }
                        let mut pairs = Vec::with_capacity((elements.len() - 1) / 2);
                        for i in (1..elements.len()).step_by(2) {
                            let key = Self::extract_string(&elements[i])?;
                            let value = Self::extract_sds(&elements[i + 1])?;
                            pairs.push((key, value));
                        }
                        Ok(Command::MSetNx(pairs))
                    }
                    "LPUSH" => {
                        if elements.len() < 3 {
                            return Err("LPUSH requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let values = elements[2..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::LPush(key, values))
                    }
                    "RPUSH" => {
                        if elements.len() < 3 {
                            return Err("RPUSH requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let values = elements[2..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::RPush(key, values))
                    }
                    "LPOP" => {
                        if elements.len() != 2 {
                            return Err("LPOP requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::LPop(key))
                    }
                    "RPOP" => {
                        if elements.len() != 2 {
                            return Err("RPOP requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::RPop(key))
                    }
                    "LRANGE" => {
                        if elements.len() != 4 {
                            return Err("LRANGE requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let start = Self::extract_integer(&elements[2])?;
                        let stop = Self::extract_integer(&elements[3])?;
                        Ok(Command::LRange(key, start, stop))
                    }
                    "LLEN" => {
                        if elements.len() != 2 {
                            return Err("LLEN requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::LLen(key))
                    }
                    "LINDEX" => {
                        if elements.len() != 3 {
                            return Err("LINDEX requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let index = Self::extract_integer(&elements[2])?;
                        Ok(Command::LIndex(key, index))
                    }
                    "LSET" => {
                        if elements.len() != 4 {
                            return Err("LSET requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let index = Self::extract_integer(&elements[2])?;
                        let value = Self::extract_sds(&elements[3])?;
                        Ok(Command::LSet(key, index, value))
                    }
                    "LTRIM" => {
                        if elements.len() != 4 {
                            return Err("LTRIM requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let start = Self::extract_integer(&elements[2])?;
                        let stop = Self::extract_integer(&elements[3])?;
                        Ok(Command::LTrim(key, start, stop))
                    }
                    "RPOPLPUSH" => {
                        if elements.len() != 3 {
                            return Err("RPOPLPUSH requires 2 arguments".to_string());
                        }
                        let source = Self::extract_string(&elements[1])?;
                        let dest = Self::extract_string(&elements[2])?;
                        Ok(Command::RPopLPush(source, dest))
                    }
                    "LMOVE" => {
                        if elements.len() != 5 {
                            return Err("LMOVE requires 4 arguments".to_string());
                        }
                        let source = Self::extract_string(&elements[1])?;
                        let dest = Self::extract_string(&elements[2])?;
                        let wherefrom = Self::extract_string(&elements[3])?.to_uppercase();
                        let whereto = Self::extract_string(&elements[4])?.to_uppercase();
                        if wherefrom != "LEFT" && wherefrom != "RIGHT" {
                            return Err("LMOVE wherefrom must be LEFT or RIGHT".to_string());
                        }
                        if whereto != "LEFT" && whereto != "RIGHT" {
                            return Err("LMOVE whereto must be LEFT or RIGHT".to_string());
                        }
                        Ok(Command::LMove {
                            source,
                            dest,
                            wherefrom,
                            whereto,
                        })
                    }
                    "SADD" => {
                        if elements.len() < 3 {
                            return Err("SADD requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let members = elements[2..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::SAdd(key, members))
                    }
                    "SMEMBERS" => {
                        if elements.len() != 2 {
                            return Err("SMEMBERS requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::SMembers(key))
                    }
                    "SISMEMBER" => {
                        if elements.len() != 3 {
                            return Err("SISMEMBER requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let member = Self::extract_sds(&elements[2])?;
                        Ok(Command::SIsMember(key, member))
                    }
                    "SREM" => {
                        if elements.len() < 3 {
                            return Err("SREM requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let members = elements[2..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::SRem(key, members))
                    }
                    "SCARD" => {
                        if elements.len() != 2 {
                            return Err("SCARD requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::SCard(key))
                    }
                    "SPOP" => {
                        // SPOP key [count]
                        if elements.len() < 2 || elements.len() > 3 {
                            return Err("SPOP requires 1 or 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let count = if elements.len() == 3 {
                            let count_str = Self::extract_string(&elements[2])?;
                            Some(
                                count_str
                                    .parse::<usize>()
                                    .map_err(|_| "ERR value is not an integer or out of range")?,
                            )
                        } else {
                            None
                        };
                        Ok(Command::SPop(key, count))
                    }
                    "HSET" => {
                        // HSET key field value [field value ...]
                        if elements.len() < 4 || (elements.len() - 2) % 2 != 0 {
                            return Err("HSET requires key and field-value pairs".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let mut pairs = Vec::with_capacity((elements.len() - 2) / 2);
                        for i in (2..elements.len()).step_by(2) {
                            let field = Self::extract_sds(&elements[i])?;
                            let value = Self::extract_sds(&elements[i + 1])?;
                            pairs.push((field, value));
                        }
                        Ok(Command::HSet(key, pairs))
                    }
                    "HGET" => {
                        if elements.len() != 3 {
                            return Err("HGET requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let field = Self::extract_sds(&elements[2])?;
                        Ok(Command::HGet(key, field))
                    }
                    "HGETALL" => {
                        if elements.len() != 2 {
                            return Err("HGETALL requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::HGetAll(key))
                    }
                    "HINCRBY" => {
                        if elements.len() != 4 {
                            return Err("HINCRBY requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let field = Self::extract_sds(&elements[2])?;
                        let increment = Self::extract_i64(&elements[3])?;
                        Ok(Command::HIncrBy(key, field, increment))
                    }
                    "HDEL" => {
                        if elements.len() < 3 {
                            return Err("HDEL requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let fields = elements[2..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::HDel(key, fields))
                    }
                    "HKEYS" => {
                        if elements.len() != 2 {
                            return Err("HKEYS requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::HKeys(key))
                    }
                    "HVALS" => {
                        if elements.len() != 2 {
                            return Err("HVALS requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::HVals(key))
                    }
                    "HLEN" => {
                        if elements.len() != 2 {
                            return Err("HLEN requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::HLen(key))
                    }
                    "HEXISTS" => {
                        if elements.len() != 3 {
                            return Err("HEXISTS requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let field = Self::extract_sds(&elements[2])?;
                        Ok(Command::HExists(key, field))
                    }
                    "ZADD" => {
                        if elements.len() < 4 {
                            return Err("ZADD requires key and score-member pairs".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;

                        // Parse optional flags (NX, XX, GT, LT, CH)
                        let mut nx = false;
                        let mut xx = false;
                        let mut gt = false;
                        let mut lt = false;
                        let mut ch = false;
                        let mut i = 2;

                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
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
                                _ => break, // Start of score-member pairs
                            }
                        }

                        // Rest are score-member pairs
                        if (elements.len() - i) % 2 != 0 || i >= elements.len() {
                            return Err("ZADD requires score-member pairs".to_string());
                        }

                        let mut pairs = Vec::with_capacity((elements.len() - i) / 2);
                        while i < elements.len() {
                            let score = Self::extract_float(&elements[i])?;
                            let member = Self::extract_sds(&elements[i + 1])?;
                            pairs.push((score, member));
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
                    "ZRANGE" => {
                        if elements.len() < 4 || elements.len() > 5 {
                            return Err("ZRANGE requires 3 or 4 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let start = Self::extract_integer(&elements[2])?;
                        let stop = Self::extract_integer(&elements[3])?;
                        let with_scores = if elements.len() == 5 {
                            let opt = Self::extract_string(&elements[4])?.to_uppercase();
                            opt == "WITHSCORES"
                        } else {
                            false
                        };
                        Ok(Command::ZRange(key, start, stop, with_scores))
                    }
                    "ZREVRANGE" => {
                        if elements.len() < 4 || elements.len() > 5 {
                            return Err("ZREVRANGE requires 3 or 4 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let start = Self::extract_integer(&elements[2])?;
                        let stop = Self::extract_integer(&elements[3])?;
                        let with_scores = if elements.len() == 5 {
                            let opt = Self::extract_string(&elements[4])?.to_uppercase();
                            opt == "WITHSCORES"
                        } else {
                            false
                        };
                        Ok(Command::ZRevRange(key, start, stop, with_scores))
                    }
                    "ZSCORE" => {
                        if elements.len() != 3 {
                            return Err("ZSCORE requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let member = Self::extract_sds(&elements[2])?;
                        Ok(Command::ZScore(key, member))
                    }
                    "ZREM" => {
                        if elements.len() < 3 {
                            return Err("ZREM requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let members = elements[2..]
                            .iter()
                            .map(Self::extract_sds)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::ZRem(key, members))
                    }
                    "ZRANK" => {
                        if elements.len() != 3 {
                            return Err("ZRANK requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let member = Self::extract_sds(&elements[2])?;
                        Ok(Command::ZRank(key, member))
                    }
                    "ZCARD" => {
                        if elements.len() != 2 {
                            return Err("ZCARD requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::ZCard(key))
                    }
                    "ZCOUNT" => {
                        if elements.len() != 4 {
                            return Err("ZCOUNT requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let min = Self::extract_string(&elements[2])?;
                        let max = Self::extract_string(&elements[3])?;
                        Ok(Command::ZCount(key, min, max))
                    }
                    "ZRANGEBYSCORE" => {
                        if elements.len() < 4 {
                            return Err("ZRANGEBYSCORE requires at least 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let min = Self::extract_string(&elements[2])?;
                        let max = Self::extract_string(&elements[3])?;
                        let mut with_scores = false;
                        let mut limit = None;
                        let mut i = 4;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "WITHSCORES" => with_scores = true,
                                "LIMIT" => {
                                    if i + 2 >= elements.len() {
                                        return Err("LIMIT requires offset and count".to_string());
                                    }
                                    let offset = Self::extract_integer(&elements[i + 1])?;
                                    let count = Self::extract_integer(&elements[i + 2])? as usize;
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
                    "SCAN" => {
                        if elements.len() < 2 {
                            return Err("SCAN requires at least 1 argument".to_string());
                        }
                        let cursor = Self::extract_u64(&elements[1])?;
                        let mut pattern = None;
                        let mut count = None;
                        let mut i = 2;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "MATCH" => {
                                    i += 1;
                                    pattern = Some(Self::extract_string(&elements[i])?);
                                }
                                "COUNT" => {
                                    i += 1;
                                    count = Some(Self::extract_integer(&elements[i])? as usize);
                                }
                                _ => return Err(format!("Unknown SCAN option: {}", opt)),
                            }
                            i += 1;
                        }
                        Ok(Command::Scan {
                            cursor,
                            pattern,
                            count,
                        })
                    }
                    "HSCAN" => {
                        if elements.len() < 3 {
                            return Err("HSCAN requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let cursor = Self::extract_u64(&elements[2])?;
                        let mut pattern = None;
                        let mut count = None;
                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "MATCH" => {
                                    i += 1;
                                    pattern = Some(Self::extract_string(&elements[i])?);
                                }
                                "COUNT" => {
                                    i += 1;
                                    count = Some(Self::extract_integer(&elements[i])? as usize);
                                }
                                _ => return Err(format!("Unknown HSCAN option: {}", opt)),
                            }
                            i += 1;
                        }
                        Ok(Command::HScan {
                            key,
                            cursor,
                            pattern,
                            count,
                        })
                    }
                    "ZSCAN" => {
                        if elements.len() < 3 {
                            return Err("ZSCAN requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let cursor = Self::extract_u64(&elements[2])?;
                        let mut pattern = None;
                        let mut count = None;
                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "MATCH" => {
                                    i += 1;
                                    pattern = Some(Self::extract_string(&elements[i])?);
                                }
                                "COUNT" => {
                                    i += 1;
                                    count = Some(Self::extract_integer(&elements[i])? as usize);
                                }
                                _ => return Err(format!("Unknown ZSCAN option: {}", opt)),
                            }
                            i += 1;
                        }
                        Ok(Command::ZScan {
                            key,
                            cursor,
                            pattern,
                            count,
                        })
                    }
                    "FUNCTION" => {
                        if elements.len() < 2 {
                            return Err("ERR wrong number of arguments for 'function' command".to_string());
                        }
                        let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "FLUSH" => Ok(Command::FunctionFlush),
                            _ => Ok(Command::Unknown(format!("FUNCTION {}", subcommand))),
                        }
                    }
                    "COMMAND" => {
                        if elements.len() >= 2 {
                            let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                            match subcommand.as_str() {
                                "COUNT" => Ok(Command::CommandCount),
                                _ => Ok(Command::CommandCommand),
                            }
                        } else {
                            Ok(Command::CommandCommand)
                        }
                    }
                    "CLIENT" => {
                        if elements.len() < 2 {
                            return Err("ERR wrong number of arguments for 'client' command".to_string());
                        }
                        let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "SETNAME" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'client|setname' command".to_string());
                                }
                                let name = Self::extract_string(&elements[2])?;
                                Ok(Command::ClientSetName(name))
                            }
                            "GETNAME" => Ok(Command::ClientGetName),
                            "ID" => Ok(Command::ClientId),
                            "INFO" => Ok(Command::ClientInfo),
                            _ => Ok(Command::Unknown(format!("CLIENT {}", subcommand))),
                        }
                    }
                    "OBJECT" => {
                        if elements.len() < 2 {
                            return Err("ERR wrong number of arguments for 'object' command".to_string());
                        }
                        let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "HELP" => Ok(Command::ObjectHelp),
                            "ENCODING" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'object|encoding' command".to_string());
                                }
                                Ok(Command::ObjectEncoding(Self::extract_string(&elements[2])?))
                            }
                            "REFCOUNT" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'object|refcount' command".to_string());
                                }
                                Ok(Command::ObjectRefCount(Self::extract_string(&elements[2])?))
                            }
                            "IDLETIME" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'object|idletime' command".to_string());
                                }
                                Ok(Command::ObjectIdleTime(Self::extract_string(&elements[2])?))
                            }
                            "FREQ" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'object|freq' command".to_string());
                                }
                                Ok(Command::ObjectFreq(Self::extract_string(&elements[2])?))
                            }
                            _ => Ok(Command::Unknown(format!("OBJECT {}", subcommand))),
                        }
                    }
                    "DEBUG" => {
                        if elements.len() < 2 {
                            return Err("ERR wrong number of arguments for 'debug' command".to_string());
                        }
                        let subcommand = Self::extract_string(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "SLEEP" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'debug|sleep' command".to_string());
                                }
                                let seconds = Self::extract_float(&elements[2])?;
                                Ok(Command::DebugSleep(seconds))
                            }
                            "SET-ACTIVE-EXPIRE" | "JMAP" | "RELOAD" | "LOADAOF" | "QUICKLIST-PACKED-THRESHOLD" => {
                                if elements.len() >= 3 {
                                    let val = Self::extract_string(&elements[2])?;
                                    Ok(Command::DebugSet(subcommand, val))
                                } else {
                                    Ok(Command::DebugSet(subcommand, String::new()))
                                }
                            }
                            "OBJECT" => {
                                if elements.len() != 3 {
                                    return Err("ERR wrong number of arguments for 'debug|object' command".to_string());
                                }
                                Ok(Command::DebugObject(Self::extract_string(&elements[2])?))
                            }
                            _ => {
                                // Accept any debug subcommand as a set
                                if elements.len() >= 3 {
                                    let val = Self::extract_string(&elements[2])?;
                                    Ok(Command::DebugSet(subcommand, val))
                                } else {
                                    Ok(Command::DebugSet(subcommand, String::new()))
                                }
                            }
                        }
                    }
                    "GETRANGE" | "SUBSTR" => {
                        if elements.len() != 4 {
                            return Err("GETRANGE requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let start = Self::extract_integer(&elements[2])?;
                        let end = Self::extract_integer(&elements[3])?;
                        Ok(Command::GetRange(key, start, end))
                    }
                    "SETRANGE" => {
                        if elements.len() != 4 {
                            return Err("SETRANGE requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let offset = Self::extract_integer(&elements[2])?;
                        if offset < 0 {
                            return Err("ERR offset is out of range".to_string());
                        }
                        let value = Self::extract_sds(&elements[3])?;
                        Ok(Command::SetRange(key, offset as usize, value))
                    }
                    "GETEX" => {
                        if elements.len() < 2 {
                            return Err("ERR wrong number of arguments for 'getex' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let mut ex = None;
                        let mut px = None;
                        let mut exat = None;
                        let mut pxat = None;
                        let mut persist = false;
                        let mut i = 2;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "EX" => {
                                    i += 1;
                                    if i >= elements.len() { return Err("GETEX EX requires a value".to_string()); }
                                    ex = Some(Self::extract_i64(&elements[i])?);
                                }
                                "PX" => {
                                    i += 1;
                                    if i >= elements.len() { return Err("GETEX PX requires a value".to_string()); }
                                    px = Some(Self::extract_i64(&elements[i])?);
                                }
                                "EXAT" => {
                                    i += 1;
                                    if i >= elements.len() { return Err("GETEX EXAT requires a value".to_string()); }
                                    exat = Some(Self::extract_i64(&elements[i])?);
                                }
                                "PXAT" => {
                                    i += 1;
                                    if i >= elements.len() { return Err("GETEX PXAT requires a value".to_string()); }
                                    pxat = Some(Self::extract_i64(&elements[i])?);
                                }
                                "PERSIST" => persist = true,
                                _ => return Err("ERR syntax error".to_string()),
                            }
                            i += 1;
                        }
                        Ok(Command::GetEx { key, ex, px, exat, pxat, persist })
                    }
                    "GETDEL" => {
                        if elements.len() != 2 {
                            return Err("GETDEL requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::GetDel(key))
                    }
                    "INCRBYFLOAT" => {
                        if elements.len() != 3 {
                            return Err("ERR wrong number of arguments for 'incrbyfloat' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let increment = Self::extract_float(&elements[2])?;
                        if increment.is_nan() || increment.is_infinite() {
                            return Err("ERR increment would produce NaN or Infinity".to_string());
                        }
                        Ok(Command::IncrByFloat(key, increment))
                    }
                    "PSETEX" => {
                        if elements.len() != 4 {
                            return Err("PSETEX requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let millis = Self::extract_integer(&elements[2])? as i64;
                        let value = Self::extract_sds(&elements[3])?;
                        Ok(Command::Set {
                            key,
                            value,
                            ex: None,
                            px: Some(millis),
                            exat: None,
                            pxat: None,
                            nx: false,
                            xx: false,
                            get: false,
                            keepttl: false,
                        })
                    }
                    "EXPIRETIME" => {
                        if elements.len() != 2 {
                            return Err("EXPIRETIME requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::ExpireTime(key))
                    }
                    "PEXPIRETIME" => {
                        if elements.len() != 2 {
                            return Err("PEXPIRETIME requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::PExpireTime(key))
                    }
                    "UNLINK" => {
                        if elements.len() < 2 {
                            return Err("UNLINK requires at least 1 argument".to_string());
                        }
                        let keys: Vec<String> = elements[1..]
                            .iter()
                            .map(Self::extract_string)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::Del(keys))
                    }
                    "WAIT" => {
                        if elements.len() != 3 {
                            return Err("WAIT requires 2 arguments".to_string());
                        }
                        let numreplicas = Self::extract_i64(&elements[1])?;
                        let timeout = Self::extract_i64(&elements[2])?;
                        Ok(Command::Wait(numreplicas, timeout))
                    }
                    "SORT" => {
                        if elements.len() < 2 {
                            return Err("ERR wrong number of arguments for 'sort' command".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let mut store = None;
                        let mut i = 2;
                        while i < elements.len() {
                            let opt = Self::extract_string(&elements[i])?.to_uppercase();
                            if opt == "STORE" {
                                i += 1;
                                if i < elements.len() {
                                    store = Some(Self::extract_string(&elements[i])?);
                                }
                            }
                            i += 1;
                        }
                        Ok(Command::Sort { key, store })
                    }
                    "RANDOMKEY" => Ok(Command::RandomKey),
                    "RENAME" => {
                        if elements.len() != 3 {
                            return Err("ERR wrong number of arguments for 'rename' command".to_string());
                        }
                        let src = Self::extract_string(&elements[1])?;
                        let dst = Self::extract_string(&elements[2])?;
                        Ok(Command::Rename(src, dst))
                    }
                    "RENAMENX" => {
                        if elements.len() != 3 {
                            return Err("ERR wrong number of arguments for 'renamenx' command".to_string());
                        }
                        let src = Self::extract_string(&elements[1])?;
                        let dst = Self::extract_string(&elements[2])?;
                        Ok(Command::RenameNx(src, dst))
                    }
                    _ => Ok(Command::Unknown(cmd_name)),
                }
            }
            _ => Err("Invalid command format".to_string()),
        }
    }

    // =========================================================================
    // Extract helpers for RespValue
    // =========================================================================

    fn extract_string(value: &RespValue) -> Result<String, String> {
        match value {
            RespValue::BulkString(Some(data)) => Ok(String::from_utf8_lossy(data).to_string()),
            _ => Err("Expected bulk string".to_string()),
        }
    }

    fn extract_sds(value: &RespValue) -> Result<SDS, String> {
        match value {
            RespValue::BulkString(Some(data)) => Ok(SDS::new(data.clone())),
            _ => Err("Expected bulk string".to_string()),
        }
    }

    fn extract_integer(value: &RespValue) -> Result<isize, String> {
        match value {
            RespValue::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<isize>()
                    .map_err(|_| "ERR value is not an integer or out of range".to_string())
            }
            RespValue::Integer(n) => Ok(*n as isize),
            _ => Err("ERR value is not an integer or out of range".to_string()),
        }
    }

    fn extract_float(value: &RespValue) -> Result<f64, String> {
        match value {
            RespValue::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<f64>()
                    .map_err(|_| "ERR value is not a valid float".to_string())
            }
            _ => Err("ERR value is not a valid float".to_string()),
        }
    }

    fn extract_i64(value: &RespValue) -> Result<i64, String> {
        match value {
            RespValue::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<i64>()
                    .map_err(|_| "ERR value is not an integer or out of range".to_string())
            }
            RespValue::Integer(n) => Ok(*n),
            _ => Err("ERR value is not an integer or out of range".to_string()),
        }
    }

    fn extract_u64(value: &RespValue) -> Result<u64, String> {
        match value {
            RespValue::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<u64>().map_err(|e| e.to_string())
            }
            RespValue::Integer(n) => Ok(*n as u64),
            _ => Err("Expected unsigned integer".to_string()),
        }
    }
}
