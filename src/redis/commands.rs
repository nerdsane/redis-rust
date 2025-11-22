use super::data::*;
use super::resp::RespValue;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Command {
    Get(String),
    Set(String, SDS),
    Del(String),
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
                    "DEL" => {
                        if elements.len() != 2 {
                            return Err("DEL requires 1 argument".to_string());
                        }
                        let key = Self::extract_string(&elements[1])?;
                        Ok(Command::Del(key))
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
}

pub struct CommandExecutor {
    data: HashMap<String, Value>,
}

impl CommandExecutor {
    pub fn new() -> Self {
        CommandExecutor {
            data: HashMap::new(),
        }
    }

    pub fn execute(&mut self, cmd: &Command) -> RespValue {
        match cmd {
            Command::Ping => RespValue::SimpleString("PONG".to_string()),
            
            Command::Get(key) => {
                match self.data.get(key) {
                    Some(Value::String(s)) => RespValue::BulkString(Some(s.as_bytes().to_vec())),
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::BulkString(None),
                }
            }
            
            Command::Set(key, value) => {
                self.data.insert(key.clone(), Value::String(value.clone()));
                RespValue::SimpleString("OK".to_string())
            }
            
            Command::Del(key) => {
                let removed = self.data.remove(key).is_some();
                RespValue::Integer(if removed { 1 } else { 0 })
            }
            
            Command::LPush(key, values) => {
                let list = self.data.entry(key.clone()).or_insert_with(|| Value::List(RedisList::new()));
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
                let list = self.data.entry(key.clone()).or_insert_with(|| Value::List(RedisList::new()));
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
                match self.data.get_mut(key) {
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
                match self.data.get_mut(key) {
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
                match self.data.get(key) {
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
                let set = self.data.entry(key.clone()).or_insert_with(|| Value::Set(RedisSet::new()));
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
                match self.data.get(key) {
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
                match self.data.get(key) {
                    Some(Value::Set(s)) => {
                        RespValue::Integer(if s.contains(member) { 1 } else { 0 })
                    }
                    Some(_) => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                    None => RespValue::Integer(0),
                }
            }
            
            Command::HSet(key, field, value) => {
                let hash = self.data.entry(key.clone()).or_insert_with(|| Value::Hash(RedisHash::new()));
                match hash {
                    Value::Hash(h) => {
                        h.set(field.clone(), value.clone());
                        RespValue::Integer(1)
                    }
                    _ => RespValue::Error("WRONGTYPE Operation against a key holding the wrong kind of value".to_string()),
                }
            }
            
            Command::HGet(key, field) => {
                match self.data.get(key) {
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
                match self.data.get(key) {
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
                let zset = self.data.entry(key.clone()).or_insert_with(|| Value::SortedSet(RedisSortedSet::new()));
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
                match self.data.get(key) {
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
                match self.data.get(key) {
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
}
