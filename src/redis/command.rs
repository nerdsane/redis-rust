//! Redis Command enum and utility methods.
//!
//! This module defines the `Command` enum representing all supported Redis commands,
//! along with helper constructors and utility methods for command introspection.
//!
//! Parsing logic is in `parser.rs` and `parser_zero_copy.rs`.
//! Execution logic is in the `executor/` module.

use super::data::SDS;

/// Represents a Redis command parsed from RESP protocol.
///
/// # Categories
///
/// - **String commands**: GET, SET, APPEND, etc.
/// - **Counter commands**: INCR, DECR, INCRBY, DECRBY
/// - **Key commands**: DEL, EXISTS, TYPE, KEYS, FLUSHDB, FLUSHALL
/// - **Expiration commands**: EXPIRE, EXPIREAT, TTL, PTTL, PERSIST
/// - **List commands**: LPUSH, RPUSH, LPOP, RPOP, LLEN, LRANGE, etc.
/// - **Set commands**: SADD, SREM, SMEMBERS, SISMEMBER, SCARD, SPOP
/// - **Hash commands**: HSET, HGET, HDEL, HGETALL, HKEYS, HVALS, etc.
/// - **Sorted set commands**: ZADD, ZREM, ZRANGE, ZREVRANGE, ZSCORE, etc.
/// - **Scan commands**: SCAN, HSCAN, ZSCAN
/// - **Transaction commands**: MULTI, EXEC, DISCARD, WATCH, UNWATCH
/// - **Script commands**: EVAL, EVALSHA, SCRIPT LOAD/EXISTS/FLUSH
/// - **Server commands**: INFO, PING, DBSIZE
/// - **Auth/ACL commands**: AUTH, ACL WHOAMI/LIST/USERS/GETUSER/SETUSER/DELUSER/CAT/GENPASS
#[derive(Debug, Clone)]
pub enum Command {
    // String commands
    Get(String),
    /// SET key value [NX|XX] [EX s|PX ms|EXAT t|PXAT t_ms|KEEPTTL] [GET]
    Set {
        key: String,
        value: SDS,
        ex: Option<i64>,   // EX seconds
        px: Option<i64>,   // PX milliseconds
        exat: Option<i64>, // EXAT unix-time-seconds
        pxat: Option<i64>, // PXAT unix-time-milliseconds
        nx: bool,          // Only set if NOT exists
        xx: bool,          // Only set if exists
        get: bool,         // Return old value
        keepttl: bool,     // Preserve existing TTL
    },
    Append(String, SDS),
    GetSet(String, SDS),
    StrLen(String),
    MGet(Vec<String>),
    MSet(Vec<(String, SDS)>),
    MSetNx(Vec<(String, SDS)>),
    /// Internal command for batched SET within a single shard (not exposed via RESP)
    BatchSet(Vec<(String, SDS)>),
    /// Internal command for batched GET within a single shard (not exposed via RESP)
    BatchGet(Vec<String>),
    /// GETRANGE key start end (also SUBSTR alias)
    GetRange(String, isize, isize),
    /// SETRANGE key offset value
    SetRange(String, usize, SDS),
    /// GETEX key [EX s|PX ms|EXAT t|PXAT t|PERSIST]
    GetEx {
        key: String,
        ex: Option<i64>,
        px: Option<i64>,
        exat: Option<i64>,
        pxat: Option<i64>,
        persist: bool,
    },
    /// GETDEL key
    GetDel(String),
    // Counter commands
    Incr(String),
    Decr(String),
    IncrBy(String, i64),
    DecrBy(String, i64),
    /// INCRBYFLOAT key increment
    IncrByFloat(String, f64),
    // Key commands
    Del(Vec<String>),
    Exists(Vec<String>),
    TypeOf(String),
    Keys(String),
    FlushDb,
    FlushAll,
    // Expiration commands
    Expire {
        key: String,
        seconds: i64,
        nx: bool,
        xx: bool,
        gt: bool,
        lt: bool,
    },
    ExpireAt(String, i64),
    PExpire {
        key: String,
        milliseconds: i64,
        nx: bool,
        xx: bool,
        gt: bool,
        lt: bool,
    },
    PExpireAt(String, i64),
    Ttl(String),
    Pttl(String),
    /// EXPIRETIME key - returns Unix timestamp (seconds) when key will expire
    ExpireTime(String),
    /// PEXPIRETIME key - returns Unix timestamp (milliseconds) when key will expire
    PExpireTime(String),
    Persist(String),
    // Server commands (stubs)
    /// WAIT numreplicas timeout
    Wait(i64, i64),
    /// TIME - returns [seconds, microseconds]
    Time,
    /// SORT key [STORE dest] ... - minimal stub
    Sort {
        key: String,
        store: Option<String>,
    },
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
    SPop(String, Option<usize>), // SPOP key [count]
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
    ZRange(String, isize, isize, bool), // bool = WITHSCORES
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
    // String commands (legacy)
    /// SETNX key value - legacy command returning Integer(1)/Integer(0)
    SetNx(String, SDS),
    // Server commands
    Info,
    Ping(Option<SDS>),
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
    AclGetUser {
        username: String,
    },
    /// ACL SETUSER username [rules...]
    AclSetUser {
        username: String,
        rules: Vec<String>,
    },
    /// ACL DELUSER username [username...]
    AclDelUser {
        usernames: Vec<String>,
    },
    /// ACL CAT [category]
    AclCat {
        category: Option<String>,
    },
    /// ACL GENPASS [bits]
    AclGenPass {
        bits: Option<u32>,
    },
    // CONFIG commands
    ConfigGet(String),
    ConfigSet(String, String),
    ConfigResetStat,
    // SELECT command
    Select(u64),
    // ECHO command
    Echo(SDS),
    // COMMAND command (for Tcl test harness compatibility)
    CommandCommand,   // COMMAND / COMMAND COUNT / COMMAND DOCS etc. - stub
    CommandCount,
    // FUNCTION FLUSH (for Tcl test harness compatibility)
    FunctionFlush,
    // CLIENT command stubs (for Tcl test harness compatibility)
    ClientSetName(String),
    ClientGetName,
    ClientId,
    ClientInfo,
    // OBJECT command stubs
    ObjectHelp,
    ObjectEncoding(String),
    ObjectRefCount(String),
    ObjectIdleTime(String),
    ObjectFreq(String),
    // DEBUG command stubs
    DebugSleep(f64),
    DebugSet(String, String),
    DebugObject(String),
    // RANDOMKEY
    RandomKey,
    // RENAME
    Rename(String, String),
    RenameNx(String, String),
    // OBJECT
    Unknown(String),
}

impl Command {
    // =========================================================================
    // Helper Constructors
    // =========================================================================

    /// Helper constructor for basic SET (no options)
    pub fn set(key: String, value: SDS) -> Self {
        Command::Set {
            key,
            value,
            ex: None,
            px: None,
            exat: None,
            pxat: None,
            nx: false,
            xx: false,
            get: false,
            keepttl: false,
        }
    }

    /// Helper constructor for SETEX (SET with EX option)
    pub fn setex(key: String, seconds: i64, value: SDS) -> Self {
        Command::Set {
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
        }
    }

    /// Helper constructor for SETNX (legacy command returning Integer)
    pub fn setnx(key: String, value: SDS) -> Self {
        Command::SetNx(key, value)
    }

    /// Helper constructor for basic EXPIRE (no flags)
    pub fn expire(key: String, seconds: i64) -> Self {
        Command::Expire {
            key,
            seconds,
            nx: false,
            xx: false,
            gt: false,
            lt: false,
        }
    }

    /// Helper constructor for single-key DEL
    pub fn del(key: String) -> Self {
        Command::Del(vec![key])
    }

    // =========================================================================
    // Utility Methods
    // =========================================================================

    /// Returns true if this command only reads data (no mutations)
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Command::Get(_)
                | Command::GetRange(_, _, _)
                | Command::StrLen(_)
                | Command::MGet(_)
                | Command::Exists(_)
                | Command::TypeOf(_)
                | Command::Keys(_)
                | Command::Ttl(_)
                | Command::Pttl(_)
                | Command::ExpireTime(_)
                | Command::PExpireTime(_)
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
                | Command::ZRange(_, _, _, _)
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
                | Command::Ping(_)
                | Command::ConfigGet(_)
                | Command::Echo(_)
                | Command::CommandCommand
                | Command::CommandCount
                | Command::ClientGetName
                | Command::ClientId
                | Command::ClientInfo
                | Command::ObjectHelp
                | Command::ObjectEncoding(_)
                | Command::ObjectRefCount(_)
                | Command::ObjectIdleTime(_)
                | Command::ObjectFreq(_)
                | Command::RandomKey
                | Command::DbSize
                | Command::Wait(_, _)
                | Command::Time
        )
    }

    /// Returns the key(s) this command operates on (for sharding)
    pub fn get_primary_key(&self) -> Option<&str> {
        match self {
            Command::Get(k)
            | Command::Set { key: k, .. }
            | Command::SetNx(k, _)
            | Command::GetRange(k, _, _)
            | Command::SetRange(k, _, _)
            | Command::GetEx { key: k, .. }
            | Command::GetDel(k)
            | Command::TypeOf(k)
            | Command::Expire { key: k, .. }
            | Command::ExpireAt(k, _)
            | Command::PExpire { key: k, .. }
            | Command::PExpireAt(k, _)
            | Command::Ttl(k)
            | Command::Pttl(k)
            | Command::ExpireTime(k)
            | Command::PExpireTime(k)
            | Command::Persist(k)
            | Command::Incr(k)
            | Command::Decr(k)
            | Command::IncrBy(k, _)
            | Command::DecrBy(k, _)
            | Command::IncrByFloat(k, _)
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
            | Command::SPop(k, _)
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
            | Command::ZRange(k, _, _, _)
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
            Command::MSet(pairs) | Command::MSetNx(pairs) => {
                pairs.first().map(|(k, _)| k.as_str())
            }
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
            | Command::Ping(_)
            | Command::DbSize
            | Command::Wait(_, _)
            | Command::Time
            | Command::Auth { .. }
            | Command::AclWhoami
            | Command::AclList
            | Command::AclUsers
            | Command::AclGetUser { .. }
            | Command::AclSetUser { .. }
            | Command::AclDelUser { .. }
            | Command::AclCat { .. }
            | Command::AclGenPass { .. }
            | Command::ConfigGet(_)
            | Command::ConfigSet(_, _)
            | Command::ConfigResetStat
            | Command::Select(_)
            | Command::Echo(_)
            | Command::CommandCommand
            | Command::CommandCount
            | Command::FunctionFlush
            | Command::ClientSetName(_)
            | Command::ClientGetName
            | Command::ClientId
            | Command::ClientInfo
            | Command::ObjectHelp
            | Command::DebugSleep(_)
            | Command::DebugSet(_, _)
            | Command::RandomKey
            | Command::Unknown(_) => None,

            Command::ObjectEncoding(k)
            | Command::ObjectRefCount(k)
            | Command::ObjectIdleTime(k)
            | Command::ObjectFreq(k)
            | Command::DebugObject(k) => Some(k.as_str()),

            Command::Sort { key: k, .. } => Some(k.as_str()),
            Command::Rename(k, _) | Command::RenameNx(k, _) => Some(k.as_str()),
        }
    }

    /// Returns all keys this command operates on (for ACL permission checking)
    pub fn get_keys(&self) -> Vec<String> {
        match self {
            Command::Get(k)
            | Command::Set { key: k, .. }
            | Command::SetNx(k, _)
            | Command::GetRange(k, _, _)
            | Command::SetRange(k, _, _)
            | Command::GetEx { key: k, .. }
            | Command::GetDel(k)
            | Command::TypeOf(k)
            | Command::Expire { key: k, .. }
            | Command::ExpireAt(k, _)
            | Command::PExpire { key: k, .. }
            | Command::PExpireAt(k, _)
            | Command::Ttl(k)
            | Command::Pttl(k)
            | Command::ExpireTime(k)
            | Command::PExpireTime(k)
            | Command::Persist(k)
            | Command::Incr(k)
            | Command::Decr(k)
            | Command::IncrBy(k, _)
            | Command::DecrBy(k, _)
            | Command::IncrByFloat(k, _)
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
            | Command::SAdd(k, _)
            | Command::SRem(k, _)
            | Command::SMembers(k)
            | Command::SIsMember(k, _)
            | Command::SCard(k)
            | Command::SPop(k, _)
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
            | Command::ZRange(k, _, _, _)
            | Command::ZRevRange(k, _, _, _)
            | Command::ZScore(k, _)
            | Command::ZRank(k, _)
            | Command::ZCard(k)
            | Command::ZCount(k, _, _)
            | Command::ZRangeByScore { key: k, .. }
            | Command::HScan { key: k, .. }
            | Command::ZScan { key: k, .. }
            | Command::Keys(k) => vec![k.clone()],

            // Commands with two keys (source, dest)
            Command::RPopLPush(src, dst) => vec![src.clone(), dst.clone()],
            Command::LMove { source, dest, .. } => vec![source.clone(), dest.clone()],

            // Multi-key commands
            Command::Del(keys) | Command::Exists(keys) | Command::MGet(keys) => keys.clone(),
            Command::MSet(pairs) | Command::MSetNx(pairs) => {
                pairs.iter().map(|(k, _)| k.clone()).collect()
            }
            Command::BatchSet(pairs) => pairs.iter().map(|(k, _)| k.clone()).collect(),
            Command::BatchGet(keys) => keys.clone(),
            Command::Watch(keys) => keys.clone(),
            Command::Eval { keys, .. } | Command::EvalSha { keys, .. } => keys.clone(),

            // Commands with no keys
            Command::Scan { .. }
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
            | Command::Ping(_)
            | Command::DbSize
            | Command::Wait(_, _)
            | Command::Time
            | Command::Auth { .. }
            | Command::AclWhoami
            | Command::AclList
            | Command::AclUsers
            | Command::AclGetUser { .. }
            | Command::AclSetUser { .. }
            | Command::AclDelUser { .. }
            | Command::AclCat { .. }
            | Command::AclGenPass { .. }
            | Command::ConfigGet(_)
            | Command::ConfigSet(_, _)
            | Command::ConfigResetStat
            | Command::Select(_)
            | Command::Echo(_)
            | Command::CommandCommand
            | Command::CommandCount
            | Command::FunctionFlush
            | Command::ClientSetName(_)
            | Command::ClientGetName
            | Command::ClientId
            | Command::ClientInfo
            | Command::ObjectHelp
            | Command::DebugSleep(_)
            | Command::DebugSet(_, _)
            | Command::RandomKey
            | Command::Unknown(_) => vec![],

            Command::ObjectEncoding(k)
            | Command::ObjectRefCount(k)
            | Command::ObjectIdleTime(k)
            | Command::ObjectFreq(k)
            | Command::DebugObject(k) => vec![k.clone()],

            Command::Sort { key, store } => {
                let mut keys = vec![key.clone()];
                if let Some(dest) = store {
                    keys.push(dest.clone());
                }
                keys
            }
            Command::Rename(src, dst) | Command::RenameNx(src, dst) => {
                vec![src.clone(), dst.clone()]
            }
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
            Command::MSetNx(_) => "MSETNX",
            Command::BatchSet(_) => "BATCHSET",
            Command::BatchGet(_) => "BATCHGET",
            Command::GetRange(_, _, _) => "GETRANGE",
            Command::SetRange(_, _, _) => "SETRANGE",
            Command::GetEx { .. } => "GETEX",
            Command::GetDel(_) => "GETDEL",
            Command::Incr(_) => "INCR",
            Command::Decr(_) => "DECR",
            Command::IncrBy(_, _) => "INCRBY",
            Command::DecrBy(_, _) => "DECRBY",
            Command::IncrByFloat(_, _) => "INCRBYFLOAT",
            Command::Del(_) => "DEL",
            Command::Exists(_) => "EXISTS",
            Command::TypeOf(_) => "TYPE",
            Command::Keys(_) => "KEYS",
            Command::FlushDb => "FLUSHDB",
            Command::FlushAll => "FLUSHALL",
            Command::Expire { .. } => "EXPIRE",
            Command::ExpireAt(_, _) => "EXPIREAT",
            Command::PExpire { .. } => "PEXPIRE",
            Command::PExpireAt(_, _) => "PEXPIREAT",
            Command::Ttl(_) => "TTL",
            Command::Pttl(_) => "PTTL",
            Command::ExpireTime(_) => "EXPIRETIME",
            Command::PExpireTime(_) => "PEXPIRETIME",
            Command::Persist(_) => "PERSIST",
            Command::Wait(_, _) => "WAIT",
            Command::Time => "TIME",
            Command::Sort { .. } => "SORT",
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
            Command::SPop(_, _) => "SPOP",
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
            Command::SetNx(_, _) => "SETNX",
            Command::ZRange(_, _, _, _) => "ZRANGE",
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
            Command::Ping(_) => "PING",
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
            Command::ConfigGet(_) => "CONFIG",
            Command::ConfigSet(_, _) => "CONFIG",
            Command::ConfigResetStat => "CONFIG",
            Command::Select(_) => "SELECT",
            Command::Echo(_) => "ECHO",
            Command::CommandCommand => "COMMAND",
            Command::CommandCount => "COMMAND",
            Command::FunctionFlush => "FUNCTION",
            Command::ClientSetName(_) => "CLIENT",
            Command::ClientGetName => "CLIENT",
            Command::ClientId => "CLIENT",
            Command::ClientInfo => "CLIENT",
            Command::ObjectHelp => "OBJECT",
            Command::ObjectEncoding(_) => "OBJECT",
            Command::ObjectRefCount(_) => "OBJECT",
            Command::ObjectIdleTime(_) => "OBJECT",
            Command::ObjectFreq(_) => "OBJECT",
            Command::DebugSleep(_) => "DEBUG",
            Command::DebugSet(_, _) => "DEBUG",
            Command::DebugObject(_) => "DEBUG",
            Command::RandomKey => "RANDOMKEY",
            Command::Rename(_, _) => "RENAME",
            Command::RenameNx(_, _) => "RENAMENX",
            Command::Unknown(_) => "UNKNOWN",
        }
    }
}
