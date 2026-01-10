use super::data::*;
use super::resp::RespValue;
use super::resp_optimized::RespValueZeroCopy;
use crate::simulator::VirtualTime;
use ahash::AHashMap;

#[derive(Debug, Clone)]
pub enum Command {
    // String commands
    Get(String),
    /// SET key value [NX|XX] [EX seconds|PX milliseconds] [GET]
    Set {
        key: String,
        value: SDS,
        ex: Option<i64>, // EX seconds
        px: Option<i64>, // PX milliseconds
        nx: bool,        // Only set if NOT exists
        xx: bool,        // Only set if exists
        get: bool,       // Return old value
    },
    Append(String, SDS),
    GetSet(String, SDS),
    StrLen(String),
    MGet(Vec<String>),
    MSet(Vec<(String, SDS)>),
    /// Internal command for batched SET within a single shard (not exposed via RESP)
    BatchSet(Vec<(String, SDS)>),
    /// Internal command for batched GET within a single shard (not exposed via RESP)
    BatchGet(Vec<String>),
    // Counter commands
    Incr(String),
    Decr(String),
    IncrBy(String, i64),
    DecrBy(String, i64),
    // Key commands
    Del(Vec<String>),
    Exists(Vec<String>),
    TypeOf(String),
    Keys(String),
    FlushDb,
    FlushAll,
    // Expiration commands
    Expire(String, i64),
    ExpireAt(String, i64),
    PExpireAt(String, i64),
    Ttl(String),
    Pttl(String),
    Persist(String),
    // List commands
    LPush(String, Vec<SDS>),
    RPush(String, Vec<SDS>),
    LPop(String),
    RPop(String),
    LLen(String),
    LIndex(String, isize),
    LRange(String, isize, isize),
    LSet(String, isize, SDS),    // key, index, value
    LTrim(String, isize, isize), // key, start, stop
    RPopLPush(String, String),   // source, dest
    LMove {
        source: String,
        dest: String,
        wherefrom: String, // LEFT or RIGHT
        whereto: String,   // LEFT or RIGHT
    },
    // Set commands
    SAdd(String, Vec<SDS>),
    SRem(String, Vec<SDS>),
    SMembers(String),
    SIsMember(String, SDS),
    SCard(String),
    // Hash commands
    HSet(String, Vec<(SDS, SDS)>),
    HGet(String, SDS),
    HDel(String, Vec<SDS>),
    HGetAll(String),
    HKeys(String),
    HVals(String),
    HLen(String),
    HExists(String, SDS),
    HIncrBy(String, SDS, i64),
    // Sorted set commands
    /// ZADD with optional NX/XX/GT/LT/CH flags
    ZAdd {
        key: String,
        pairs: Vec<(f64, SDS)>,
        nx: bool, // Only add new elements
        xx: bool, // Only update existing elements
        gt: bool, // Only update when new score > current score
        lt: bool, // Only update when new score < current score
        ch: bool, // Return number of elements changed (not just added)
    },
    ZRem(String, Vec<SDS>),
    ZRange(String, isize, isize),
    ZRevRange(String, isize, isize, bool), // bool = WITHSCORES
    ZScore(String, SDS),
    ZRank(String, SDS),
    ZCard(String),
    ZCount(String, String, String), // key, min, max (strings to support -inf, +inf, exclusive)
    ZRangeByScore {
        key: String,
        min: String,
        max: String,
        with_scores: bool,
        limit: Option<(isize, usize)>, // offset, count
    },
    // Scan commands
    Scan {
        cursor: u64,
        pattern: Option<String>,
        count: Option<usize>,
    },
    HScan {
        key: String,
        cursor: u64,
        pattern: Option<String>,
        count: Option<usize>,
    },
    ZScan {
        key: String,
        cursor: u64,
        pattern: Option<String>,
        count: Option<usize>,
    },
    // Transaction commands
    Multi,
    Exec,
    Discard,
    Watch(Vec<String>),
    Unwatch,
    // Script commands
    Eval {
        script: String,
        keys: Vec<String>,
        args: Vec<SDS>,
    },
    EvalSha {
        sha1: String,
        keys: Vec<String>,
        args: Vec<SDS>,
    },
    /// SCRIPT LOAD command - loads script and returns SHA1
    ScriptLoad(String),
    /// SCRIPT EXISTS command - checks if scripts exist by SHA1
    ScriptExists(Vec<String>),
    /// SCRIPT FLUSH command - clears script cache
    ScriptFlush,
    // Server commands
    Info,
    Ping,
    DbSize,
    // Auth/ACL commands
    /// AUTH [username] password
    Auth {
        username: Option<String>,
        password: String,
    },
    /// ACL WHOAMI
    AclWhoami,
    /// ACL LIST
    AclList,
    /// ACL USERS
    AclUsers,
    /// ACL GETUSER username
    AclGetUser { username: String },
    /// ACL SETUSER username [rules...]
    AclSetUser {
        username: String,
        rules: Vec<String>,
    },
    /// ACL DELUSER username [username...]
    AclDelUser { usernames: Vec<String> },
    /// ACL CAT [category]
    AclCat { category: Option<String> },
    /// ACL GENPASS [bits]
    AclGenPass { bits: Option<u32> },
    Unknown(String),
}

impl Command {
    /// Helper constructor for basic SET (no options)
    pub fn set(key: String, value: SDS) -> Self {
        Command::Set {
            key,
            value,
            ex: None,
            px: None,
            nx: false,
            xx: false,
            get: false,
        }
    }

    /// Helper constructor for SETEX (SET with EX option)
    pub fn setex(key: String, seconds: i64, value: SDS) -> Self {
        Command::Set {
            key,
            value,
            ex: Some(seconds),
            px: None,
            nx: false,
            xx: false,
            get: false,
        }
    }

    /// Helper constructor for SETNX (SET with NX option)
    pub fn setnx(key: String, value: SDS) -> Self {
        Command::Set {
            key,
            value,
            ex: None,
            px: None,
            nx: true,
            xx: false,
            get: false,
        }
    }

    /// Helper constructor for single-key DEL
    pub fn del(key: String) -> Self {
        Command::Del(vec![key])
    }

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
                    "PING" => Ok(Command::Ping),
                    "INFO" => Ok(Command::Info),
                    "DBSIZE" => Ok(Command::DbSize),
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
                                    return Err("ACL SETUSER requires at least 1 argument".to_string());
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
                                    return Err("ACL DELUSER requires at least 1 argument".to_string());
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
                                    Some(Self::extract_string(&elements[2])?.parse::<u32>().map_err(|_| "Invalid bits value")?)
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
                            return Err("GET requires 1 argument".to_string());
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
                        let mut nx = false;
                        let mut xx = false;
                        let mut get = false;

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
                                "EXAT" | "PXAT" | "KEEPTTL" | "IFEQ" | "IFGT" => {
                                    return Err(format!("SET {} option not yet supported", opt));
                                }
                                _ => return Err(format!("ERR syntax error")),
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
                            nx,
                            xx,
                            get,
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
                            nx: false,
                            xx: false,
                            get: false,
                        })
                    }
                    "SETNX" => {
                        if elements.len() != 3 {
                            return Err("SETNX requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let value = Self::extract_sds(&elements[2])?;
                        Ok(Command::Set {
                            key,
                            value,
                            ex: None,
                            px: None,
                            nx: true,
                            xx: false,
                            get: false,
                        })
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
                        if elements.len() != 3 {
                            return Err("EXPIRE requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let seconds = Self::extract_integer(&elements[2])? as i64;
                        Ok(Command::Expire(key, seconds))
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
                            return Err("INCR requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Incr(key))
                    }
                    "DECR" => {
                        if elements.len() != 2 {
                            return Err("DECR requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Decr(key))
                    }
                    "INCRBY" => {
                        if elements.len() != 3 {
                            return Err("INCRBY requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let increment = Self::extract_integer(&elements[2])? as i64;
                        Ok(Command::IncrBy(key, increment))
                    }
                    "DECRBY" => {
                        if elements.len() != 3 {
                            return Err("DECRBY requires 2 arguments".to_string());
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
                            return Err("MSET requires key-value pairs".to_string());
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
                        if elements.len() != 4 {
                            return Err("ZRANGE requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let start = Self::extract_integer(&elements[2])?;
                        let stop = Self::extract_integer(&elements[3])?;
                        Ok(Command::ZRange(key, start, stop))
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
                    _ => Ok(Command::Unknown(cmd_name)),
                }
            }
            _ => Err("Invalid command format".to_string()),
        }
    }

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
                s.parse::<isize>().map_err(|e| e.to_string())
            }
            RespValue::Integer(n) => Ok(*n as isize),
            _ => Err("Expected integer".to_string()),
        }
    }

    fn extract_float(value: &RespValue) -> Result<f64, String> {
        match value {
            RespValue::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<f64>().map_err(|e| e.to_string())
            }
            _ => Err("Expected float".to_string()),
        }
    }

    fn extract_i64(value: &RespValue) -> Result<i64, String> {
        match value {
            RespValue::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<i64>().map_err(|e| e.to_string())
            }
            RespValue::Integer(n) => Ok(*n),
            _ => Err("Expected integer".to_string()),
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

    pub fn from_resp_zero_copy(value: &RespValueZeroCopy) -> Result<Command, String> {
        match value {
            RespValueZeroCopy::Array(Some(elements)) if !elements.is_empty() => {
                let cmd_name = match &elements[0] {
                    RespValueZeroCopy::BulkString(Some(data)) => {
                        String::from_utf8_lossy(data).to_uppercase()
                    }
                    _ => return Err("Invalid command format".to_string()),
                };

                match cmd_name.as_str() {
                    "PING" => Ok(Command::Ping),
                    "INFO" => Ok(Command::Info),
                    "DBSIZE" => Ok(Command::DbSize),
                    "AUTH" => {
                        match elements.len() {
                            2 => {
                                let password = Self::extract_string_zc(&elements[1])?;
                                Ok(Command::Auth {
                                    username: None,
                                    password,
                                })
                            }
                            3 => {
                                let username = Self::extract_string_zc(&elements[1])?;
                                let password = Self::extract_string_zc(&elements[2])?;
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
                        let subcommand = Self::extract_string_zc(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "WHOAMI" => Ok(Command::AclWhoami),
                            "LIST" => Ok(Command::AclList),
                            "USERS" => Ok(Command::AclUsers),
                            "GETUSER" => {
                                if elements.len() != 3 {
                                    return Err("ACL GETUSER requires 1 argument".to_string());
                                }
                                let username = Self::extract_string_zc(&elements[2])?;
                                Ok(Command::AclGetUser { username })
                            }
                            "SETUSER" => {
                                if elements.len() < 3 {
                                    return Err("ACL SETUSER requires at least 1 argument".to_string());
                                }
                                let username = Self::extract_string_zc(&elements[2])?;
                                let rules: Vec<String> = elements[3..]
                                    .iter()
                                    .map(Self::extract_string_zc)
                                    .collect::<Result<Vec<_>, _>>()?;
                                Ok(Command::AclSetUser { username, rules })
                            }
                            "DELUSER" => {
                                if elements.len() < 3 {
                                    return Err("ACL DELUSER requires at least 1 argument".to_string());
                                }
                                let usernames: Vec<String> = elements[2..]
                                    .iter()
                                    .map(Self::extract_string_zc)
                                    .collect::<Result<Vec<_>, _>>()?;
                                Ok(Command::AclDelUser { usernames })
                            }
                            "CAT" => {
                                let category = if elements.len() > 2 {
                                    Some(Self::extract_string_zc(&elements[2])?)
                                } else {
                                    None
                                };
                                Ok(Command::AclCat { category })
                            }
                            "GENPASS" => {
                                let bits = if elements.len() > 2 {
                                    Some(Self::extract_string_zc(&elements[2])?.parse::<u32>().map_err(|_| "Invalid bits value")?)
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
                            .map(Self::extract_string_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::Watch(keys))
                    }
                    "UNWATCH" => Ok(Command::Unwatch),
                    "EVAL" => {
                        if elements.len() < 3 {
                            return Err("EVAL requires at least 2 arguments".to_string());
                        }
                        let script = Self::extract_string_zc(&elements[1])?;
                        let numkeys = Self::extract_integer_zc(&elements[2])? as usize;

                        if elements.len() < 3 + numkeys {
                            return Err("EVAL wrong number of keys".to_string());
                        }

                        let keys: Vec<String> = elements[3..3 + numkeys]
                            .iter()
                            .map(Self::extract_string_zc)
                            .collect::<Result<Vec<_>, _>>()?;

                        let args: Vec<SDS> = elements[3 + numkeys..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;

                        Ok(Command::Eval { script, keys, args })
                    }
                    "EVALSHA" => {
                        if elements.len() < 3 {
                            return Err("EVALSHA requires at least 2 arguments".to_string());
                        }
                        let sha1 = Self::extract_string_zc(&elements[1])?;
                        let numkeys = Self::extract_integer_zc(&elements[2])? as usize;

                        if elements.len() < 3 + numkeys {
                            return Err("EVALSHA wrong number of keys".to_string());
                        }

                        let keys: Vec<String> = elements[3..3 + numkeys]
                            .iter()
                            .map(Self::extract_string_zc)
                            .collect::<Result<Vec<_>, _>>()?;

                        let args: Vec<SDS> = elements[3 + numkeys..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;

                        Ok(Command::EvalSha { sha1, keys, args })
                    }
                    "SCRIPT" => {
                        if elements.len() < 2 {
                            return Err("SCRIPT requires a subcommand".to_string());
                        }
                        let subcommand = Self::extract_string_zc(&elements[1])?.to_uppercase();
                        match subcommand.as_str() {
                            "LOAD" => {
                                if elements.len() != 3 {
                                    return Err("SCRIPT LOAD requires 1 argument".to_string());
                                }
                                let script = Self::extract_string_zc(&elements[2])?;
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
                                    .map(Self::extract_string_zc)
                                    .collect::<Result<Vec<_>, _>>()?;
                                Ok(Command::ScriptExists(sha1s))
                            }
                            "FLUSH" => Ok(Command::ScriptFlush),
                            _ => Err(format!("Unknown SCRIPT subcommand '{}'", subcommand)),
                        }
                    }
                    "GET" => {
                        if elements.len() != 2 {
                            return Err("GET requires 1 argument".to_string());
                        }
                        Ok(Command::Get(Self::extract_string_zc(&elements[1])?))
                    }
                    "SET" => {
                        if elements.len() < 3 {
                            return Err("SET requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let value = Self::extract_sds_zc(&elements[2])?;

                        let mut ex = None;
                        let mut px = None;
                        let mut nx = false;
                        let mut xx = false;
                        let mut get = false;

                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string_zc(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "NX" => nx = true,
                                "XX" => xx = true,
                                "GET" => get = true,
                                "EX" => {
                                    i += 1;
                                    if i >= elements.len() {
                                        return Err("SET EX requires a value".to_string());
                                    }
                                    ex = Some(Self::extract_i64_zc(&elements[i])?);
                                }
                                "PX" => {
                                    i += 1;
                                    if i >= elements.len() {
                                        return Err("SET PX requires a value".to_string());
                                    }
                                    px = Some(Self::extract_i64_zc(&elements[i])?);
                                }
                                "EXAT" | "PXAT" | "KEEPTTL" | "IFEQ" | "IFGT" => {
                                    return Err(format!("SET {} option not yet supported", opt));
                                }
                                _ => return Err(format!("ERR syntax error")),
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
                            nx,
                            xx,
                            get,
                        })
                    }
                    "SETEX" => {
                        if elements.len() != 4 {
                            return Err("SETEX requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let seconds = Self::extract_integer_zc(&elements[2])? as i64;
                        let value = Self::extract_sds_zc(&elements[3])?;
                        Ok(Command::Set {
                            key,
                            value,
                            ex: Some(seconds),
                            px: None,
                            nx: false,
                            xx: false,
                            get: false,
                        })
                    }
                    "SETNX" => {
                        if elements.len() != 3 {
                            return Err("SETNX requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let value = Self::extract_sds_zc(&elements[2])?;
                        Ok(Command::Set {
                            key,
                            value,
                            ex: None,
                            px: None,
                            nx: true,
                            xx: false,
                            get: false,
                        })
                    }
                    "DEL" => {
                        if elements.len() < 2 {
                            return Err("DEL requires at least 1 argument".to_string());
                        }
                        let keys: Vec<String> = elements[1..]
                            .iter()
                            .map(Self::extract_string_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::Del(keys))
                    }
                    "EXISTS" => {
                        if elements.len() < 2 {
                            return Err("EXISTS requires at least 1 argument".to_string());
                        }
                        Ok(Command::Exists(
                            elements[1..]
                                .iter()
                                .map(Self::extract_string_zc)
                                .collect::<Result<Vec<_>, _>>()?,
                        ))
                    }
                    "TYPE" => {
                        if elements.len() != 2 {
                            return Err("TYPE requires 1 argument".to_string());
                        }
                        Ok(Command::TypeOf(Self::extract_string_zc(&elements[1])?))
                    }
                    "KEYS" => {
                        if elements.len() != 2 {
                            return Err("KEYS requires 1 argument".to_string());
                        }
                        Ok(Command::Keys(Self::extract_string_zc(&elements[1])?))
                    }
                    "EXPIRE" => {
                        if elements.len() != 3 {
                            return Err("EXPIRE requires 2 arguments".to_string());
                        }
                        Ok(Command::Expire(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])? as i64,
                        ))
                    }
                    "EXPIREAT" => {
                        if elements.len() != 3 {
                            return Err("EXPIREAT requires 2 arguments".to_string());
                        }
                        Ok(Command::ExpireAt(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])? as i64,
                        ))
                    }
                    "PEXPIREAT" => {
                        if elements.len() != 3 {
                            return Err("PEXPIREAT requires 2 arguments".to_string());
                        }
                        Ok(Command::PExpireAt(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])? as i64,
                        ))
                    }
                    "TTL" => {
                        if elements.len() != 2 {
                            return Err("TTL requires 1 argument".to_string());
                        }
                        Ok(Command::Ttl(Self::extract_string_zc(&elements[1])?))
                    }
                    "PTTL" => {
                        if elements.len() != 2 {
                            return Err("PTTL requires 1 argument".to_string());
                        }
                        Ok(Command::Pttl(Self::extract_string_zc(&elements[1])?))
                    }
                    "PERSIST" => {
                        if elements.len() != 2 {
                            return Err("PERSIST requires 1 argument".to_string());
                        }
                        Ok(Command::Persist(Self::extract_string_zc(&elements[1])?))
                    }
                    "INCR" => {
                        if elements.len() != 2 {
                            return Err("INCR requires 1 argument".to_string());
                        }
                        Ok(Command::Incr(Self::extract_string_zc(&elements[1])?))
                    }
                    "DECR" => {
                        if elements.len() != 2 {
                            return Err("DECR requires 1 argument".to_string());
                        }
                        Ok(Command::Decr(Self::extract_string_zc(&elements[1])?))
                    }
                    "INCRBY" => {
                        if elements.len() != 3 {
                            return Err("INCRBY requires 2 arguments".to_string());
                        }
                        Ok(Command::IncrBy(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])? as i64,
                        ))
                    }
                    "DECRBY" => {
                        if elements.len() != 3 {
                            return Err("DECRBY requires 2 arguments".to_string());
                        }
                        Ok(Command::DecrBy(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])? as i64,
                        ))
                    }
                    "APPEND" => {
                        if elements.len() != 3 {
                            return Err("APPEND requires 2 arguments".to_string());
                        }
                        Ok(Command::Append(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                        ))
                    }
                    "GETSET" => {
                        if elements.len() != 3 {
                            return Err("GETSET requires 2 arguments".to_string());
                        }
                        Ok(Command::GetSet(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                        ))
                    }
                    "STRLEN" => {
                        if elements.len() != 2 {
                            return Err("STRLEN requires 1 argument".to_string());
                        }
                        Ok(Command::StrLen(Self::extract_string_zc(&elements[1])?))
                    }
                    "MGET" => {
                        if elements.len() < 2 {
                            return Err("MGET requires at least 1 argument".to_string());
                        }
                        Ok(Command::MGet(
                            elements[1..]
                                .iter()
                                .map(Self::extract_string_zc)
                                .collect::<Result<Vec<_>, _>>()?,
                        ))
                    }
                    "MSET" => {
                        if elements.len() < 3 || (elements.len() - 1) % 2 != 0 {
                            return Err("MSET requires key-value pairs".to_string());
                        }
                        // Pre-allocate capacity (Abseil Tip #19)
                        let mut pairs = Vec::with_capacity((elements.len() - 1) / 2);
                        for i in (1..elements.len()).step_by(2) {
                            pairs.push((
                                Self::extract_string_zc(&elements[i])?,
                                Self::extract_sds_zc(&elements[i + 1])?,
                            ));
                        }
                        Ok(Command::MSet(pairs))
                    }
                    "LPUSH" => {
                        if elements.len() < 3 {
                            return Err("LPUSH requires key and values".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let values = elements[2..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::LPush(key, values))
                    }
                    "RPUSH" => {
                        if elements.len() < 3 {
                            return Err("RPUSH requires key and values".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let values = elements[2..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::RPush(key, values))
                    }
                    "LPOP" => {
                        if elements.len() != 2 {
                            return Err("LPOP requires 1 argument".to_string());
                        }
                        Ok(Command::LPop(Self::extract_string_zc(&elements[1])?))
                    }
                    "RPOP" => {
                        if elements.len() != 2 {
                            return Err("RPOP requires 1 argument".to_string());
                        }
                        Ok(Command::RPop(Self::extract_string_zc(&elements[1])?))
                    }
                    "LRANGE" => {
                        if elements.len() != 4 {
                            return Err("LRANGE requires 3 arguments".to_string());
                        }
                        Ok(Command::LRange(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])?,
                            Self::extract_integer_zc(&elements[3])?,
                        ))
                    }
                    "LLEN" => {
                        if elements.len() != 2 {
                            return Err("LLEN requires 1 argument".to_string());
                        }
                        Ok(Command::LLen(Self::extract_string_zc(&elements[1])?))
                    }
                    "LINDEX" => {
                        if elements.len() != 3 {
                            return Err("LINDEX requires 2 arguments".to_string());
                        }
                        Ok(Command::LIndex(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])?,
                        ))
                    }
                    "LSET" => {
                        if elements.len() != 4 {
                            return Err("LSET requires 3 arguments".to_string());
                        }
                        Ok(Command::LSet(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])?,
                            Self::extract_sds_zc(&elements[3])?,
                        ))
                    }
                    "LTRIM" => {
                        if elements.len() != 4 {
                            return Err("LTRIM requires 3 arguments".to_string());
                        }
                        Ok(Command::LTrim(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])?,
                            Self::extract_integer_zc(&elements[3])?,
                        ))
                    }
                    "RPOPLPUSH" => {
                        if elements.len() != 3 {
                            return Err("RPOPLPUSH requires 2 arguments".to_string());
                        }
                        Ok(Command::RPopLPush(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_string_zc(&elements[2])?,
                        ))
                    }
                    "LMOVE" => {
                        if elements.len() != 5 {
                            return Err("LMOVE requires 4 arguments".to_string());
                        }
                        let source = Self::extract_string_zc(&elements[1])?;
                        let dest = Self::extract_string_zc(&elements[2])?;
                        let wherefrom = Self::extract_string_zc(&elements[3])?.to_uppercase();
                        let whereto = Self::extract_string_zc(&elements[4])?.to_uppercase();
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
                            return Err("SADD requires key and members".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let members = elements[2..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::SAdd(key, members))
                    }
                    "SMEMBERS" => {
                        if elements.len() != 2 {
                            return Err("SMEMBERS requires 1 argument".to_string());
                        }
                        Ok(Command::SMembers(Self::extract_string_zc(&elements[1])?))
                    }
                    "SISMEMBER" => {
                        if elements.len() != 3 {
                            return Err("SISMEMBER requires 2 arguments".to_string());
                        }
                        Ok(Command::SIsMember(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                        ))
                    }
                    "SREM" => {
                        if elements.len() < 3 {
                            return Err("SREM requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let members = elements[2..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::SRem(key, members))
                    }
                    "SCARD" => {
                        if elements.len() != 2 {
                            return Err("SCARD requires 1 argument".to_string());
                        }
                        Ok(Command::SCard(Self::extract_string_zc(&elements[1])?))
                    }
                    "HSET" => {
                        // HSET key field value [field value ...]
                        if elements.len() < 4 || (elements.len() - 2) % 2 != 0 {
                            return Err("HSET requires key and field-value pairs".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let mut pairs = Vec::with_capacity((elements.len() - 2) / 2);
                        for i in (2..elements.len()).step_by(2) {
                            let field = Self::extract_sds_zc(&elements[i])?;
                            let value = Self::extract_sds_zc(&elements[i + 1])?;
                            pairs.push((field, value));
                        }
                        Ok(Command::HSet(key, pairs))
                    }
                    "HGET" => {
                        if elements.len() != 3 {
                            return Err("HGET requires 2 arguments".to_string());
                        }
                        Ok(Command::HGet(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                        ))
                    }
                    "HGETALL" => {
                        if elements.len() != 2 {
                            return Err("HGETALL requires 1 argument".to_string());
                        }
                        Ok(Command::HGetAll(Self::extract_string_zc(&elements[1])?))
                    }
                    "HINCRBY" => {
                        if elements.len() != 4 {
                            return Err("HINCRBY requires 3 arguments".to_string());
                        }
                        Ok(Command::HIncrBy(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                            Self::extract_i64_zc(&elements[3])?,
                        ))
                    }
                    "HDEL" => {
                        if elements.len() < 3 {
                            return Err("HDEL requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let fields = elements[2..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::HDel(key, fields))
                    }
                    "HKEYS" => {
                        if elements.len() != 2 {
                            return Err("HKEYS requires 1 argument".to_string());
                        }
                        Ok(Command::HKeys(Self::extract_string_zc(&elements[1])?))
                    }
                    "HVALS" => {
                        if elements.len() != 2 {
                            return Err("HVALS requires 1 argument".to_string());
                        }
                        Ok(Command::HVals(Self::extract_string_zc(&elements[1])?))
                    }
                    "HLEN" => {
                        if elements.len() != 2 {
                            return Err("HLEN requires 1 argument".to_string());
                        }
                        Ok(Command::HLen(Self::extract_string_zc(&elements[1])?))
                    }
                    "HEXISTS" => {
                        if elements.len() != 3 {
                            return Err("HEXISTS requires 2 arguments".to_string());
                        }
                        Ok(Command::HExists(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                        ))
                    }
                    "ZADD" => {
                        if elements.len() < 4 {
                            return Err("ZADD requires key and score-member pairs".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;

                        // Parse optional flags (NX, XX, GT, LT, CH)
                        let mut nx = false;
                        let mut xx = false;
                        let mut gt = false;
                        let mut lt = false;
                        let mut ch = false;
                        let mut i = 2;

                        while i < elements.len() {
                            let opt = Self::extract_string_zc(&elements[i])?.to_uppercase();
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

                        if (elements.len() - i) % 2 != 0 || i >= elements.len() {
                            return Err("ZADD requires score-member pairs".to_string());
                        }

                        let mut pairs = Vec::with_capacity((elements.len() - i) / 2);
                        while i < elements.len() {
                            pairs.push((
                                Self::extract_float_zc(&elements[i])?,
                                Self::extract_sds_zc(&elements[i + 1])?,
                            ));
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
                        if elements.len() != 4 {
                            return Err("ZRANGE requires 3 arguments".to_string());
                        }
                        Ok(Command::ZRange(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_integer_zc(&elements[2])?,
                            Self::extract_integer_zc(&elements[3])?,
                        ))
                    }
                    "ZREVRANGE" => {
                        if elements.len() < 4 || elements.len() > 5 {
                            return Err("ZREVRANGE requires 3 or 4 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let start = Self::extract_integer_zc(&elements[2])?;
                        let stop = Self::extract_integer_zc(&elements[3])?;
                        let with_scores = if elements.len() == 5 {
                            Self::extract_string_zc(&elements[4])?.to_uppercase() == "WITHSCORES"
                        } else {
                            false
                        };
                        Ok(Command::ZRevRange(key, start, stop, with_scores))
                    }
                    "ZSCORE" => {
                        if elements.len() != 3 {
                            return Err("ZSCORE requires 2 arguments".to_string());
                        }
                        Ok(Command::ZScore(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                        ))
                    }
                    "ZREM" => {
                        if elements.len() < 3 {
                            return Err("ZREM requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let members = elements[2..]
                            .iter()
                            .map(Self::extract_sds_zc)
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::ZRem(key, members))
                    }
                    "ZRANK" => {
                        if elements.len() != 3 {
                            return Err("ZRANK requires 2 arguments".to_string());
                        }
                        Ok(Command::ZRank(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_sds_zc(&elements[2])?,
                        ))
                    }
                    "ZCARD" => {
                        if elements.len() != 2 {
                            return Err("ZCARD requires 1 argument".to_string());
                        }
                        Ok(Command::ZCard(Self::extract_string_zc(&elements[1])?))
                    }
                    "ZCOUNT" => {
                        if elements.len() != 4 {
                            return Err("ZCOUNT requires 3 arguments".to_string());
                        }
                        Ok(Command::ZCount(
                            Self::extract_string_zc(&elements[1])?,
                            Self::extract_string_zc(&elements[2])?,
                            Self::extract_string_zc(&elements[3])?,
                        ))
                    }
                    "ZRANGEBYSCORE" => {
                        if elements.len() < 4 {
                            return Err("ZRANGEBYSCORE requires at least 3 arguments".to_string());
                        }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let min = Self::extract_string_zc(&elements[2])?;
                        let max = Self::extract_string_zc(&elements[3])?;
                        let mut with_scores = false;
                        let mut limit = None;
                        let mut i = 4;
                        while i < elements.len() {
                            let opt = Self::extract_string_zc(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "WITHSCORES" => with_scores = true,
                                "LIMIT" => {
                                    if i + 2 >= elements.len() {
                                        return Err("LIMIT requires offset and count".to_string());
                                    }
                                    let offset = Self::extract_integer_zc(&elements[i + 1])?;
                                    let count =
                                        Self::extract_integer_zc(&elements[i + 2])? as usize;
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
                        let cursor = Self::extract_u64_zc(&elements[1])?;
                        let mut pattern = None;
                        let mut count = None;
                        let mut i = 2;
                        while i < elements.len() {
                            let opt = Self::extract_string_zc(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "MATCH" => {
                                    i += 1;
                                    pattern = Some(Self::extract_string_zc(&elements[i])?);
                                }
                                "COUNT" => {
                                    i += 1;
                                    count = Some(Self::extract_integer_zc(&elements[i])? as usize);
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
                        let key = Self::extract_string_zc(&elements[1])?;
                        let cursor = Self::extract_u64_zc(&elements[2])?;
                        let mut pattern = None;
                        let mut count = None;
                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string_zc(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "MATCH" => {
                                    i += 1;
                                    pattern = Some(Self::extract_string_zc(&elements[i])?);
                                }
                                "COUNT" => {
                                    i += 1;
                                    count = Some(Self::extract_integer_zc(&elements[i])? as usize);
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
                        let key = Self::extract_string_zc(&elements[1])?;
                        let cursor = Self::extract_u64_zc(&elements[2])?;
                        let mut pattern = None;
                        let mut count = None;
                        let mut i = 3;
                        while i < elements.len() {
                            let opt = Self::extract_string_zc(&elements[i])?.to_uppercase();
                            match opt.as_str() {
                                "MATCH" => {
                                    i += 1;
                                    pattern = Some(Self::extract_string_zc(&elements[i])?);
                                }
                                "COUNT" => {
                                    i += 1;
                                    count = Some(Self::extract_integer_zc(&elements[i])? as usize);
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
                    _ => Ok(Command::Unknown(cmd_name)),
                }
            }
            _ => Err("Invalid command format".to_string()),
        }
    }

    fn extract_string_zc(value: &RespValueZeroCopy) -> Result<String, String> {
        match value {
            RespValueZeroCopy::BulkString(Some(data)) => {
                Ok(String::from_utf8_lossy(data).to_string())
            }
            _ => Err("Expected bulk string".to_string()),
        }
    }

    fn extract_sds_zc(value: &RespValueZeroCopy) -> Result<SDS, String> {
        match value {
            RespValueZeroCopy::BulkString(Some(data)) => Ok(SDS::new(data.to_vec())),
            _ => Err("Expected bulk string".to_string()),
        }
    }

    fn extract_integer_zc(value: &RespValueZeroCopy) -> Result<isize, String> {
        match value {
            RespValueZeroCopy::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<isize>().map_err(|e| e.to_string())
            }
            RespValueZeroCopy::Integer(n) => Ok(*n as isize),
            _ => Err("Expected integer".to_string()),
        }
    }

    fn extract_float_zc(value: &RespValueZeroCopy) -> Result<f64, String> {
        match value {
            RespValueZeroCopy::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<f64>().map_err(|e| e.to_string())
            }
            _ => Err("Expected float".to_string()),
        }
    }

    fn extract_i64_zc(value: &RespValueZeroCopy) -> Result<i64, String> {
        match value {
            RespValueZeroCopy::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<i64>().map_err(|e| e.to_string())
            }
            RespValueZeroCopy::Integer(n) => Ok(*n),
            _ => Err("Expected integer".to_string()),
        }
    }

    fn extract_u64_zc(value: &RespValueZeroCopy) -> Result<u64, String> {
        match value {
            RespValueZeroCopy::BulkString(Some(data)) => {
                let s = String::from_utf8_lossy(data);
                s.parse::<u64>().map_err(|e| e.to_string())
            }
            RespValueZeroCopy::Integer(n) => Ok(*n as u64),
            _ => Err("Expected unsigned integer".to_string()),
        }
    }
}

pub struct CommandExecutor {
    data: AHashMap<String, Value>,
    expirations: AHashMap<String, VirtualTime>,
    current_time: VirtualTime,
    access_times: AHashMap<String, VirtualTime>,
    #[allow(dead_code)]
    key_count: usize,
    commands_processed: usize,
    simulation_start_epoch: i64,
    // Transaction state
    in_transaction: bool,
    queued_commands: Vec<Command>,
    watched_keys: AHashMap<String, Option<Value>>, // key -> value at watch time
    // Lua scripting - local cache for single-shard mode
    script_cache: super::lua::ScriptCache,
    // Shared script cache for multi-shard mode (all shards share one cache)
    shared_script_cache: Option<super::lua::SharedScriptCache>,
}

impl Command {
    /// Returns true if this command only reads data (no mutations)
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Command::Get(_)
                | Command::StrLen(_)
                | Command::MGet(_)
                | Command::Exists(_)
                | Command::TypeOf(_)
                | Command::Keys(_)
                | Command::Ttl(_)
                | Command::Pttl(_)
                | Command::LLen(_)
                | Command::LIndex(_, _)
                | Command::LRange(_, _, _)
                | Command::SMembers(_)
                | Command::SIsMember(_, _)
                | Command::SCard(_)
                | Command::HGet(_, _)
                | Command::HGetAll(_)
                | Command::HKeys(_)
                | Command::HVals(_)
                | Command::HLen(_)
                | Command::HExists(_, _)
                | Command::ZRange(_, _, _)
                | Command::ZRevRange(_, _, _, _)
                | Command::ZScore(_, _)
                | Command::ZRank(_, _)
                | Command::ZCard(_)
                | Command::ZCount(_, _, _)
                | Command::ZRangeByScore { .. }
                | Command::Scan { .. }
                | Command::HScan { .. }
                | Command::ZScan { .. }
                | Command::Info
                | Command::Ping
        )
    }

    /// Returns the key(s) this command operates on (for sharding)
    pub fn get_primary_key(&self) -> Option<&str> {
        match self {
            Command::Get(k)
            | Command::Set { key: k, .. }
            | Command::TypeOf(k)
            | Command::Expire(k, _)
            | Command::ExpireAt(k, _)
            | Command::PExpireAt(k, _)
            | Command::Ttl(k)
            | Command::Pttl(k)
            | Command::Persist(k)
            | Command::Incr(k)
            | Command::Decr(k)
            | Command::IncrBy(k, _)
            | Command::DecrBy(k, _)
            | Command::Append(k, _)
            | Command::GetSet(k, _)
            | Command::StrLen(k)
            | Command::LPush(k, _)
            | Command::RPush(k, _)
            | Command::LPop(k)
            | Command::RPop(k)
            | Command::LLen(k)
            | Command::LIndex(k, _)
            | Command::LRange(k, _, _)
            | Command::LSet(k, _, _)
            | Command::LTrim(k, _, _)
            | Command::RPopLPush(k, _)
            | Command::LMove { source: k, .. }
            | Command::SAdd(k, _)
            | Command::SRem(k, _)
            | Command::SMembers(k)
            | Command::SIsMember(k, _)
            | Command::SCard(k)
            | Command::HSet(k, _)
            | Command::HGet(k, _)
            | Command::HDel(k, _)
            | Command::HGetAll(k)
            | Command::HKeys(k)
            | Command::HVals(k)
            | Command::HLen(k)
            | Command::HExists(k, _)
            | Command::HIncrBy(k, _, _)
            | Command::ZAdd { key: k, .. }
            | Command::ZRem(k, _)
            | Command::ZRange(k, _, _)
            | Command::ZRevRange(k, _, _, _)
            | Command::ZScore(k, _)
            | Command::ZRank(k, _)
            | Command::ZCard(k)
            | Command::ZCount(k, _, _)
            | Command::ZRangeByScore { key: k, .. }
            | Command::HScan { key: k, .. }
            | Command::ZScan { key: k, .. } => Some(k.as_str()),
            Command::Del(keys) | Command::Exists(keys) => keys.first().map(|s| s.as_str()),
            Command::MGet(keys) => keys.first().map(|s| s.as_str()),
            Command::MSet(pairs) => pairs.first().map(|(k, _)| k.as_str()),
            Command::BatchSet(pairs) => pairs.first().map(|(k, _)| k.as_str()),
            Command::BatchGet(keys) => keys.first().map(|s| s.as_str()),
            Command::Watch(keys) => keys.first().map(|s| s.as_str()),
            Command::Eval { keys, .. } | Command::EvalSha { keys, .. } => {
                keys.first().map(|s| s.as_str())
            }
            Command::Scan { .. }
            | Command::Keys(_)
            | Command::FlushDb
            | Command::FlushAll
            | Command::Multi
            | Command::Exec
            | Command::Discard
            | Command::Unwatch
            | Command::ScriptLoad(_)
            | Command::ScriptExists(_)
            | Command::ScriptFlush
            | Command::Info
            | Command::Ping
            | Command::DbSize
            | Command::Auth { .. }
            | Command::AclWhoami
            | Command::AclList
            | Command::AclUsers
            | Command::AclGetUser { .. }
            | Command::AclSetUser { .. }
            | Command::AclDelUser { .. }
            | Command::AclCat { .. }
            | Command::AclGenPass { .. }
            | Command::Unknown(_) => None,
        }
    }

    /// Returns the command name as a string (for metrics/tracing)
    #[inline]
    pub fn name(&self) -> &'static str {
        match self {
            Command::Get(_) => "GET",
            Command::Set { .. } => "SET",
            Command::Append(_, _) => "APPEND",
            Command::GetSet(_, _) => "GETSET",
            Command::StrLen(_) => "STRLEN",
            Command::MGet(_) => "MGET",
            Command::MSet(_) => "MSET",
            Command::BatchSet(_) => "BATCHSET",
            Command::BatchGet(_) => "BATCHGET",
            Command::Incr(_) => "INCR",
            Command::Decr(_) => "DECR",
            Command::IncrBy(_, _) => "INCRBY",
            Command::DecrBy(_, _) => "DECRBY",
            Command::Del(_) => "DEL",
            Command::Exists(_) => "EXISTS",
            Command::TypeOf(_) => "TYPE",
            Command::Keys(_) => "KEYS",
            Command::FlushDb => "FLUSHDB",
            Command::FlushAll => "FLUSHALL",
            Command::Expire(_, _) => "EXPIRE",
            Command::ExpireAt(_, _) => "EXPIREAT",
            Command::PExpireAt(_, _) => "PEXPIREAT",
            Command::Ttl(_) => "TTL",
            Command::Pttl(_) => "PTTL",
            Command::Persist(_) => "PERSIST",
            Command::LPush(_, _) => "LPUSH",
            Command::RPush(_, _) => "RPUSH",
            Command::LPop(_) => "LPOP",
            Command::RPop(_) => "RPOP",
            Command::LLen(_) => "LLEN",
            Command::LIndex(_, _) => "LINDEX",
            Command::LRange(_, _, _) => "LRANGE",
            Command::LSet(_, _, _) => "LSET",
            Command::LTrim(_, _, _) => "LTRIM",
            Command::RPopLPush(_, _) => "RPOPLPUSH",
            Command::LMove { .. } => "LMOVE",
            Command::SAdd(_, _) => "SADD",
            Command::SRem(_, _) => "SREM",
            Command::SMembers(_) => "SMEMBERS",
            Command::SIsMember(_, _) => "SISMEMBER",
            Command::SCard(_) => "SCARD",
            Command::HSet(_, _) => "HSET",
            Command::HGet(_, _) => "HGET",
            Command::HDel(_, _) => "HDEL",
            Command::HGetAll(_) => "HGETALL",
            Command::HKeys(_) => "HKEYS",
            Command::HVals(_) => "HVALS",
            Command::HLen(_) => "HLEN",
            Command::HExists(_, _) => "HEXISTS",
            Command::HIncrBy(_, _, _) => "HINCRBY",
            Command::ZAdd { .. } => "ZADD",
            Command::ZRem(_, _) => "ZREM",
            Command::ZRange(_, _, _) => "ZRANGE",
            Command::ZRevRange(_, _, _, _) => "ZREVRANGE",
            Command::ZScore(_, _) => "ZSCORE",
            Command::ZRank(_, _) => "ZRANK",
            Command::ZCard(_) => "ZCARD",
            Command::ZCount(_, _, _) => "ZCOUNT",
            Command::ZRangeByScore { .. } => "ZRANGEBYSCORE",
            Command::Scan { .. } => "SCAN",
            Command::HScan { .. } => "HSCAN",
            Command::ZScan { .. } => "ZSCAN",
            Command::Multi => "MULTI",
            Command::Exec => "EXEC",
            Command::Discard => "DISCARD",
            Command::Watch(_) => "WATCH",
            Command::Unwatch => "UNWATCH",
            Command::Eval { .. } => "EVAL",
            Command::EvalSha { .. } => "EVALSHA",
            Command::ScriptLoad(_) => "SCRIPT",
            Command::ScriptExists(_) => "SCRIPT",
            Command::ScriptFlush => "SCRIPT",
            Command::Info => "INFO",
            Command::Ping => "PING",
            Command::DbSize => "DBSIZE",
            Command::Auth { .. } => "AUTH",
            Command::AclWhoami => "ACL",
            Command::AclList => "ACL",
            Command::AclUsers => "ACL",
            Command::AclGetUser { .. } => "ACL",
            Command::AclSetUser { .. } => "ACL",
            Command::AclDelUser { .. } => "ACL",
            Command::AclCat { .. } => "ACL",
            Command::AclGenPass { .. } => "ACL",
            Command::Unknown(_) => "UNKNOWN",
        }
    }
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
            in_transaction: false,
            queued_commands: Vec::new(),
            watched_keys: AHashMap::new(),
            script_cache: super::lua::ScriptCache::new(),
            shared_script_cache: None,
        }
    }

    /// Create a new CommandExecutor with a shared script cache
    ///
    /// This is used in multi-shard mode to ensure all shards share
    /// the same script cache, allowing SCRIPT LOAD on any shard to
    /// make scripts available on all shards.
    pub fn with_shared_script_cache(shared_cache: super::lua::SharedScriptCache) -> Self {
        CommandExecutor {
            data: AHashMap::new(),
            expirations: AHashMap::new(),
            current_time: VirtualTime::from_millis(0),
            access_times: AHashMap::new(),
            key_count: 0,
            commands_processed: 0,
            simulation_start_epoch: 0,
            in_transaction: false,
            queued_commands: Vec::new(),
            watched_keys: AHashMap::new(),
            script_cache: super::lua::ScriptCache::new(),
            shared_script_cache: Some(shared_cache),
        }
    }

    /// Set the shared script cache (for updating after creation)
    pub fn set_shared_script_cache(&mut self, shared_cache: super::lua::SharedScriptCache) {
        self.shared_script_cache = Some(shared_cache);
    }

    // Helper methods for script cache operations that check shared cache first

    /// Cache a script and return its SHA1
    fn cache_script_internal(&mut self, script: &str) -> String {
        if let Some(ref shared) = self.shared_script_cache {
            shared.cache_script(script)
        } else {
            self.script_cache.cache_script(script)
        }
    }

    /// Get a script by SHA1
    fn get_script_internal(&self, sha1: &str) -> Option<String> {
        if let Some(ref shared) = self.shared_script_cache {
            shared.get_script(sha1)
        } else {
            self.script_cache.get_script(sha1).cloned()
        }
    }

    /// Check if a script exists
    fn has_script_internal(&self, sha1: &str) -> bool {
        if let Some(ref shared) = self.shared_script_cache {
            shared.has_script(sha1)
        } else {
            self.script_cache.has_script(sha1)
        }
    }

    /// Flush all scripts
    fn flush_scripts_internal(&mut self) {
        if let Some(ref shared) = self.shared_script_cache {
            shared.flush()
        } else {
            self.script_cache.flush()
        }
    }

    pub fn set_simulation_start_epoch(&mut self, epoch: i64) {
        self.simulation_start_epoch = epoch;
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

    fn is_expired(&self, key: &str) -> bool {
        if let Some(expiration) = self.expirations.get(key) {
            *expiration <= self.current_time
        } else {
            false
        }
    }

    /// Fast path GET - avoids Command enum overhead
    /// Used by the fast path in connection handler for ~10-15% improvement
    #[inline]
    pub fn get_direct(&mut self, key: &str) -> RespValue {
        self.commands_processed += 1;
        match self.get_value(key) {
            Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
            Some(_) => RespValue::Error(
                "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
            ),
            None => RespValue::BulkString(None),
        }
    }

    /// Fast path SET - avoids Command enum overhead
    /// Used by the fast path in connection handler for ~10-15% improvement
    ///
    /// Optimizations:
    /// - P0 (opt-single-key-alloc): Single key allocation instead of two
    /// - P1 (opt-static-responses): Static "OK" response via RespValue::ok()
    #[inline]
    pub fn set_direct(&mut self, key: &str, value: &[u8]) -> RespValue {
        self.commands_processed += 1;

        // P0 optimization: Single key allocation instead of two separate to_string() calls
        // Without this, we call key.to_string() twice: once for data, once for access_times
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

        // P1 optimization: Use static response helper
        RespValue::ok()
    }

    /// Direct expiration eviction - call this from TTL manager
    /// Returns the number of keys evicted
    pub fn evict_expired_direct(&mut self, current_time: VirtualTime) -> usize {
        // TigerStyle: Capture pre-state for postcondition verification
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

        // TigerStyle: Postconditions
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
            // No expired keys should remain
            for (_, &exp_time) in &self.expirations {
                debug_assert!(
                    exp_time > self.current_time,
                    "Postcondition: no expired keys should remain after eviction"
                );
            }
        }

        count
    }

    fn evict_expired_keys(&mut self) {
        // TigerStyle: Capture pre-state for postcondition verification
        #[cfg(debug_assertions)]
        let pre_data_len = self.data.len();

        let expired_keys: Vec<String> = self
            .expirations
            .iter()
            .filter(|(_, &exp_time)| exp_time <= self.current_time)
            .map(|(k, _)| k.clone())
            .collect();

        #[cfg(debug_assertions)]
        let evicted_count = expired_keys.len();

        for key in expired_keys {
            self.data.remove(&key);
            self.expirations.remove(&key);
            self.access_times.remove(&key);
        }

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.data.len(),
                pre_data_len.saturating_sub(evicted_count),
                "Postcondition: data size must decrease by evicted count"
            );
            // No expired keys should remain
            for (_, &exp_time) in &self.expirations {
                debug_assert!(
                    exp_time > self.current_time,
                    "Postcondition: no expired keys should remain after eviction"
                );
            }
        }
    }

    /// Execute a read-only command (can be called with just &self for some operations)
    /// Note: This still requires &mut self due to access_times updates, but is semantically read-only
    pub fn execute_read(&mut self, cmd: &Command) -> RespValue {
        debug_assert!(cmd.is_read_only(), "execute_read called with write command");
        self.execute(cmd)
    }

    /// Execute a read-only command without updating access times
    /// This can be used when we only have immutable access to the executor
    pub fn execute_readonly(&self, cmd: &Command) -> RespValue {
        match cmd {
            Command::Get(key) => {
                if self.is_expired(key) {
                    return RespValue::BulkString(None);
                }
                match self.data.get(key) {
                    Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                    Some(_) => RespValue::Error("WRONGTYPE".to_string()),
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
            Command::Ping => RespValue::SimpleString("PONG".to_string()),
            _ => RespValue::Error("ERR command not supported in readonly mode".to_string()),
        }
    }

    /// Get read-only access to the data store
    pub fn get_data(&self) -> &AHashMap<String, Value> {
        &self.data
    }

    fn get_value(&mut self, key: &str) -> Option<&Value> {
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

    fn get_value_mut(&mut self, key: &str) -> Option<&mut Value> {
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

    pub fn execute(&mut self, cmd: &Command) -> RespValue {
        self.commands_processed += 1;

        // Handle command queueing when in transaction
        if self.in_transaction {
            match cmd {
                // These commands are executed immediately even in transaction
                Command::Exec | Command::Discard | Command::Multi => {}
                // All other commands get queued
                _ => {
                    self.queued_commands.push(cmd.clone());
                    return RespValue::SimpleString("QUEUED".to_string());
                }
            }
        }

        match cmd {
            Command::Ping => RespValue::SimpleString("PONG".to_string()),

            Command::Info => {
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

            Command::Get(key) => match self.get_value(key) {
                Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::BulkString(None),
            },

            Command::Set {
                key,
                value,
                ex,
                px,
                nx,
                xx,
                get,
            } => {
                // Validate expiration values
                if let Some(seconds) = ex {
                    if *seconds <= 0 {
                        return RespValue::Error(
                            "ERR invalid expire time in 'set' command".to_string(),
                        );
                    }
                }
                if let Some(millis) = px {
                    if *millis <= 0 {
                        return RespValue::Error(
                            "ERR invalid expire time in 'set' command".to_string(),
                        );
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
                self.data.insert(key.clone(), Value::String(value.clone()));
                self.access_times.insert(key.clone(), self.current_time);

                // Handle expiration
                if let Some(seconds) = ex {
                    let expiration =
                        self.current_time + crate::simulator::Duration::from_secs(*seconds as u64);
                    self.expirations.insert(key.clone(), expiration);
                } else if let Some(millis) = px {
                    let expiration =
                        self.current_time + crate::simulator::Duration::from_millis(*millis as u64);
                    self.expirations.insert(key.clone(), expiration);
                } else {
                    self.expirations.remove(key);
                }

                // Return appropriate response
                if *get {
                    match old_value {
                        Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                        None => RespValue::BulkString(None),
                    }
                } else {
                    RespValue::SimpleString("OK".to_string())
                }
            }

            Command::Del(keys) => {
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

            Command::Exists(keys) => {
                let count = keys
                    .iter()
                    .filter(|k| !self.is_expired(k) && self.data.contains_key(*k))
                    .count();
                RespValue::Integer(count as i64)
            }

            Command::TypeOf(key) => match self.get_value(key) {
                Some(Value::String(_)) => RespValue::SimpleString("string".to_string()),
                Some(Value::List(_)) => RespValue::SimpleString("list".to_string()),
                Some(Value::Set(_)) => RespValue::SimpleString("set".to_string()),
                Some(Value::Hash(_)) => RespValue::SimpleString("hash".to_string()),
                Some(Value::SortedSet(_)) => RespValue::SimpleString("zset".to_string()),
                _ => RespValue::SimpleString("none".to_string()),
            },

            Command::Keys(pattern) => {
                let keys: Vec<RespValue> = self
                    .data
                    .keys()
                    .filter(|k| !self.is_expired(k) && self.matches_glob_pattern(k, pattern))
                    .map(|k| RespValue::BulkString(Some(k.as_bytes().to_vec())))
                    .collect();
                RespValue::Array(Some(keys))
            }

            Command::FlushDb | Command::FlushAll => {
                self.data.clear();
                self.expirations.clear();
                self.access_times.clear();
                RespValue::SimpleString("OK".to_string())
            }

            Command::Expire(key, seconds) => {
                if self.is_expired(key) || !self.data.contains_key(key) {
                    RespValue::Integer(0)
                } else {
                    if *seconds <= 0 {
                        self.data.remove(key);
                        self.expirations.remove(key);
                        self.access_times.remove(key);
                        RespValue::Integer(1)
                    } else {
                        let expiration = self.current_time
                            + crate::simulator::Duration::from_secs(*seconds as u64);
                        self.expirations.insert(key.clone(), expiration);
                        RespValue::Integer(1)
                    }
                }
            }

            Command::ExpireAt(key, timestamp) => {
                if self.is_expired(key) || !self.data.contains_key(key) {
                    RespValue::Integer(0)
                } else {
                    let simulation_relative_secs = *timestamp - self.simulation_start_epoch;
                    if simulation_relative_secs <= 0 {
                        self.data.remove(key);
                        self.expirations.remove(key);
                        self.access_times.remove(key);
                        RespValue::Integer(1)
                    } else {
                        let expiration_millis =
                            (simulation_relative_secs as u64).saturating_mul(1000);
                        if expiration_millis <= self.current_time.as_millis() {
                            self.data.remove(key);
                            self.expirations.remove(key);
                            self.access_times.remove(key);
                            RespValue::Integer(1)
                        } else {
                            let expiration = VirtualTime::from_millis(expiration_millis);
                            self.expirations.insert(key.clone(), expiration);
                            RespValue::Integer(1)
                        }
                    }
                }
            }

            Command::PExpireAt(key, timestamp_millis) => {
                if self.is_expired(key) || !self.data.contains_key(key) {
                    RespValue::Integer(0)
                } else {
                    let simulation_relative_millis =
                        *timestamp_millis - (self.simulation_start_epoch * 1000);
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
                        let expiration =
                            VirtualTime::from_millis(simulation_relative_millis as u64);
                        self.expirations.insert(key.clone(), expiration);
                        RespValue::Integer(1)
                    }
                }
            }

            Command::Ttl(key) => {
                if self.is_expired(key) || !self.data.contains_key(key) {
                    RespValue::Integer(-2)
                } else if let Some(expiration) = self.expirations.get(key) {
                    let remaining_ms =
                        expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
                    let remaining_secs = (remaining_ms / 1000).max(0);
                    RespValue::Integer(remaining_secs)
                } else {
                    RespValue::Integer(-1)
                }
            }

            Command::Pttl(key) => {
                if self.is_expired(key) || !self.data.contains_key(key) {
                    RespValue::Integer(-2)
                } else if let Some(expiration) = self.expirations.get(key) {
                    let remaining =
                        expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
                    RespValue::Integer(remaining.max(0))
                } else {
                    RespValue::Integer(-1)
                }
            }

            Command::Persist(key) => {
                if self.is_expired(key) || !self.data.contains_key(key) {
                    RespValue::Integer(0)
                } else if self.expirations.remove(key).is_some() {
                    RespValue::Integer(1)
                } else {
                    RespValue::Integer(0)
                }
            }

            Command::Incr(key) => self.incr_by_impl(key, 1),

            Command::Decr(key) => self.incr_by_impl(key, -1),

            Command::IncrBy(key, increment) => self.incr_by_impl(key, *increment),

            Command::DecrBy(key, decrement) => {
                // TigerStyle: Use checked_neg() to prevent overflow when decrement == i64::MIN
                match decrement.checked_neg() {
                    Some(negated) => self.incr_by_impl(key, negated),
                    None => RespValue::Error("ERR value is out of range".to_string()),
                }
            }

            Command::Append(key, value) => match self.get_value_mut(key) {
                Some(Value::String(s)) => {
                    s.append(value);
                    RespValue::Integer(s.len() as i64)
                }
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => {
                    let len = value.len();
                    self.data.insert(key.clone(), Value::String(value.clone()));
                    self.access_times.insert(key.clone(), self.current_time);
                    RespValue::Integer(len as i64)
                }
            },

            Command::GetSet(key, value) => {
                let old_value = match self.get_value(key) {
                    Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                    Some(_) => {
                        return RespValue::Error(
                            "WRONGTYPE Operation against a key holding the wrong kind of value"
                                .to_string(),
                        )
                    }
                    None => RespValue::BulkString(None),
                };
                self.data.insert(key.clone(), Value::String(value.clone()));
                self.access_times.insert(key.clone(), self.current_time);
                old_value
            }

            Command::MGet(keys) => {
                let values: Vec<RespValue> = keys
                    .iter()
                    .map(|k| match self.get_value(k) {
                        Some(Value::String(s)) => {
                            RespValue::BulkString(Some(s.as_bytes().to_vec()))
                        }
                        _ => RespValue::BulkString(None),
                    })
                    .collect();
                RespValue::Array(Some(values))
            }

            Command::MSet(pairs) => {
                for (key, value) in pairs {
                    self.data.insert(key.clone(), Value::String(value.clone()));
                    self.access_times.insert(key.clone(), self.current_time);
                }
                RespValue::SimpleString("OK".to_string())
            }

            Command::BatchSet(pairs) => {
                // Optimized batch set - all keys are guaranteed to be on this shard
                for (key, value) in pairs {
                    self.data.insert(key.clone(), Value::String(value.clone()));
                    self.access_times.insert(key.clone(), self.current_time);
                }
                RespValue::SimpleString("OK".to_string())
            }

            Command::BatchGet(keys) => {
                // Optimized batch get - all keys are guaranteed to be on this shard
                // Pre-allocate result vector with capacity
                let mut results = Vec::with_capacity(keys.len());
                for key in keys {
                    let value = self.get_value(key);
                    results.push(match value {
                        Some(Value::String(s)) => {
                            RespValue::BulkString(Some(s.as_bytes().to_vec()))
                        }
                        Some(_) => RespValue::Error(
                            "WRONGTYPE Operation against a key holding the wrong kind of value"
                                .to_string(),
                        ),
                        None => RespValue::BulkString(None),
                    });
                }
                RespValue::Array(Some(results))
            }

            Command::LPush(key, values) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let list = self
                    .data
                    .entry(key.clone())
                    .or_insert_with(|| Value::List(RedisList::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match list {
                    Value::List(l) => {
                        for value in values {
                            l.lpush(value.clone());
                        }
                        RespValue::Integer(l.len() as i64)
                    }
                    _ => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                }
            }

            Command::RPush(key, values) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let list = self
                    .data
                    .entry(key.clone())
                    .or_insert_with(|| Value::List(RedisList::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match list {
                    Value::List(l) => {
                        for value in values {
                            l.rpush(value.clone());
                        }
                        RespValue::Integer(l.len() as i64)
                    }
                    _ => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                }
            }

            Command::LPop(key) => match self.get_value_mut(key) {
                Some(Value::List(l)) => match l.lpop() {
                    Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                    None => RespValue::BulkString(None),
                },
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::BulkString(None),
            },

            Command::RPop(key) => match self.get_value_mut(key) {
                Some(Value::List(l)) => match l.rpop() {
                    Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                    None => RespValue::BulkString(None),
                },
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::BulkString(None),
            },

            Command::LRange(key, start, stop) => match self.get_value(key) {
                Some(Value::List(l)) => {
                    let range = l.range(*start, *stop);
                    let elements: Vec<RespValue> = range
                        .iter()
                        .map(|s| RespValue::BulkString(Some(s.as_bytes().to_vec())))
                        .collect();
                    RespValue::Array(Some(elements))
                }
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::Array(Some(Vec::new())),
            },

            Command::LSet(key, index, value) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                match self.data.get_mut(key) {
                    Some(Value::List(list)) => match list.set(*index, value.clone()) {
                        Ok(()) => {
                            self.access_times.insert(key.clone(), self.current_time);
                            RespValue::SimpleString("OK".to_string())
                        }
                        Err(e) => RespValue::Error(e),
                    },
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Error("ERR no such key".to_string()),
                }
            }

            Command::LTrim(key, start, stop) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                match self.data.get_mut(key) {
                    Some(Value::List(list)) => {
                        list.trim(*start, *stop);
                        self.access_times.insert(key.clone(), self.current_time);
                        // Remove key if list becomes empty
                        if list.is_empty() {
                            self.data.remove(key);
                            self.access_times.remove(key);
                            self.expirations.remove(key);
                        }
                        RespValue::SimpleString("OK".to_string())
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::SimpleString("OK".to_string()), // No-op if key doesn't exist
                }
            }

            Command::RPopLPush(source, dest) => {
                if self.is_expired(source) {
                    self.data.remove(source);
                    self.expirations.remove(source);
                }
                // Pop from source
                let popped = match self.data.get_mut(source) {
                    Some(Value::List(list)) => list.rpop(),
                    Some(_) => {
                        return RespValue::Error(
                            "WRONGTYPE Operation against a key holding the wrong kind of value"
                                .to_string(),
                        )
                    }
                    None => None,
                };

                match popped {
                    Some(value) => {
                        // Remove source if now empty
                        if let Some(Value::List(list)) = self.data.get(source) {
                            if list.is_empty() {
                                self.data.remove(source);
                                self.access_times.remove(source);
                                self.expirations.remove(source);
                            }
                        }

                        // Push to dest
                        if self.is_expired(dest) {
                            self.data.remove(dest);
                            self.expirations.remove(dest);
                        }
                        let dest_list = self
                            .data
                            .entry(dest.clone())
                            .or_insert_with(|| Value::List(RedisList::new()));
                        match dest_list {
                            Value::List(list) => {
                                list.lpush(value.clone());
                                self.access_times.insert(dest.clone(), self.current_time);
                                RespValue::BulkString(Some(value.as_bytes().to_vec()))
                            }
                            _ => RespValue::Error(
                                "WRONGTYPE Operation against a key holding the wrong kind of value"
                                    .to_string(),
                            ),
                        }
                    }
                    None => RespValue::BulkString(None),
                }
            }

            Command::LMove {
                source,
                dest,
                wherefrom,
                whereto,
            } => {
                if self.is_expired(source) {
                    self.data.remove(source);
                    self.expirations.remove(source);
                }
                // Pop from source
                let popped = match self.data.get_mut(source) {
                    Some(Value::List(list)) => {
                        if wherefrom == "LEFT" {
                            list.lpop()
                        } else {
                            list.rpop()
                        }
                    }
                    Some(_) => {
                        return RespValue::Error(
                            "WRONGTYPE Operation against a key holding the wrong kind of value"
                                .to_string(),
                        )
                    }
                    None => None,
                };

                match popped {
                    Some(value) => {
                        // Remove source if now empty
                        if let Some(Value::List(list)) = self.data.get(source) {
                            if list.is_empty() {
                                self.data.remove(source);
                                self.access_times.remove(source);
                                self.expirations.remove(source);
                            }
                        }

                        // Push to dest
                        if self.is_expired(dest) {
                            self.data.remove(dest);
                            self.expirations.remove(dest);
                        }
                        let dest_list = self
                            .data
                            .entry(dest.clone())
                            .or_insert_with(|| Value::List(RedisList::new()));
                        match dest_list {
                            Value::List(list) => {
                                if whereto == "LEFT" {
                                    list.lpush(value.clone());
                                } else {
                                    list.rpush(value.clone());
                                }
                                self.access_times.insert(dest.clone(), self.current_time);
                                RespValue::BulkString(Some(value.as_bytes().to_vec()))
                            }
                            _ => RespValue::Error(
                                "WRONGTYPE Operation against a key holding the wrong kind of value"
                                    .to_string(),
                            ),
                        }
                    }
                    None => RespValue::BulkString(None),
                }
            }

            Command::SAdd(key, members) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let set = self
                    .data
                    .entry(key.clone())
                    .or_insert_with(|| Value::Set(RedisSet::new()));
                self.access_times.insert(key.clone(), self.current_time);
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
                    _ => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                }
            }

            Command::SMembers(key) => match self.get_value(key) {
                Some(Value::Set(s)) => {
                    let members: Vec<RespValue> = s
                        .members()
                        .iter()
                        .map(|m| RespValue::BulkString(Some(m.as_bytes().to_vec())))
                        .collect();
                    RespValue::Array(Some(members))
                }
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::Array(Some(Vec::new())),
            },

            Command::SIsMember(key, member) => match self.get_value(key) {
                Some(Value::Set(s)) => RespValue::Integer(if s.contains(member) { 1 } else { 0 }),
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::Integer(0),
            },

            Command::HSet(key, pairs) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let hash = self
                    .data
                    .entry(key.clone())
                    .or_insert_with(|| Value::Hash(RedisHash::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match hash {
                    Value::Hash(h) => {
                        let mut new_fields = 0i64;
                        for (field, value) in pairs {
                            if !h.exists(field) {
                                new_fields += 1;
                            }
                            h.set(field.clone(), value.clone());
                        }
                        RespValue::Integer(new_fields)
                    }
                    _ => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                }
            }

            Command::HGet(key, field) => match self.get_value(key) {
                Some(Value::Hash(h)) => match h.get(field) {
                    Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                    None => RespValue::BulkString(None),
                },
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::BulkString(None),
            },

            Command::HGetAll(key) => {
                match self.get_value(key) {
                    Some(Value::Hash(h)) => {
                        // Pre-allocate capacity: each field has key and value (Abseil Tip #19)
                        let mut elements = Vec::with_capacity(h.len() * 2);
                        for (k, v) in h.get_all() {
                            elements.push(RespValue::BulkString(Some(k.as_bytes().to_vec())));
                            elements.push(RespValue::BulkString(Some(v.as_bytes().to_vec())));
                        }
                        RespValue::Array(Some(elements))
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Array(Some(Vec::new())),
                }
            }

            Command::HIncrBy(key, field, increment) => {
                // Handle expiration first
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                    self.access_times.remove(key);
                }

                // Check if key exists and is wrong type before inserting
                if let Some(existing) = self.data.get(key) {
                    if !matches!(existing, Value::Hash(_)) {
                        return RespValue::Error(
                            "WRONGTYPE Operation against a key holding the wrong kind of value"
                                .to_string(),
                        );
                    }
                }

                let hash = self
                    .data
                    .entry(key.clone())
                    .or_insert_with(|| Value::Hash(RedisHash::new()));
                self.access_times.insert(key.clone(), self.current_time);

                match hash {
                    Value::Hash(h) => {
                        // Get current value, parse as i64, return error if not an integer
                        let current: i64 = match h.get(field) {
                            Some(v) => {
                                let s = v.to_string();
                                match s.parse::<i64>() {
                                    Ok(n) => n,
                                    Err(_) => {
                                        return RespValue::Error(
                                            "ERR hash value is not an integer".to_string(),
                                        )
                                    }
                                }
                            }
                            None => 0,
                        };

                        // TigerStyle: Use checked arithmetic to detect overflow
                        let new_value = match current.checked_add(*increment) {
                            Some(v) => v,
                            None => {
                                return RespValue::Error(
                                    "ERR increment or decrement would overflow".to_string(),
                                )
                            }
                        };

                        h.set(field.clone(), SDS::from_str(&new_value.to_string()));

                        // TigerStyle: Assert invariants after mutation
                        debug_assert!(
                            h.get(field).is_some(),
                            "Invariant violated: field must exist after HINCRBY"
                        );
                        debug_assert_eq!(
                            h.get(field).map(|v| v.to_string().parse::<i64>().ok()).flatten(),
                            Some(new_value),
                            "Invariant violated: field value must equal computed value after HINCRBY"
                        );

                        RespValue::Integer(new_value)
                    }
                    _ => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                }
            }

            Command::ZAdd {
                key,
                pairs,
                nx,
                xx,
                gt,
                lt,
                ch,
            } => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let zset = self
                    .data
                    .entry(key.clone())
                    .or_insert_with(|| Value::SortedSet(RedisSortedSet::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match zset {
                    Value::SortedSet(zs) => {
                        let mut added = 0;
                        let mut changed = 0;
                        for (score, member) in pairs {
                            let exists = zs.score(&member).is_some();
                            let current_score = zs.score(&member);

                            // NX: only add new elements
                            if *nx && exists {
                                continue;
                            }
                            // XX: only update existing elements
                            if *xx && !exists {
                                continue;
                            }
                            // GT: only update when new score > current
                            if *gt && exists {
                                if let Some(cs) = current_score {
                                    if *score <= cs {
                                        continue;
                                    }
                                }
                            }
                            // LT: only update when new score < current
                            if *lt && exists {
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
                        if *ch {
                            RespValue::Integer(changed)
                        } else {
                            RespValue::Integer(added)
                        }
                    }
                    _ => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                }
            }

            Command::ZRange(key, start, stop) => match self.get_value(key) {
                Some(Value::SortedSet(zs)) => {
                    let range = zs.range(*start, *stop);
                    let elements: Vec<RespValue> = range
                        .iter()
                        .map(|(m, _)| RespValue::BulkString(Some(m.as_bytes().to_vec())))
                        .collect();
                    RespValue::Array(Some(elements))
                }
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::Array(Some(Vec::new())),
            },

            Command::ZRevRange(key, start, stop, with_scores) => match self.get_value(key) {
                Some(Value::SortedSet(zs)) => {
                    let range = zs.rev_range(*start, *stop);
                    if *with_scores {
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
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::Array(Some(Vec::new())),
            },

            Command::ZScore(key, member) => match self.get_value(key) {
                Some(Value::SortedSet(zs)) => match zs.score(member) {
                    Some(score) => {
                        let score_str = score.to_string();
                        RespValue::BulkString(Some(score_str.into_bytes()))
                    }
                    None => RespValue::BulkString(None),
                },
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::BulkString(None),
            },

            // === NEW COMMANDS (TigerStyle + VOPR) ===
            Command::StrLen(key) => {
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
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::LLen(key) => {
                match self.get_value(key) {
                    Some(Value::List(l)) => {
                        let len = l.len() as i64;
                        // TigerStyle: Postcondition - length must be non-negative
                        debug_assert!(
                            len >= 0,
                            "Invariant violated: LLEN must return non-negative"
                        );
                        RespValue::Integer(len)
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::LIndex(key, index) => {
                match self.get_value(key) {
                    Some(Value::List(l)) => {
                        let len = l.len() as isize;
                        // TigerStyle: Handle negative indices (Redis convention)
                        let actual_index = if *index < 0 {
                            let normalized = len + *index;
                            if normalized < 0 {
                                return RespValue::BulkString(None);
                            }
                            normalized as usize
                        } else if *index >= len {
                            return RespValue::BulkString(None);
                        } else {
                            *index as usize
                        };

                        // TigerStyle: Precondition verified
                        debug_assert!(
                            actual_index < l.len(),
                            "Invariant violated: index must be in bounds"
                        );

                        let range = l.range(actual_index as isize, actual_index as isize);
                        if let Some(item) = range.first() {
                            RespValue::BulkString(Some(item.as_bytes().to_vec()))
                        } else {
                            RespValue::BulkString(None)
                        }
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::BulkString(None),
                }
            }

            Command::SRem(key, members) => {
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
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::SCard(key) => {
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
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::HDel(key, fields) => {
                match self.get_value_mut(key) {
                    Some(Value::Hash(h)) => {
                        // TigerStyle: Capture pre-state for postcondition
                        #[cfg(debug_assertions)]
                        let pre_len = h.len();

                        let mut deleted = 0i64;
                        for field in fields {
                            if h.delete(field) {
                                deleted += 1;
                            }
                        }

                        // TigerStyle: Postconditions
                        #[cfg(debug_assertions)]
                        {
                            debug_assert!(
                                deleted >= 0,
                                "Invariant violated: deleted count must be non-negative"
                            );
                            debug_assert!(
                                deleted <= fields.len() as i64,
                                "Invariant violated: can't delete more than requested"
                            );
                            debug_assert_eq!(
                                h.len(),
                                pre_len - deleted as usize,
                                "Invariant violated: len must decrease by deleted count"
                            );
                        }

                        RespValue::Integer(deleted)
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::HKeys(key) => {
                match self.get_value(key) {
                    Some(Value::Hash(h)) => {
                        let hash_keys = h.keys();
                        // TigerStyle: Postcondition - keys count must equal len
                        debug_assert_eq!(
                            hash_keys.len(),
                            h.len(),
                            "Invariant violated: HKEYS count must equal HLEN"
                        );

                        let keys: Vec<RespValue> = hash_keys
                            .iter()
                            .map(|k| RespValue::BulkString(Some(k.as_bytes().to_vec())))
                            .collect();
                        RespValue::Array(Some(keys))
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Array(Some(Vec::new())),
                }
            }

            Command::HVals(key) => {
                match self.get_value(key) {
                    Some(Value::Hash(h)) => {
                        let hash_vals = h.values();
                        // TigerStyle: Postcondition - values count must equal len
                        debug_assert_eq!(
                            hash_vals.len(),
                            h.len(),
                            "Invariant violated: HVALS count must equal HLEN"
                        );

                        let vals: Vec<RespValue> = hash_vals
                            .iter()
                            .map(|v| RespValue::BulkString(Some(v.as_bytes().to_vec())))
                            .collect();
                        RespValue::Array(Some(vals))
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Array(Some(Vec::new())),
                }
            }

            Command::HLen(key) => {
                match self.get_value(key) {
                    Some(Value::Hash(h)) => {
                        let len = h.len() as i64;
                        // TigerStyle: Postcondition
                        debug_assert!(
                            len >= 0,
                            "Invariant violated: HLEN must return non-negative"
                        );
                        RespValue::Integer(len)
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::HExists(key, field) => {
                match self.get_value(key) {
                    Some(Value::Hash(h)) => {
                        let exists = h.exists(field);
                        // TigerStyle: Postcondition - result must be 0 or 1
                        let result = if exists { 1i64 } else { 0i64 };
                        debug_assert!(
                            result == 0 || result == 1,
                            "Invariant violated: HEXISTS must return 0 or 1"
                        );
                        RespValue::Integer(result)
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::ZRem(key, members) => {
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
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::ZRank(key, member) => {
                match self.get_value(key) {
                    Some(Value::SortedSet(zs)) => {
                        match zs.rank(member) {
                            Some(rank) => {
                                // TigerStyle: Postcondition - rank must be valid index
                                debug_assert!(
                                    rank < zs.len(),
                                    "Invariant violated: rank must be less than zset length"
                                );
                                RespValue::Integer(rank as i64)
                            }
                            None => RespValue::BulkString(None),
                        }
                    }
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::BulkString(None),
                }
            }

            Command::ZCard(key) => {
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
                    Some(_) => RespValue::Error(
                        "WRONGTYPE Operation against a key holding the wrong kind of value"
                            .to_string(),
                    ),
                    None => RespValue::Integer(0),
                }
            }

            Command::ZCount(key, min, max) => match self.get_value(key) {
                Some(Value::SortedSet(zs)) => match zs.count_in_range(min, max) {
                    Ok(count) => RespValue::Integer(count as i64),
                    Err(e) => RespValue::Error(e),
                },
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::Integer(0),
            },

            Command::ZRangeByScore {
                key,
                min,
                max,
                with_scores,
                limit,
            } => match self.get_value(key) {
                Some(Value::SortedSet(zs)) => {
                    match zs.range_by_score(min, max, *with_scores, *limit) {
                        Ok(results) => {
                            let elements: Vec<RespValue> = results
                                .iter()
                                .flat_map(|(member, score)| {
                                    let mut v = vec![RespValue::BulkString(Some(
                                        member.as_bytes().to_vec(),
                                    ))];
                                    if let Some(s) = score {
                                        v.push(RespValue::BulkString(Some(
                                            s.to_string().into_bytes(),
                                        )));
                                    }
                                    v
                                })
                                .collect();
                            RespValue::Array(Some(elements))
                        }
                        Err(e) => RespValue::Error(e),
                    }
                }
                Some(_) => RespValue::Error(
                    "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
                ),
                None => RespValue::Array(Some(vec![])),
            },

            Command::Scan {
                cursor,
                pattern,
                count,
            } => {
                let count = count.unwrap_or(10);
                // Collect all non-expired keys
                let mut keys: Vec<String> = self
                    .data
                    .keys()
                    .filter(|k| !self.is_expired(k))
                    .filter(|k| {
                        pattern
                            .as_ref()
                            .map_or(true, |p| self.matches_glob_pattern(k, p))
                    })
                    .cloned()
                    .collect();
                // Sort for deterministic iteration
                keys.sort();

                // Skip to cursor position and take count+1 to know if there's more
                let results: Vec<String> = keys
                    .into_iter()
                    .skip(*cursor as usize)
                    .take(count + 1)
                    .collect();

                let (next_cursor, result_keys) = if results.len() > count {
                    (*cursor + count as u64, &results[..count])
                } else {
                    (0u64, &results[..])
                };

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

            Command::HScan {
                key,
                cursor,
                pattern,
                count,
            } => {
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
                        return RespValue::Error(
                            "WRONGTYPE Operation against a key holding the wrong kind of value"
                                .to_string(),
                        )
                    }
                    None => None,
                };

                match raw_fields {
                    Some(all_fields) => {
                        let count = count.unwrap_or(10);
                        // Filter by pattern
                        let mut fields: Vec<(String, String)> = all_fields
                            .into_iter()
                            .filter(|(f, _)| {
                                pattern
                                    .as_ref()
                                    .map_or(true, |p| self.matches_glob_pattern(f, p))
                            })
                            .collect();
                        // Sort for deterministic iteration
                        fields.sort_by(|a, b| a.0.cmp(&b.0));

                        // Skip to cursor position and take count+1
                        let results: Vec<(String, String)> = fields
                            .into_iter()
                            .skip(*cursor as usize)
                            .take(count + 1)
                            .collect();

                        let (next_cursor, result_fields) = if results.len() > count {
                            (*cursor + count as u64, &results[..count])
                        } else {
                            (0u64, &results[..])
                        };

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

            Command::ZScan {
                key,
                cursor,
                pattern,
                count,
            } => {
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
                        return RespValue::Error(
                            "WRONGTYPE Operation against a key holding the wrong kind of value"
                                .to_string(),
                        )
                    }
                    None => None,
                };

                match raw_members {
                    Some(all_members) => {
                        let count = count.unwrap_or(10);
                        // Filter by pattern
                        let mut members: Vec<(String, f64)> = all_members
                            .into_iter()
                            .filter(|(m, _)| {
                                pattern
                                    .as_ref()
                                    .map_or(true, |p| self.matches_glob_pattern(m, p))
                            })
                            .collect();
                        // Sort by member for deterministic iteration
                        members.sort_by(|a, b| a.0.cmp(&b.0));

                        // Skip to cursor position and take count+1
                        let results: Vec<(String, f64)> = members
                            .into_iter()
                            .skip(*cursor as usize)
                            .take(count + 1)
                            .collect();

                        let (next_cursor, result_members) = if results.len() > count {
                            (*cursor + count as u64, &results[..count])
                        } else {
                            (0u64, &results[..])
                        };

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

            Command::Multi => {
                if self.in_transaction {
                    return RespValue::Error("ERR MULTI calls can not be nested".to_string());
                }
                self.in_transaction = true;
                self.queued_commands.clear();
                RespValue::SimpleString("OK".to_string())
            }

            Command::Exec => {
                if !self.in_transaction {
                    return RespValue::Error("ERR EXEC without MULTI".to_string());
                }

                // Check if any watched keys have changed
                let watch_violated = self.watched_keys.iter().any(|(key, original_value)| {
                    let current_value = self.data.get(key).cloned();
                    &current_value != original_value
                });

                // Clear transaction state
                self.in_transaction = false;
                let commands = std::mem::take(&mut self.queued_commands);
                self.watched_keys.clear();

                if watch_violated {
                    // WATCH detected a change, abort the transaction
                    return RespValue::BulkString(None);
                }

                // Execute all queued commands
                let results: Vec<RespValue> =
                    commands.into_iter().map(|cmd| self.execute(&cmd)).collect();

                RespValue::Array(Some(results))
            }

            Command::Discard => {
                if !self.in_transaction {
                    return RespValue::Error("ERR DISCARD without MULTI".to_string());
                }
                self.in_transaction = false;
                self.queued_commands.clear();
                self.watched_keys.clear();
                RespValue::SimpleString("OK".to_string())
            }

            Command::Watch(keys) => {
                if self.in_transaction {
                    return RespValue::Error("ERR WATCH inside MULTI is not allowed".to_string());
                }
                // Store current values of watched keys
                for key in keys {
                    let current_value = self.data.get(key).cloned();
                    self.watched_keys.insert(key.clone(), current_value);
                }
                RespValue::SimpleString("OK".to_string())
            }

            Command::Unwatch => {
                self.watched_keys.clear();
                RespValue::SimpleString("OK".to_string())
            }

            Command::Eval { script, keys, args } => {
                #[cfg(feature = "lua")]
                {
                    self.execute_lua_script(script, keys, args)
                }
                #[cfg(not(feature = "lua"))]
                {
                    let _ = (script, keys, args);
                    RespValue::Error("ERR Lua scripting not compiled in".to_string())
                }
            }

            Command::EvalSha { sha1, keys, args } => {
                #[cfg(feature = "lua")]
                {
                    // Look up script in cache (uses shared cache if available)
                    match self.get_script_internal(sha1) {
                        Some(script) => {
                            let script = script.clone();
                            self.execute_lua_script(&script, keys, args)
                        }
                        None => RespValue::Error(
                            "NOSCRIPT No matching script. Please use EVAL.".to_string(),
                        ),
                    }
                }
                #[cfg(not(feature = "lua"))]
                {
                    let _ = (sha1, keys, args);
                    RespValue::Error("ERR Lua scripting not compiled in".to_string())
                }
            }

            Command::ScriptLoad(script) => {
                #[cfg(feature = "lua")]
                {
                    let sha1 = self.cache_script_internal(&script);
                    RespValue::BulkString(Some(sha1.into_bytes()))
                }
                #[cfg(not(feature = "lua"))]
                {
                    let _ = script;
                    RespValue::Error("ERR Lua scripting not compiled in".to_string())
                }
            }

            Command::ScriptExists(sha1s) => {
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
                    RespValue::Error("ERR Lua scripting not compiled in".to_string())
                }
            }

            Command::ScriptFlush => {
                #[cfg(feature = "lua")]
                {
                    self.flush_scripts_internal();
                    RespValue::SimpleString("OK".to_string())
                }
                #[cfg(not(feature = "lua"))]
                {
                    RespValue::Error("ERR Lua scripting not compiled in".to_string())
                }
            }

            // DBSIZE - returns number of keys
            Command::DbSize => {
                let count = self.data.len() as i64;
                RespValue::Integer(count)
            }

            // AUTH/ACL commands - these are handled at the connection level, not here
            // Return errors indicating they should be handled elsewhere
            Command::Auth { .. } => {
                // AUTH is handled at the connection level
                // If we get here, it means the server doesn't have ACL enabled
                RespValue::SimpleString("OK".to_string())
            }

            Command::AclWhoami => {
                // Without ACL feature, default user is always authenticated
                RespValue::BulkString(Some(b"default".to_vec()))
            }

            Command::AclList => {
                // Return default user rule
                let rule = "user default on nopass ~* +@all".to_string();
                RespValue::Array(Some(vec![RespValue::BulkString(Some(rule.into_bytes()))]))
            }

            Command::AclUsers => {
                RespValue::Array(Some(vec![RespValue::BulkString(Some(b"default".to_vec()))]))
            }

            Command::AclGetUser { username } => {
                if username == "default" {
                    // Return info about default user
                    RespValue::Array(Some(vec![
                        RespValue::BulkString(Some(b"flags".to_vec())),
                        RespValue::Array(Some(vec![
                            RespValue::BulkString(Some(b"on".to_vec())),
                            RespValue::BulkString(Some(b"nopass".to_vec())),
                        ])),
                        RespValue::BulkString(Some(b"passwords".to_vec())),
                        RespValue::Array(Some(vec![])),
                        RespValue::BulkString(Some(b"commands".to_vec())),
                        RespValue::BulkString(Some(b"+@all".to_vec())),
                        RespValue::BulkString(Some(b"keys".to_vec())),
                        RespValue::BulkString(Some(b"~*".to_vec())),
                    ]))
                } else {
                    RespValue::BulkString(None) // User not found
                }
            }

            Command::AclSetUser { .. } => {
                // ACL management requires the ACL feature
                RespValue::Error("ERR ACL feature not enabled".to_string())
            }

            Command::AclDelUser { .. } => {
                RespValue::Error("ERR ACL feature not enabled".to_string())
            }

            Command::AclCat { category } => {
                // Return command categories or commands in a category
                #[cfg(feature = "acl")]
                {
                    use crate::security::acl::CommandCategory;
                    match category {
                        None => {
                            // List all categories
                            let categories = vec![
                                "read", "write", "admin", "dangerous", "keyspace",
                                "string", "list", "set", "hash", "sortedset",
                                "connection", "server", "scripting", "transaction",
                            ];
                            RespValue::Array(Some(
                                categories
                                    .into_iter()
                                    .map(|c| RespValue::BulkString(Some(c.as_bytes().to_vec())))
                                    .collect(),
                            ))
                        }
                        Some(cat) => {
                            // List commands in category
                            if let Some(cat_enum) = CommandCategory::from_str(cat) {
                                let commands: Vec<RespValue> = cat_enum
                                    .commands()
                                    .iter()
                                    .map(|c| RespValue::BulkString(Some(c.to_lowercase().into_bytes())))
                                    .collect();
                                RespValue::Array(Some(commands))
                            } else {
                                RespValue::Error(format!("ERR Unknown ACL category '{}'", cat))
                            }
                        }
                    }
                }
                #[cfg(not(feature = "acl"))]
                {
                    let _ = category; // Suppress unused warning
                    // Without ACL feature, return basic category list
                    let categories = vec![
                        "read", "write", "admin", "dangerous", "keyspace",
                        "string", "list", "set", "hash", "sortedset",
                        "connection", "server", "scripting", "transaction",
                    ];
                    RespValue::Array(Some(
                        categories
                            .into_iter()
                            .map(|c| RespValue::BulkString(Some(c.as_bytes().to_vec())))
                            .collect(),
                    ))
                }
            }

            Command::AclGenPass { bits } => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let bits = bits.unwrap_or(256).min(1024);
                let bytes = (bits as usize + 7) / 8;
                let seed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let mut result = String::with_capacity(bytes * 2);
                let mut state = seed;
                for _ in 0..bytes {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let byte = ((state >> 33) & 0xFF) as u8;
                    result.push_str(&format!("{:02x}", byte));
                }
                RespValue::BulkString(Some(result.into_bytes()))
            }

            Command::Unknown(cmd) => RespValue::Error(format!("ERR unknown command '{}'", cmd)),
        }
    }

    /// Execute a Lua script with KEYS and ARGV
    ///
    /// TigerStyle: This function executes Redis commands immediately during Lua execution,
    /// matching real Redis semantics. math.random is seeded deterministically for DST.
    #[cfg(feature = "lua")]
    fn execute_lua_script(&mut self, script: &str, keys: &[String], args: &[SDS]) -> RespValue {
        use mlua::{Lua, MultiValue, Result as LuaResult, Value as LuaValue};
        use std::cell::RefCell;

        // TigerStyle: Preconditions
        debug_assert!(!script.is_empty(), "Precondition: script must not be empty");

        // Track initial state for postcondition verification
        #[cfg(debug_assertions)]
        let _initial_key_count = self.data.len();

        // Cache the script for EVALSHA (uses shared cache if available)
        #[allow(unused_variables)]
        let script_sha = self.cache_script_internal(script);

        // Create a new Lua instance for this execution
        let lua = Lua::new();

        // DST: Seed math.random deterministically using current_time
        // This ensures Lua scripts produce deterministic results for simulation testing
        let seed = self.current_time.as_millis();
        if let Err(e) = lua.load(&format!("math.randomseed({})", seed)).exec() {
            return RespValue::Error(format!("ERR Failed to seed RNG: {}", e));
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
            return RespValue::Error(format!("ERR Lua sandbox error: {}", e));
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
            return RespValue::Error(format!("ERR Failed to set KEYS: {}", e));
        }

        // Create ARGV table - use binary-safe Lua strings to preserve protobuf/binary data
        if let Err(e) = (|| -> LuaResult<()> {
            let argv_table = lua.create_table()?;
            for (i, arg) in args.iter().enumerate() {
                // Use lua.create_string() which is binary-safe, not to_string() which uses UTF-8 lossy
                let lua_str = lua.create_string(arg.as_bytes())?;
                argv_table.set(i + 1, lua_str)?;
            }
            lua.globals().set("ARGV", argv_table)?;
            Ok(())
        })() {
            return RespValue::Error(format!("ERR Failed to set ARGV: {}", e));
        }

        // Use RefCell to allow mutable borrow from within Lua callbacks
        // This enables immediate command execution with results returned to Lua
        // Note: We reborrow self to allow using self again after the scope
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
                            return Err(mlua::Error::RuntimeError(e.clone()));
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
                            err_table.set("err", e.as_str())?;
                            return Ok(LuaValue::Table(err_table));
                        }
                        Self::resp_to_lua_value(lua, resp)
                    }
                    Err(e) => {
                        // Return error as table for pcall
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

        // Convert result - use a separate method call to convert Lua result
        let resp = match result {
            Ok(lua_value) => self.lua_to_resp(&lua, lua_value),
            Err(e) => RespValue::Error(format!("ERR {}", e)),
        };

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            // Verify script was cached
            debug_assert!(
                self.has_script_internal(&script_sha),
                "Postcondition: script must be cached after execution"
            );

            // Verify data integrity - key count should be reasonable
            debug_assert!(
                self.data.len() <= 1_000_000_000,
                "Postcondition: data count must be reasonable (< 1B keys)"
            );
        }

        resp
    }

    /// Parse MultiValue arguments to bytes for redis.call/pcall
    /// Uses binary-safe handling to preserve protobuf and other binary data
    #[cfg(feature = "lua")]
    fn parse_multivalue_to_bytes(args: mlua::MultiValue) -> mlua::Result<Vec<Vec<u8>>> {
        use mlua::Value as LuaValue;

        let mut cmd_parts = Vec::new();
        for arg in args {
            match arg {
                // Use as_bytes() instead of to_str() to preserve binary data
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

    /// Convert RespValue to Lua Value for returning results to Lua scripts
    #[cfg(feature = "lua")]
    fn resp_to_lua_value(lua: &mlua::Lua, resp: RespValue) -> mlua::Result<mlua::Value> {
        use mlua::Value as LuaValue;

        Ok(match resp {
            RespValue::SimpleString(s) => {
                // Return as {ok = "message"} table for status replies
                let t = lua.create_table()?;
                t.set("ok", s.as_str())?;
                LuaValue::Table(t)
            }
            RespValue::Error(e) => {
                // Should not reach here for redis.call (errors propagate)
                // For pcall, caller handles this
                let t = lua.create_table()?;
                t.set("err", e.as_str())?;
                LuaValue::Table(t)
            }
            RespValue::Integer(i) => LuaValue::Integer(i),
            RespValue::BulkString(Some(bytes)) => {
                // Use bytes directly - create_string is binary-safe in Lua
                LuaValue::String(lua.create_string(&bytes)?)
            }
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

    /// Parse a command from Lua script arguments
    #[cfg(feature = "lua")]
    fn parse_lua_command(&self, parts: &[String]) -> Result<Command, String> {
        if parts.is_empty() {
            return Err("Empty command".to_string());
        }

        let cmd_name = parts[0].to_uppercase();
        let args = &parts[1..];

        match cmd_name.as_str() {
            "GET" => {
                if args.len() != 1 {
                    return Err("GET requires 1 argument".to_string());
                }
                Ok(Command::Get(args[0].clone()))
            }
            "SET" => {
                if args.len() < 2 {
                    return Err("SET requires at least 2 arguments".to_string());
                }
                let key = args[0].clone();
                let value = SDS::from_str(&args[1]);
                let mut ex = None;
                let mut px = None;
                let mut nx = false;
                let mut xx = false;
                let mut get = false;

                let mut i = 2;
                while i < args.len() {
                    let opt = args[i].to_uppercase();
                    match opt.as_str() {
                        "NX" => nx = true,
                        "XX" => xx = true,
                        "GET" => get = true,
                        "EX" => {
                            i += 1;
                            if i >= args.len() {
                                return Err("SET EX requires value".to_string());
                            }
                            ex = Some(args[i].parse().map_err(|_| "SET EX must be integer")?);
                        }
                        "PX" => {
                            i += 1;
                            if i >= args.len() {
                                return Err("SET PX requires value".to_string());
                            }
                            px = Some(args[i].parse().map_err(|_| "SET PX must be integer")?);
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
                Ok(Command::Del(args.iter().cloned().collect()))
            }
            "INCR" => {
                if args.len() != 1 {
                    return Err("INCR requires 1 argument".to_string());
                }
                Ok(Command::Incr(args[0].clone()))
            }
            "DECR" => {
                if args.len() != 1 {
                    return Err("DECR requires 1 argument".to_string());
                }
                Ok(Command::Decr(args[0].clone()))
            }
            "INCRBY" => {
                if args.len() != 2 {
                    return Err("INCRBY requires 2 arguments".to_string());
                }
                let incr: i64 = args[1]
                    .parse()
                    .map_err(|_| "INCRBY increment must be integer".to_string())?;
                Ok(Command::IncrBy(args[0].clone(), incr))
            }
            "HGET" => {
                if args.len() != 2 {
                    return Err("HGET requires 2 arguments".to_string());
                }
                Ok(Command::HGet(args[0].clone(), SDS::from_str(&args[1])))
            }
            "HSET" => {
                if args.len() < 3 || args.len() % 2 == 0 {
                    return Err("HSET requires key and field-value pairs".to_string());
                }
                let key = args[0].clone();
                let pairs: Vec<(SDS, SDS)> = args[1..]
                    .chunks(2)
                    .map(|chunk| (SDS::from_str(&chunk[0]), SDS::from_str(&chunk[1])))
                    .collect();
                Ok(Command::HSet(key, pairs))
            }
            "HDEL" => {
                if args.len() < 2 {
                    return Err("HDEL requires key and at least 1 field".to_string());
                }
                let key = args[0].clone();
                let fields: Vec<SDS> = args[1..].iter().map(|s| SDS::from_str(s)).collect();
                Ok(Command::HDel(key, fields))
            }
            "HINCRBY" => {
                if args.len() != 3 {
                    return Err("HINCRBY requires 3 arguments".to_string());
                }
                let incr: i64 = args[2]
                    .parse()
                    .map_err(|_| "HINCRBY increment must be integer".to_string())?;
                Ok(Command::HIncrBy(
                    args[0].clone(),
                    SDS::from_str(&args[1]),
                    incr,
                ))
            }
            "LPUSH" => {
                if args.len() < 2 {
                    return Err("LPUSH requires key and at least 1 value".to_string());
                }
                let key = args[0].clone();
                let values: Vec<SDS> = args[1..].iter().map(|s| SDS::from_str(s)).collect();
                Ok(Command::LPush(key, values))
            }
            "RPUSH" => {
                if args.len() < 2 {
                    return Err("RPUSH requires key and at least 1 value".to_string());
                }
                let key = args[0].clone();
                let values: Vec<SDS> = args[1..].iter().map(|s| SDS::from_str(s)).collect();
                Ok(Command::RPush(key, values))
            }
            "LPOP" => {
                if args.len() != 1 {
                    return Err("LPOP requires 1 argument".to_string());
                }
                Ok(Command::LPop(args[0].clone()))
            }
            "RPOP" => {
                if args.len() != 1 {
                    return Err("RPOP requires 1 argument".to_string());
                }
                Ok(Command::RPop(args[0].clone()))
            }
            "LLEN" => {
                if args.len() != 1 {
                    return Err("LLEN requires 1 argument".to_string());
                }
                Ok(Command::LLen(args[0].clone()))
            }
            "LRANGE" => {
                if args.len() != 3 {
                    return Err("LRANGE requires 3 arguments".to_string());
                }
                let start: isize = args[1]
                    .parse()
                    .map_err(|_| "LRANGE start must be integer".to_string())?;
                let stop: isize = args[2]
                    .parse()
                    .map_err(|_| "LRANGE stop must be integer".to_string())?;
                Ok(Command::LRange(args[0].clone(), start, stop))
            }
            "RPOPLPUSH" => {
                if args.len() != 2 {
                    return Err("RPOPLPUSH requires 2 arguments".to_string());
                }
                Ok(Command::RPopLPush(args[0].clone(), args[1].clone()))
            }
            "SADD" => {
                if args.len() < 2 {
                    return Err("SADD requires key and at least 1 member".to_string());
                }
                let key = args[0].clone();
                let members: Vec<SDS> = args[1..].iter().map(|s| SDS::from_str(s)).collect();
                Ok(Command::SAdd(key, members))
            }
            "SREM" => {
                if args.len() < 2 {
                    return Err("SREM requires key and at least 1 member".to_string());
                }
                let key = args[0].clone();
                let members: Vec<SDS> = args[1..].iter().map(|s| SDS::from_str(s)).collect();
                Ok(Command::SRem(key, members))
            }
            "SMEMBERS" => {
                if args.len() != 1 {
                    return Err("SMEMBERS requires 1 argument".to_string());
                }
                Ok(Command::SMembers(args[0].clone()))
            }
            "SISMEMBER" => {
                if args.len() != 2 {
                    return Err("SISMEMBER requires 2 arguments".to_string());
                }
                Ok(Command::SIsMember(args[0].clone(), SDS::from_str(&args[1])))
            }
            "ZADD" => {
                if args.len() < 3 {
                    return Err("ZADD requires key and score-member pairs".to_string());
                }
                let key = args[0].clone();

                // Parse optional flags
                let mut nx = false;
                let mut xx = false;
                let mut gt = false;
                let mut lt = false;
                let mut ch = false;
                let mut i = 1;

                while i < args.len() {
                    let opt = args[i].to_uppercase();
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
                    let score: f64 = args[i]
                        .parse()
                        .map_err(|_| "ZADD score must be a number".to_string())?;
                    pairs.push((score, SDS::from_str(&args[i + 1])));
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
                let key = args[0].clone();
                let members: Vec<SDS> = args[1..].iter().map(|s| SDS::from_str(s)).collect();
                Ok(Command::ZRem(key, members))
            }
            "ZRANGE" => {
                if args.len() != 3 {
                    return Err("ZRANGE requires 3 arguments".to_string());
                }
                let start: isize = args[1]
                    .parse()
                    .map_err(|_| "ZRANGE start must be integer".to_string())?;
                let stop: isize = args[2]
                    .parse()
                    .map_err(|_| "ZRANGE stop must be integer".to_string())?;
                Ok(Command::ZRange(args[0].clone(), start, stop))
            }
            "ZSCORE" => {
                if args.len() != 2 {
                    return Err("ZSCORE requires 2 arguments".to_string());
                }
                Ok(Command::ZScore(args[0].clone(), SDS::from_str(&args[1])))
            }
            "ZCARD" => {
                if args.len() != 1 {
                    return Err("ZCARD requires 1 argument".to_string());
                }
                Ok(Command::ZCard(args[0].clone()))
            }
            "ZCOUNT" => {
                if args.len() != 3 {
                    return Err("ZCOUNT requires 3 arguments".to_string());
                }
                Ok(Command::ZCount(
                    args[0].clone(),
                    args[1].clone(),
                    args[2].clone(),
                ))
            }
            "ZRANGEBYSCORE" => {
                if args.len() < 3 {
                    return Err("ZRANGEBYSCORE requires at least 3 arguments".to_string());
                }
                let key = args[0].clone();
                let min = args[1].clone();
                let max = args[2].clone();
                let mut with_scores = false;
                let mut limit = None;

                let mut i = 3;
                while i < args.len() {
                    let opt = args[i].to_uppercase();
                    match opt.as_str() {
                        "WITHSCORES" => with_scores = true,
                        "LIMIT" => {
                            if i + 2 >= args.len() {
                                return Err(
                                    "ZRANGEBYSCORE LIMIT requires offset and count".to_string()
                                );
                            }
                            let offset: isize = args[i + 1]
                                .parse()
                                .map_err(|_| "ZRANGEBYSCORE LIMIT offset must be integer")?;
                            let count: usize = args[i + 2]
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
            "LMOVE" => {
                if args.len() != 4 {
                    return Err("LMOVE requires 4 arguments".to_string());
                }
                let wherefrom = args[2].to_uppercase();
                let whereto = args[3].to_uppercase();
                if wherefrom != "LEFT" && wherefrom != "RIGHT" {
                    return Err("LMOVE wherefrom must be LEFT or RIGHT".to_string());
                }
                if whereto != "LEFT" && whereto != "RIGHT" {
                    return Err("LMOVE whereto must be LEFT or RIGHT".to_string());
                }
                Ok(Command::LMove {
                    source: args[0].clone(),
                    dest: args[1].clone(),
                    wherefrom,
                    whereto,
                })
            }
            "HGETALL" => {
                if args.len() != 1 {
                    return Err("HGETALL requires 1 argument".to_string());
                }
                Ok(Command::HGetAll(args[0].clone()))
            }
            "EXISTS" => {
                if args.is_empty() {
                    return Err("EXISTS requires at least 1 argument".to_string());
                }
                Ok(Command::Exists(args.iter().cloned().collect()))
            }
            "EXPIRE" => {
                if args.len() != 2 {
                    return Err("EXPIRE requires 2 arguments".to_string());
                }
                let seconds: i64 = args[1]
                    .parse()
                    .map_err(|_| "EXPIRE seconds must be integer".to_string())?;
                Ok(Command::Expire(args[0].clone(), seconds))
            }
            "TTL" => {
                if args.len() != 1 {
                    return Err("TTL requires 1 argument".to_string());
                }
                Ok(Command::Ttl(args[0].clone()))
            }
            "TYPE" => {
                if args.len() != 1 {
                    return Err("TYPE requires 1 argument".to_string());
                }
                Ok(Command::TypeOf(args[0].clone()))
            }
            _ => Err(format!(
                "ERR Unknown Redis command '{}' called from Lua",
                cmd_name
            )),
        }
    }

    /// Parse a command from Lua script arguments (bytes version for binary-safe handling)
    /// This version preserves binary data like protobuf without UTF-8 conversion
    #[cfg(feature = "lua")]
    fn parse_lua_command_bytes(&self, parts: &[Vec<u8>]) -> Result<Command, String> {
        if parts.is_empty() {
            return Err("Empty command".to_string());
        }

        // Command name is ASCII so safe to convert to string
        let cmd_name = String::from_utf8_lossy(&parts[0]).to_uppercase();
        let args = &parts[1..];

        // Helper to convert bytes to string (for keys and options)
        let to_string = |b: &[u8]| String::from_utf8_lossy(b).to_string();
        // Helper to create SDS from bytes (preserves binary data)
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
                let value = to_sds(&args[1]); // Binary-safe
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
                Ok(Command::Del(args.iter().map(|b| to_string(b)).collect()))
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
                    .map_err(|_| "INCRBY increment must be integer".to_string())?;
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
                let fields: Vec<SDS> = args[1..].iter().map(|b| to_sds(b)).collect();
                Ok(Command::HDel(key, fields))
            }
            "HINCRBY" => {
                if args.len() != 3 {
                    return Err("HINCRBY requires 3 arguments".to_string());
                }
                let incr: i64 = to_string(&args[2])
                    .parse()
                    .map_err(|_| "HINCRBY increment must be integer".to_string())?;
                Ok(Command::HIncrBy(
                    to_string(&args[0]),
                    to_sds(&args[1]),
                    incr,
                ))
            }
            "LPUSH" => {
                if args.len() < 2 {
                    return Err("LPUSH requires key and at least 1 value".to_string());
                }
                let key = to_string(&args[0]);
                let values: Vec<SDS> = args[1..].iter().map(|b| to_sds(b)).collect();
                Ok(Command::LPush(key, values))
            }
            "RPUSH" => {
                if args.len() < 2 {
                    return Err("RPUSH requires key and at least 1 value".to_string());
                }
                let key = to_string(&args[0]);
                let values: Vec<SDS> = args[1..].iter().map(|b| to_sds(b)).collect();
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
            "LRANGE" => {
                if args.len() != 3 {
                    return Err("LRANGE requires 3 arguments".to_string());
                }
                let start: isize = to_string(&args[1])
                    .parse()
                    .map_err(|_| "LRANGE start must be integer".to_string())?;
                let stop: isize = to_string(&args[2])
                    .parse()
                    .map_err(|_| "LRANGE stop must be integer".to_string())?;
                Ok(Command::LRange(to_string(&args[0]), start, stop))
            }
            "RPOPLPUSH" => {
                if args.len() != 2 {
                    return Err("RPOPLPUSH requires 2 arguments".to_string());
                }
                Ok(Command::RPopLPush(to_string(&args[0]), to_string(&args[1])))
            }
            "SADD" => {
                if args.len() < 2 {
                    return Err("SADD requires key and at least 1 member".to_string());
                }
                let key = to_string(&args[0]);
                let members: Vec<SDS> = args[1..].iter().map(|b| to_sds(b)).collect();
                Ok(Command::SAdd(key, members))
            }
            "SREM" => {
                if args.len() < 2 {
                    return Err("SREM requires key and at least 1 member".to_string());
                }
                let key = to_string(&args[0]);
                let members: Vec<SDS> = args[1..].iter().map(|b| to_sds(b)).collect();
                Ok(Command::SRem(key, members))
            }
            "SMEMBERS" => {
                if args.len() != 1 {
                    return Err("SMEMBERS requires 1 argument".to_string());
                }
                Ok(Command::SMembers(to_string(&args[0])))
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

                // Parse optional flags
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
                        .map_err(|_| "ZADD score must be a number".to_string())?;
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
                let members: Vec<SDS> = args[1..].iter().map(|b| to_sds(b)).collect();
                Ok(Command::ZRem(key, members))
            }
            "ZRANGE" => {
                if args.len() != 3 {
                    return Err("ZRANGE requires 3 arguments".to_string());
                }
                let start: isize = to_string(&args[1])
                    .parse()
                    .map_err(|_| "ZRANGE start must be integer".to_string())?;
                let stop: isize = to_string(&args[2])
                    .parse()
                    .map_err(|_| "ZRANGE stop must be integer".to_string())?;
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
            "EXISTS" => {
                if args.is_empty() {
                    return Err("EXISTS requires at least 1 argument".to_string());
                }
                Ok(Command::Exists(args.iter().map(|b| to_string(b)).collect()))
            }
            "EXPIRE" => {
                if args.len() != 2 {
                    return Err("EXPIRE requires 2 arguments".to_string());
                }
                let seconds: i64 = to_string(&args[1])
                    .parse()
                    .map_err(|_| "EXPIRE seconds must be integer".to_string())?;
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
            _ => Err(format!(
                "ERR Unknown Redis command '{}' called from Lua",
                cmd_name
            )),
        }
    }

    /// Convert a Lua value to a RespValue
    #[cfg(feature = "lua")]
    fn lua_to_resp(&self, lua: &mlua::Lua, value: mlua::Value) -> RespValue {
        use mlua::Value as LuaValue;

        match value {
            LuaValue::Nil => RespValue::BulkString(None),
            LuaValue::Boolean(b) => {
                // Redis convention: true = 1, false = nil
                if b {
                    RespValue::Integer(1)
                } else {
                    RespValue::BulkString(None)
                }
            }
            LuaValue::Integer(i) => RespValue::Integer(i),
            LuaValue::Number(n) => {
                // Redis converts floats to bulk strings
                RespValue::BulkString(Some(n.to_string().into_bytes()))
            }
            LuaValue::String(s) => match s.as_bytes() {
                b => RespValue::BulkString(Some(b.to_vec())),
            },
            LuaValue::Table(t) => {
                // Check for Redis error/ok convention first
                if let Ok(err) = t.get::<String>("err") {
                    return RespValue::Error(err);
                }
                if let Ok(ok) = t.get::<String>("ok") {
                    return RespValue::SimpleString(ok);
                }

                // Check if it's a sequential array
                let mut elements = Vec::new();
                let mut is_array = true;
                let mut idx = 1i64;

                loop {
                    match t.get::<LuaValue>(idx) {
                        Ok(LuaValue::Nil) => break,
                        Ok(v) => {
                            elements.push(self.lua_to_resp(lua, v));
                            idx += 1;
                        }
                        Err(_) => {
                            is_array = false;
                            break;
                        }
                    }
                }

                if is_array && !elements.is_empty() {
                    RespValue::Array(Some(elements))
                } else {
                    // Return empty array for non-array tables
                    RespValue::Array(Some(vec![]))
                }
            }
            _ => RespValue::BulkString(None),
        }
    }

    fn incr_by_impl(&mut self, key: &str, increment: i64) -> RespValue {
        // TigerStyle: Precondition
        debug_assert!(!key.is_empty(), "Precondition: key must not be empty");

        let response = match self.get_value_mut(key) {
            Some(Value::String(s)) => {
                let current = match s.to_string().parse::<i64>() {
                    Ok(n) => n,
                    Err(_) => {
                        return RespValue::Error(
                            "ERR value is not an integer or out of range".to_string(),
                        )
                    }
                };
                let new_value = match current.checked_add(increment) {
                    Some(n) => n,
                    None => {
                        return RespValue::Error(
                            "ERR increment or decrement would overflow".to_string(),
                        )
                    }
                };
                let new_str = SDS::from_str(&new_value.to_string());
                self.data.insert(key.to_string(), Value::String(new_str));
                RespValue::Integer(new_value)
            }
            Some(_) => RespValue::Error(
                "WRONGTYPE Operation against a key holding the wrong kind of value".to_string(),
            ),
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

    fn matches_glob_pattern(&self, key: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        let key_chars: Vec<char> = key.chars().collect();
        let pattern_chars: Vec<char> = pattern.chars().collect();

        self.glob_match(&key_chars, &pattern_chars, 0, 0)
    }

    fn glob_match(&self, key: &[char], pattern: &[char], k_idx: usize, p_idx: usize) -> bool {
        if p_idx == pattern.len() {
            return k_idx == key.len();
        }

        let p_char = pattern[p_idx];

        if p_char == '*' {
            for i in k_idx..=key.len() {
                if self.glob_match(key, pattern, i, p_idx + 1) {
                    return true;
                }
            }
            false
        } else if p_char == '?' {
            if k_idx >= key.len() {
                false
            } else {
                self.glob_match(key, pattern, k_idx + 1, p_idx + 1)
            }
        } else if p_char == '[' {
            if k_idx >= key.len() {
                return false;
            }

            let mut bracket_end = p_idx + 1;
            while bracket_end < pattern.len() && pattern[bracket_end] != ']' {
                bracket_end += 1;
            }

            if bracket_end >= pattern.len() {
                return p_char == key[k_idx] && self.glob_match(key, pattern, k_idx + 1, p_idx + 1);
            }

            let char_set: Vec<char> = pattern[p_idx + 1..bracket_end].to_vec();
            let negate = !char_set.is_empty() && char_set[0] == '^';
            let chars_to_check = if negate {
                &char_set[1..]
            } else {
                &char_set[..]
            };

            let mut matched = false;
            for &c in chars_to_check {
                if c == key[k_idx] {
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
