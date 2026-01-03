use super::data::*;
use super::resp::RespValue;
use super::resp_optimized::RespValueZeroCopy;
use std::collections::HashMap;
use crate::simulator::VirtualTime;

#[derive(Debug, Clone)]
pub enum Command {
    Get(String),
    Set(String, SDS),
    SetEx(String, i64, SDS),
    SetNx(String, SDS),
    Del(String),
    Exists(Vec<String>),
    TypeOf(String),
    Keys(String),
    FlushDb,
    FlushAll,
    Expire(String, i64),
    ExpireAt(String, i64),
    PExpireAt(String, i64),
    Ttl(String),
    Pttl(String),
    Persist(String),
    Incr(String),
    Decr(String),
    IncrBy(String, i64),
    DecrBy(String, i64),
    Append(String, SDS),
    GetSet(String, SDS),
    MGet(Vec<String>),
    MSet(Vec<(String, SDS)>),
    /// Internal command for batched SET within a single shard (not exposed via RESP)
    BatchSet(Vec<(String, SDS)>),
    LPush(String, Vec<SDS>),
    RPush(String, Vec<SDS>),
    LPop(String),
    RPop(String),
    LRange(String, isize, isize),
    SAdd(String, Vec<SDS>),
    SMembers(String),
    SIsMember(String, SDS),
    HSet(String, SDS, SDS),
    HGet(String, SDS),
    HGetAll(String),
    ZAdd(String, Vec<(f64, SDS)>),
    ZRange(String, isize, isize),
    ZScore(String, SDS),
    Info,
    Ping,
    Unknown(String),
}

impl Command {
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
                    "FLUSHDB" => Ok(Command::FlushDb),
                    "FLUSHALL" => Ok(Command::FlushAll),
                    "GET" => {
                        if elements.len() != 2 {
                            return Err("GET requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Get(key))
                    }
                    "SET" => {
                        if elements.len() != 3 {
                            return Err("SET requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let value = Self::extract_sds(&elements[2])?;
                        Ok(Command::Set(key, value))
                    }
                    "SETEX" => {
                        if elements.len() != 4 {
                            return Err("SETEX requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let seconds = Self::extract_integer(&elements[2])? as i64;
                        let value = Self::extract_sds(&elements[3])?;
                        Ok(Command::SetEx(key, seconds, value))
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
                        if elements.len() != 2 {
                            return Err("DEL requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Del(key))
                    }
                    "EXISTS" => {
                        if elements.len() < 2 {
                            return Err("EXISTS requires at least 1 argument".to_string());
                        }
                        let keys = elements[1..].iter().map(Self::extract_string).collect::<Result<Vec<_>, _>>()?;
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
                    "MGET" => {
                        if elements.len() < 2 {
                            return Err("MGET requires at least 1 argument".to_string());
                        }
                        let keys = elements[1..].iter().map(Self::extract_string).collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::MGet(keys))
                    }
                    "MSET" => {
                        if elements.len() < 3 || (elements.len() - 1) % 2 != 0 {
                            return Err("MSET requires key-value pairs".to_string());
                        }
                        let mut pairs = Vec::new();
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
                        let values = elements[2..].iter().map(Self::extract_sds).collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::LPush(key, values))
                    }
                    "RPUSH" => {
                        if elements.len() < 3 {
                            return Err("RPUSH requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let values = elements[2..].iter().map(Self::extract_sds).collect::<Result<Vec<_>, _>>()?;
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
                    "SADD" => {
                        if elements.len() < 3 {
                            return Err("SADD requires at least 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let members = elements[2..].iter().map(Self::extract_sds).collect::<Result<Vec<_>, _>>()?;
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
                    "HSET" => {
                        if elements.len() != 4 {
                            return Err("HSET requires 3 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let field = Self::extract_sds(&elements[2])?;
                        let value = Self::extract_sds(&elements[3])?;
                        Ok(Command::HSet(key, field, value))
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
                    "ZADD" => {
                        if elements.len() < 4 || (elements.len() - 2) % 2 != 0 {
                            return Err("ZADD requires key and score-member pairs".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let mut pairs = Vec::new();
                        for i in (2..elements.len()).step_by(2) {
                            let score = Self::extract_float(&elements[i])?;
                            let member = Self::extract_sds(&elements[i + 1])?;
                            pairs.push((score, member));
                        }
                        Ok(Command::ZAdd(key, pairs))
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
                    "ZSCORE" => {
                        if elements.len() != 3 {
                            return Err("ZSCORE requires 2 arguments".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        let member = Self::extract_sds(&elements[2])?;
                        Ok(Command::ZScore(key, member))
                    }
                    _ => Ok(Command::Unknown(cmd_name)),
                }
            }
            _ => Err("Invalid command format".to_string()),
        }
    }

    fn extract_string(value: &RespValue) -> Result<String, String> {
        match value {
            RespValue::BulkString(Some(data)) => {
                Ok(String::from_utf8_lossy(data).to_string())
            }
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
                    "FLUSHDB" => Ok(Command::FlushDb),
                    "FLUSHALL" => Ok(Command::FlushAll),
                    "GET" => {
                        if elements.len() != 2 { return Err("GET requires 1 argument".to_string()); }
                        Ok(Command::Get(Self::extract_string_zc(&elements[1])?))
                    }
                    "SET" => {
                        if elements.len() != 3 { return Err("SET requires 2 arguments".to_string()); }
                        Ok(Command::Set(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?))
                    }
                    "SETEX" => {
                        if elements.len() != 4 { return Err("SETEX requires 3 arguments".to_string()); }
                        Ok(Command::SetEx(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])? as i64, Self::extract_sds_zc(&elements[3])?))
                    }
                    "SETNX" => {
                        if elements.len() != 3 { return Err("SETNX requires 2 arguments".to_string()); }
                        Ok(Command::SetNx(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?))
                    }
                    "DEL" => {
                        if elements.len() != 2 { return Err("DEL requires 1 argument".to_string()); }
                        Ok(Command::Del(Self::extract_string_zc(&elements[1])?))
                    }
                    "EXISTS" => {
                        if elements.len() < 2 { return Err("EXISTS requires at least 1 argument".to_string()); }
                        Ok(Command::Exists(elements[1..].iter().map(Self::extract_string_zc).collect::<Result<Vec<_>, _>>()?))
                    }
                    "TYPE" => {
                        if elements.len() != 2 { return Err("TYPE requires 1 argument".to_string()); }
                        Ok(Command::TypeOf(Self::extract_string_zc(&elements[1])?))
                    }
                    "KEYS" => {
                        if elements.len() != 2 { return Err("KEYS requires 1 argument".to_string()); }
                        Ok(Command::Keys(Self::extract_string_zc(&elements[1])?))
                    }
                    "EXPIRE" => {
                        if elements.len() != 3 { return Err("EXPIRE requires 2 arguments".to_string()); }
                        Ok(Command::Expire(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])? as i64))
                    }
                    "EXPIREAT" => {
                        if elements.len() != 3 { return Err("EXPIREAT requires 2 arguments".to_string()); }
                        Ok(Command::ExpireAt(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])? as i64))
                    }
                    "PEXPIREAT" => {
                        if elements.len() != 3 { return Err("PEXPIREAT requires 2 arguments".to_string()); }
                        Ok(Command::PExpireAt(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])? as i64))
                    }
                    "TTL" => {
                        if elements.len() != 2 { return Err("TTL requires 1 argument".to_string()); }
                        Ok(Command::Ttl(Self::extract_string_zc(&elements[1])?))
                    }
                    "PTTL" => {
                        if elements.len() != 2 { return Err("PTTL requires 1 argument".to_string()); }
                        Ok(Command::Pttl(Self::extract_string_zc(&elements[1])?))
                    }
                    "PERSIST" => {
                        if elements.len() != 2 { return Err("PERSIST requires 1 argument".to_string()); }
                        Ok(Command::Persist(Self::extract_string_zc(&elements[1])?))
                    }
                    "INCR" => {
                        if elements.len() != 2 { return Err("INCR requires 1 argument".to_string()); }
                        Ok(Command::Incr(Self::extract_string_zc(&elements[1])?))
                    }
                    "DECR" => {
                        if elements.len() != 2 { return Err("DECR requires 1 argument".to_string()); }
                        Ok(Command::Decr(Self::extract_string_zc(&elements[1])?))
                    }
                    "INCRBY" => {
                        if elements.len() != 3 { return Err("INCRBY requires 2 arguments".to_string()); }
                        Ok(Command::IncrBy(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])? as i64))
                    }
                    "DECRBY" => {
                        if elements.len() != 3 { return Err("DECRBY requires 2 arguments".to_string()); }
                        Ok(Command::DecrBy(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])? as i64))
                    }
                    "APPEND" => {
                        if elements.len() != 3 { return Err("APPEND requires 2 arguments".to_string()); }
                        Ok(Command::Append(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?))
                    }
                    "GETSET" => {
                        if elements.len() != 3 { return Err("GETSET requires 2 arguments".to_string()); }
                        Ok(Command::GetSet(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?))
                    }
                    "MGET" => {
                        if elements.len() < 2 { return Err("MGET requires at least 1 argument".to_string()); }
                        Ok(Command::MGet(elements[1..].iter().map(Self::extract_string_zc).collect::<Result<Vec<_>, _>>()?))
                    }
                    "MSET" => {
                        if elements.len() < 3 || (elements.len() - 1) % 2 != 0 { return Err("MSET requires key-value pairs".to_string()); }
                        let mut pairs = Vec::new();
                        for i in (1..elements.len()).step_by(2) {
                            pairs.push((Self::extract_string_zc(&elements[i])?, Self::extract_sds_zc(&elements[i + 1])?));
                        }
                        Ok(Command::MSet(pairs))
                    }
                    "LPUSH" => {
                        if elements.len() < 3 { return Err("LPUSH requires key and values".to_string()); }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let values = elements[2..].iter().map(Self::extract_sds_zc).collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::LPush(key, values))
                    }
                    "RPUSH" => {
                        if elements.len() < 3 { return Err("RPUSH requires key and values".to_string()); }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let values = elements[2..].iter().map(Self::extract_sds_zc).collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::RPush(key, values))
                    }
                    "LPOP" => {
                        if elements.len() != 2 { return Err("LPOP requires 1 argument".to_string()); }
                        Ok(Command::LPop(Self::extract_string_zc(&elements[1])?))
                    }
                    "RPOP" => {
                        if elements.len() != 2 { return Err("RPOP requires 1 argument".to_string()); }
                        Ok(Command::RPop(Self::extract_string_zc(&elements[1])?))
                    }
                    "LRANGE" => {
                        if elements.len() != 4 { return Err("LRANGE requires 3 arguments".to_string()); }
                        Ok(Command::LRange(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])?, Self::extract_integer_zc(&elements[3])?))
                    }
                    "SADD" => {
                        if elements.len() < 3 { return Err("SADD requires key and members".to_string()); }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let members = elements[2..].iter().map(Self::extract_sds_zc).collect::<Result<Vec<_>, _>>()?;
                        Ok(Command::SAdd(key, members))
                    }
                    "SMEMBERS" => {
                        if elements.len() != 2 { return Err("SMEMBERS requires 1 argument".to_string()); }
                        Ok(Command::SMembers(Self::extract_string_zc(&elements[1])?))
                    }
                    "SISMEMBER" => {
                        if elements.len() != 3 { return Err("SISMEMBER requires 2 arguments".to_string()); }
                        Ok(Command::SIsMember(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?))
                    }
                    "HSET" => {
                        if elements.len() != 4 { return Err("HSET requires 3 arguments".to_string()); }
                        Ok(Command::HSet(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?, Self::extract_sds_zc(&elements[3])?))
                    }
                    "HGET" => {
                        if elements.len() != 3 { return Err("HGET requires 2 arguments".to_string()); }
                        Ok(Command::HGet(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?))
                    }
                    "HGETALL" => {
                        if elements.len() != 2 { return Err("HGETALL requires 1 argument".to_string()); }
                        Ok(Command::HGetAll(Self::extract_string_zc(&elements[1])?))
                    }
                    "ZADD" => {
                        if elements.len() < 4 || (elements.len() - 2) % 2 != 0 { return Err("ZADD requires key and score-member pairs".to_string()); }
                        let key = Self::extract_string_zc(&elements[1])?;
                        let mut pairs = Vec::new();
                        for i in (2..elements.len()).step_by(2) {
                            pairs.push((Self::extract_float_zc(&elements[i])?, Self::extract_sds_zc(&elements[i + 1])?));
                        }
                        Ok(Command::ZAdd(key, pairs))
                    }
                    "ZRANGE" => {
                        if elements.len() != 4 { return Err("ZRANGE requires 3 arguments".to_string()); }
                        Ok(Command::ZRange(Self::extract_string_zc(&elements[1])?, Self::extract_integer_zc(&elements[2])?, Self::extract_integer_zc(&elements[3])?))
                    }
                    "ZSCORE" => {
                        if elements.len() != 3 { return Err("ZSCORE requires 2 arguments".to_string()); }
                        Ok(Command::ZScore(Self::extract_string_zc(&elements[1])?, Self::extract_sds_zc(&elements[2])?))
                    }
                    _ => Ok(Command::Unknown(cmd_name)),
                }
            }
            _ => Err("Invalid command format".to_string()),
        }
    }

    fn extract_string_zc(value: &RespValueZeroCopy) -> Result<String, String> {
        match value {
            RespValueZeroCopy::BulkString(Some(data)) => Ok(String::from_utf8_lossy(data).to_string()),
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
}

pub struct CommandExecutor {
    data: HashMap<String, Value>,
    expirations: HashMap<String, VirtualTime>,
    current_time: VirtualTime,
    access_times: HashMap<String, VirtualTime>,
    key_count: usize,
    commands_processed: usize,
    simulation_start_epoch: i64,
}

impl Command {
    /// Returns true if this command only reads data (no mutations)
    pub fn is_read_only(&self) -> bool {
        matches!(self,
            Command::Get(_) |
            Command::MGet(_) |
            Command::Exists(_) |
            Command::TypeOf(_) |
            Command::Keys(_) |
            Command::Ttl(_) |
            Command::Pttl(_) |
            Command::SMembers(_) |
            Command::SIsMember(_, _) |
            Command::HGet(_, _) |
            Command::HGetAll(_) |
            Command::LRange(_, _, _) |
            Command::ZRange(_, _, _) |
            Command::ZScore(_, _) |
            Command::Info |
            Command::Ping
        )
    }
    
    /// Returns the key(s) this command operates on (for sharding)
    pub fn get_primary_key(&self) -> Option<&str> {
        match self {
            Command::Get(k) | Command::Set(k, _) | Command::SetEx(k, _, _) |
            Command::SetNx(k, _) | Command::Del(k) | Command::TypeOf(k) |
            Command::Expire(k, _) | Command::ExpireAt(k, _) | Command::PExpireAt(k, _) |
            Command::Ttl(k) | Command::Pttl(k) | Command::Persist(k) |
            Command::Incr(k) | Command::Decr(k) | Command::IncrBy(k, _) |
            Command::DecrBy(k, _) | Command::Append(k, _) | Command::GetSet(k, _) |
            Command::LPush(k, _) | Command::RPush(k, _) | Command::LPop(k) |
            Command::RPop(k) | Command::LRange(k, _, _) | Command::SAdd(k, _) |
            Command::SMembers(k) | Command::SIsMember(k, _) | Command::HSet(k, _, _) |
            Command::HGet(k, _) | Command::HGetAll(k) | Command::ZAdd(k, _) |
            Command::ZRange(k, _, _) | Command::ZScore(k, _) => Some(k.as_str()),
            Command::Exists(keys) => keys.first().map(|s| s.as_str()),
            Command::MGet(keys) => keys.first().map(|s| s.as_str()),
            Command::MSet(pairs) => pairs.first().map(|(k, _)| k.as_str()),
            Command::BatchSet(pairs) => pairs.first().map(|(k, _)| k.as_str()),
            Command::Keys(_) | Command::FlushDb | Command::FlushAll |
            Command::Info | Command::Ping | Command::Unknown(_) => None,
        }
    }
}

impl CommandExecutor {
    pub fn new() -> Self {
        CommandExecutor {
            data: HashMap::new(),
            expirations: HashMap::new(),
            current_time: VirtualTime::from_millis(0),
            access_times: HashMap::new(),
            key_count: 0,
            commands_processed: 0,
            simulation_start_epoch: 0,
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
    
    /// Direct expiration eviction - call this from TTL manager
    /// Returns the number of keys evicted
    pub fn evict_expired_direct(&mut self, current_time: VirtualTime) -> usize {
        self.current_time = current_time;
        
        let expired_keys: Vec<String> = self.expirations
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
        count
    }

    fn evict_expired_keys(&mut self) {
        let expired_keys: Vec<String> = self.expirations
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
                let count = keys.iter().filter(|k| {
                    !self.is_expired(k) && self.data.contains_key(*k)
                }).count();
                RespValue::Integer(count as i64)
            }
            Command::Keys(pattern) => {
                let matching: Vec<RespValue> = self.data.keys()
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
    pub fn get_data(&self) -> &HashMap<String, Value> {
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
            
            Command::Get(key) => {
                match self.get_value(key) {
                    Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::BulkString(None),
                }
            }
            
            Command::Set(key, value) => {
                self.data.insert(key.clone(), Value::String(value.clone()));
                self.expirations.remove(key);
                self.access_times.insert(key.clone(), self.current_time);
                RespValue::SimpleString("OK".to_string())
            }
            
            Command::SetEx(key, seconds, value) => {
                if *seconds <= 0 {
                    return RespValue::Error("ERR invalid expire time in setex".to_string());
                }
                self.data.insert(key.clone(), Value::String(value.clone()));
                let expiration = self.current_time + crate::simulator::Duration::from_secs(*seconds as u64);
                self.expirations.insert(key.clone(), expiration);
                self.access_times.insert(key.clone(), self.current_time);
                RespValue::SimpleString("OK".to_string())
            }
            
            Command::SetNx(key, value) => {
                if !self.is_expired(key) && self.data.contains_key(key) {
                    RespValue::Integer(0)
                } else {
                    self.data.insert(key.clone(), Value::String(value.clone()));
                    self.expirations.remove(key);
                    self.access_times.insert(key.clone(), self.current_time);
                    RespValue::Integer(1)
                }
            }
            
            Command::Del(key) => {
                let removed = self.data.remove(key).is_some();
                self.expirations.remove(key);
                self.access_times.remove(key);
                RespValue::Integer(if removed { 1 } else { 0 })
            }
            
            Command::Exists(keys) => {
                let count = keys.iter().filter(|k| {
                    !self.is_expired(k) && self.data.contains_key(*k)
                }).count();
                RespValue::Integer(count as i64)
            }
            
            Command::TypeOf(key) => {
                match self.get_value(key) {
                    Some(Value::String(_)) => RespValue::SimpleString("string".to_string()),
                    Some(Value::List(_)) => RespValue::SimpleString("list".to_string()),
                    Some(Value::Set(_)) => RespValue::SimpleString("set".to_string()),
                    Some(Value::Hash(_)) => RespValue::SimpleString("hash".to_string()),
                    Some(Value::SortedSet(_)) => RespValue::SimpleString("zset".to_string()),
                    _ => RespValue::SimpleString("none".to_string()),
                }
            }
            
            Command::Keys(pattern) => {
                let keys: Vec<RespValue> = self.data.keys()
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
                        let expiration = self.current_time + crate::simulator::Duration::from_secs(*seconds as u64);
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
                        let expiration_millis = (simulation_relative_secs as u64).saturating_mul(1000);
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
                    let simulation_relative_millis = *timestamp_millis - (self.simulation_start_epoch * 1000);
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
                        self.expirations.insert(key.clone(), expiration);
                        RespValue::Integer(1)
                    }
                }
            }
            
            Command::Ttl(key) => {
                if self.is_expired(key) || !self.data.contains_key(key) {
                    RespValue::Integer(-2)
                } else if let Some(expiration) = self.expirations.get(key) {
                    let remaining_ms = expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
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
                    let remaining = expiration.as_millis() as i64 - self.current_time.as_millis() as i64;
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
            
            Command::Incr(key) => {
                self.incr_by_impl(key, 1)
            }
            
            Command::Decr(key) => {
                self.incr_by_impl(key, -1)
            }
            
            Command::IncrBy(key, increment) => {
                self.incr_by_impl(key, *increment)
            }
            
            Command::DecrBy(key, decrement) => {
                self.incr_by_impl(key, -decrement)
            }
            
            Command::Append(key, value) => {
                match self.get_value_mut(key) {
                    Some(Value::String(s)) => {
                        s.append(value);
                        RespValue::Integer(s.len() as i64)
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => {
                        let len = value.len();
                        self.data.insert(key.clone(), Value::String(value.clone()));
                        self.access_times.insert(key.clone(), self.current_time);
                        RespValue::Integer(len as i64)
                    }
                }
            }
            
            Command::GetSet(key, value) => {
                let old_value = match self.get_value(key) {
                    Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                    Some(_) => return RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::BulkString(None),
                };
                self.data.insert(key.clone(), Value::String(value.clone()));
                self.access_times.insert(key.clone(), self.current_time);
                old_value
            }
            
            Command::MGet(keys) => {
                let values: Vec<RespValue> = keys.iter().map(|k| {
                    match self.get_value(k) {
                        Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                        _ => RespValue::BulkString(None),
                    }
                }).collect();
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

            Command::LPush(key, values) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let list = self.data.entry(key.clone()).or_insert_with(|| Value::List(RedisList::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match list {
                    Value::List(l) => {
                        for value in values {
                            l.lpush(value.clone());
                        }
                        RespValue::Integer(l.len() as i64)
                    }
                    _ => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                }
            }
            
            Command::RPush(key, values) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let list = self.data.entry(key.clone()).or_insert_with(|| Value::List(RedisList::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match list {
                    Value::List(l) => {
                        for value in values {
                            l.rpush(value.clone());
                        }
                        RespValue::Integer(l.len() as i64)
                    }
                    _ => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                }
            }
            
            Command::LPop(key) => {
                match self.get_value_mut(key) {
                    Some(Value::List(l)) => {
                        match l.lpop() {
                            Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                            None => RespValue::BulkString(None),
                        }
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::BulkString(None),
                }
            }
            
            Command::RPop(key) => {
                match self.get_value_mut(key) {
                    Some(Value::List(l)) => {
                        match l.rpop() {
                            Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                            None => RespValue::BulkString(None),
                        }
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::BulkString(None),
                }
            }
            
            Command::LRange(key, start, stop) => {
                match self.get_value(key) {
                    Some(Value::List(l)) => {
                        let range = l.range(*start, *stop);
                        let elements: Vec<RespValue> = range
                            .iter()
                            .map(|s| RespValue::BulkString(Some(s.as_bytes().to_vec())))
                            .collect();
                        RespValue::Array(Some(elements))
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::Array(Some(Vec::new())),
                }
            }
            
            Command::SAdd(key, members) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let set = self.data.entry(key.clone()).or_insert_with(|| Value::Set(RedisSet::new()));
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
                    _ => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                }
            }
            
            Command::SMembers(key) => {
                match self.get_value(key) {
                    Some(Value::Set(s)) => {
                        let members: Vec<RespValue> = s.members()
                            .iter()
                            .map(|m| RespValue::BulkString(Some(m.as_bytes().to_vec())))
                            .collect();
                        RespValue::Array(Some(members))
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::Array(Some(Vec::new())),
                }
            }
            
            Command::SIsMember(key, member) => {
                match self.get_value(key) {
                    Some(Value::Set(s)) => {
                        RespValue::Integer(if s.contains(member) { 1 } else { 0 })
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::Integer(0),
                }
            }
            
            Command::HSet(key, field, value) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let hash = self.data.entry(key.clone()).or_insert_with(|| Value::Hash(RedisHash::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match hash {
                    Value::Hash(h) => {
                        h.set(field.clone(), value.clone());
                        RespValue::Integer(1)
                    }
                    _ => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                }
            }
            
            Command::HGet(key, field) => {
                match self.get_value(key) {
                    Some(Value::Hash(h)) => {
                        match h.get(field) {
                            Some(v) => RespValue::BulkString(Some(v.as_bytes().to_vec())),
                            None => RespValue::BulkString(None),
                        }
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::BulkString(None),
                }
            }
            
            Command::HGetAll(key) => {
                match self.get_value(key) {
                    Some(Value::Hash(h)) => {
                        let mut elements = Vec::new();
                        for (k, v) in h.get_all() {
                            elements.push(RespValue::BulkString(Some(k.as_bytes().to_vec())));
                            elements.push(RespValue::BulkString(Some(v.as_bytes().to_vec())));
                        }
                        RespValue::Array(Some(elements))
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::Array(Some(Vec::new())),
                }
            }
            
            Command::ZAdd(key, pairs) => {
                if self.is_expired(key) {
                    self.data.remove(key);
                    self.expirations.remove(key);
                }
                let zset = self.data.entry(key.clone()).or_insert_with(|| Value::SortedSet(RedisSortedSet::new()));
                self.access_times.insert(key.clone(), self.current_time);
                match zset {
                    Value::SortedSet(zs) => {
                        let mut added = 0;
                        for (score, member) in pairs {
                            if zs.add(member.clone(), *score) {
                                added += 1;
                            }
                        }
                        RespValue::Integer(added)
                    }
                    _ => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                }
            }
            
            Command::ZRange(key, start, stop) => {
                match self.get_value(key) {
                    Some(Value::SortedSet(zs)) => {
                        let range = zs.range(*start, *stop);
                        let elements: Vec<RespValue> = range
                            .iter()
                            .map(|(m, _)| RespValue::BulkString(Some(m.as_bytes().to_vec())))
                            .collect();
                        RespValue::Array(Some(elements))
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::Array(Some(Vec::new())),
                }
            }
            
            Command::ZScore(key, member) => {
                match self.get_value(key) {
                    Some(Value::SortedSet(zs)) => {
                        match zs.score(member) {
                            Some(score) => {
                                let score_str = score.to_string();
                                RespValue::BulkString(Some(score_str.into_bytes()))
                            }
                            None => RespValue::BulkString(None),
                        }
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::BulkString(None),
                }
            }
            
            Command::Unknown(cmd) => {
                RespValue::Error(format!("ERR unknown command '{}'", cmd))
            }
        }
    }

    fn incr_by_impl(&mut self, key: &str, increment: i64) -> RespValue {
        match self.get_value_mut(key) {
            Some(Value::String(s)) => {
                let current = match s.to_string().parse::<i64>() {
                    Ok(n) => n,
                    Err(_) => return RespValue::Error("ERR value is not an integer or out of range".to_string()),
                };
                let new_value = match current.checked_add(increment) {
                    Some(n) => n,
                    None => return RespValue::Error("ERR increment or decrement would overflow".to_string()),
                };
                let new_str = SDS::from_str(&new_value.to_string());
                self.data.insert(key.to_string(), Value::String(new_str));
                RespValue::Integer(new_value)
            }
            Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
            None => {
                self.data.insert(key.to_string(), Value::String(SDS::from_str(&increment.to_string())));
                self.access_times.insert(key.to_string(), self.current_time);
                RespValue::Integer(increment)
            }
        }
    }

    fn matches_glob_pattern(&self, key: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        
        let mut key_chars: Vec<char> = key.chars().collect();
        let mut pattern_chars: Vec<char> = pattern.chars().collect();
        
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
            let chars_to_check = if negate { &char_set[1..] } else { &char_set[..] };
            
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
