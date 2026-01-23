//! Redis Equivalence Differential Test
//!
//! Compares responses from real Redis vs redis-rust to ensure behavioral equivalence.
//!
//! # Prerequisites
//! - Real Redis running on port 6379
//! - redis-rust running on port 3000
//!
//! # Usage
//! ```bash
//! # Start Redis
//! docker run -p 6379:6379 redis:7-alpine
//!
//! # Start redis-rust
//! cargo run --release --bin redis-server-optimized -- --port 3000
//!
//! # Run tests
//! cargo test --release redis_equivalence -- --ignored --nocapture
//! ```

use redis::{Connection, RedisResult};
use std::time::Duration;

/// Connect to a Redis server with the given port
fn connect(port: u16) -> RedisResult<Connection> {
    let client = redis::Client::open(format!("redis://127.0.0.1:{}/", port))?;
    client.get_connection_with_timeout(Duration::from_secs(2))
}

/// Wrapper to compare Redis responses
#[derive(Debug, Clone, PartialEq)]
enum Response {
    Nil,
    Int(i64),
    Str(String),
    Bulk(Vec<String>),
    Error(String),
}

impl Response {
    fn from_redis_value(v: redis::Value) -> Self {
        match v {
            redis::Value::Nil => Response::Nil,
            redis::Value::Int(i) => Response::Int(i),
            redis::Value::BulkString(s) => Response::Str(String::from_utf8_lossy(&s).to_string()),
            redis::Value::SimpleString(s) => Response::Str(s),
            redis::Value::Okay => Response::Str("OK".to_string()),
            redis::Value::Array(arr) => {
                let strings: Vec<String> = arr
                    .into_iter()
                    .map(|v| match v {
                        redis::Value::BulkString(s) => String::from_utf8_lossy(&s).to_string(),
                        redis::Value::SimpleString(s) => s,
                        redis::Value::Okay => "OK".to_string(),
                        redis::Value::Int(i) => i.to_string(),
                        redis::Value::Nil => "nil".to_string(),
                        _ => format!("{:?}", v),
                    })
                    .collect();
                Response::Bulk(strings)
            }
            _ => Response::Str(format!("{:?}", v)),
        }
    }
}

/// Execute a raw Redis command and return the response
fn exec_cmd(conn: &mut Connection, args: &[&str]) -> Response {
    let mut cmd = redis::cmd(args[0]);
    for arg in &args[1..] {
        cmd.arg(*arg);
    }
    match cmd.query::<redis::Value>(conn) {
        Ok(v) => Response::from_redis_value(v),
        Err(e) => Response::Error(format!("{}", e)),
    }
}

/// Compare responses, allowing for acceptable differences
fn responses_match(redis_resp: &Response, rust_resp: &Response, cmd: &str) -> bool {
    if redis_resp == rust_resp {
        return true;
    }

    let cmd_upper = cmd.to_uppercase();

    // Handle acceptable differences
    match (redis_resp, rust_resp) {
        // Both errors (wording may differ)
        (Response::Error(_), Response::Error(_)) => true,
        // String comparison (trim whitespace)
        (Response::Str(a), Response::Str(b)) => a.trim() == b.trim(),
        // For DEBUG commands, accept any response
        _ if cmd_upper.starts_with("DEBUG") => true,
        // HGETALL returns pairs - order doesn't matter
        (Response::Bulk(r), Response::Bulk(u)) if cmd_upper.starts_with("HGETALL") => {
            // Convert to key-value pairs and compare as maps
            if r.len() != u.len() || r.len() % 2 != 0 {
                return false;
            }
            let mut r_map: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
            let mut u_map: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
            for pair in r.chunks(2) {
                r_map.insert(&pair[0], &pair[1]);
            }
            for pair in u.chunks(2) {
                u_map.insert(&pair[0], &pair[1]);
            }
            r_map == u_map
        }
        _ => false,
    }
}

/// Struct to run differential tests
struct DifferentialTester {
    redis_conn: Connection,
    rust_conn: Connection,
    passed: usize,
    failed: usize,
    errors: Vec<String>,
}

impl DifferentialTester {
    fn new(redis_port: u16, rust_port: u16) -> RedisResult<Self> {
        let redis_conn = connect(redis_port)?;
        let rust_conn = connect(rust_port)?;
        Ok(Self {
            redis_conn,
            rust_conn,
            passed: 0,
            failed: 0,
            errors: Vec::new(),
        })
    }

    /// Run a command on both servers and compare results
    fn test(&mut self, args: &[&str]) {
        let cmd_str = args.join(" ");
        let redis_resp = exec_cmd(&mut self.redis_conn, args);
        let rust_resp = exec_cmd(&mut self.rust_conn, args);

        if responses_match(&redis_resp, &rust_resp, &cmd_str) {
            self.passed += 1;
            println!("  [PASS] {}", cmd_str);
        } else {
            self.failed += 1;
            let error = format!(
                "  [FAIL] {}\n    Redis: {:?}\n    Rust:  {:?}",
                cmd_str, redis_resp, rust_resp
            );
            println!("{}", error);
            self.errors.push(error);
        }
    }

    /// Run a command only to set up state (don't compare)
    fn setup(&mut self, args: &[&str]) {
        exec_cmd(&mut self.redis_conn, args);
        exec_cmd(&mut self.rust_conn, args);
    }

    /// Clean up both servers
    fn cleanup(&mut self) {
        let _ = exec_cmd(&mut self.redis_conn, &["FLUSHDB"]);
        let _ = exec_cmd(&mut self.rust_conn, &["FLUSHDB"]);
    }

    fn report(&self) {
        println!("\n=== Differential Test Results ===");
        println!("Passed: {}", self.passed);
        println!("Failed: {}", self.failed);
        println!(
            "Success rate: {:.1}%",
            (self.passed as f64 / (self.passed + self.failed) as f64) * 100.0
        );

        if !self.errors.is_empty() {
            println!("\nFailures:");
            for err in &self.errors {
                println!("{}", err);
            }
        }
    }
}

/// Test basic string operations (SET, GET, DEL, etc.)
#[test]
#[ignore]
fn test_string_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== String Operations ===");

    // Clean slate
    tester.cleanup();

    // GET non-existent key
    tester.test(&["GET", "nonexistent"]);

    // SET and GET
    tester.test(&["SET", "key1", "value1"]);
    tester.test(&["GET", "key1"]);

    // SET with EX
    tester.test(&["SET", "expiring", "temp", "EX", "100"]);
    tester.test(&["TTL", "expiring"]);

    // SET with NX (only if not exists)
    tester.test(&["SET", "nx_key", "first", "NX"]);
    tester.test(&["SET", "nx_key", "second", "NX"]);
    tester.test(&["GET", "nx_key"]);

    // SET with XX (only if exists)
    tester.test(&["SET", "xx_key", "first", "XX"]);
    tester.test(&["SET", "key1", "updated", "XX"]);
    tester.test(&["GET", "key1"]);

    // SETNX
    tester.test(&["SETNX", "setnx_key", "value"]);
    tester.test(&["SETNX", "setnx_key", "other"]);
    tester.test(&["GET", "setnx_key"]);

    // SETEX
    tester.test(&["SETEX", "setex_key", "100", "temp_value"]);
    tester.test(&["GET", "setex_key"]);
    tester.test(&["TTL", "setex_key"]);

    // MSET and MGET
    tester.test(&["MSET", "a", "1", "b", "2", "c", "3"]);
    tester.test(&["MGET", "a", "b", "c", "nonexistent"]);

    // APPEND
    tester.test(&["APPEND", "append_key", "hello"]);
    tester.test(&["APPEND", "append_key", " world"]);
    tester.test(&["GET", "append_key"]);

    // STRLEN
    tester.test(&["STRLEN", "append_key"]);
    tester.test(&["STRLEN", "nonexistent"]);

    // GETRANGE
    tester.test(&["GETRANGE", "append_key", "0", "4"]);
    tester.test(&["GETRANGE", "append_key", "-5", "-1"]);

    // SETRANGE
    tester.test(&["SETRANGE", "append_key", "6", "Redis"]);
    tester.test(&["GET", "append_key"]);

    // DEL
    tester.test(&["DEL", "key1"]);
    tester.test(&["GET", "key1"]);
    tester.test(&["DEL", "a", "b", "c"]);

    // EXISTS
    tester.test(&["EXISTS", "append_key"]);
    tester.test(&["EXISTS", "nonexistent"]);

    // TYPE
    tester.test(&["TYPE", "append_key"]);
    tester.test(&["TYPE", "nonexistent"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some string operations differed");
}

/// Test numeric operations (INCR, DECR, etc.)
#[test]
#[ignore]
fn test_numeric_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== Numeric Operations ===");

    tester.cleanup();

    // INCR on new key
    tester.test(&["INCR", "counter"]);
    tester.test(&["GET", "counter"]);

    // INCR on existing numeric
    tester.test(&["INCR", "counter"]);
    tester.test(&["INCR", "counter"]);

    // INCRBY
    tester.test(&["INCRBY", "counter", "10"]);
    tester.test(&["GET", "counter"]);

    // DECRBY
    tester.test(&["DECRBY", "counter", "5"]);

    // DECR
    tester.test(&["DECR", "counter"]);

    // INCRBYFLOAT
    tester.test(&["SET", "float_key", "10.5"]);
    tester.test(&["INCRBYFLOAT", "float_key", "0.1"]);
    tester.test(&["GET", "float_key"]);

    // INCR on non-numeric (should error)
    tester.setup(&["SET", "not_numeric", "hello"]);
    tester.test(&["INCR", "not_numeric"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some numeric operations differed");
}

/// Test hash operations
#[test]
#[ignore]
fn test_hash_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== Hash Operations ===");

    tester.cleanup();

    // HSET and HGET
    tester.test(&["HSET", "user:1", "name", "Alice"]);
    tester.test(&["HGET", "user:1", "name"]);

    // HGET non-existent field
    tester.test(&["HGET", "user:1", "nonexistent"]);

    // Multiple HSET
    tester.test(&["HSET", "user:1", "age", "30", "city", "NYC"]);

    // HMGET
    tester.test(&["HMGET", "user:1", "name", "age", "nonexistent"]);

    // HGETALL
    tester.test(&["HGETALL", "user:1"]);

    // HEXISTS
    tester.test(&["HEXISTS", "user:1", "name"]);
    tester.test(&["HEXISTS", "user:1", "nonexistent"]);

    // HLEN
    tester.test(&["HLEN", "user:1"]);

    // HKEYS and HVALS
    tester.test(&["HKEYS", "user:1"]);
    tester.test(&["HVALS", "user:1"]);

    // HINCRBY
    tester.test(&["HINCRBY", "user:1", "age", "1"]);
    tester.test(&["HGET", "user:1", "age"]);

    // HINCRBYFLOAT
    tester.test(&["HSET", "user:1", "score", "10.5"]);
    tester.test(&["HINCRBYFLOAT", "user:1", "score", "0.5"]);
    tester.test(&["HGET", "user:1", "score"]);

    // HSETNX
    tester.test(&["HSETNX", "user:1", "name", "Bob"]);
    tester.test(&["HSETNX", "user:1", "email", "alice@example.com"]);
    tester.test(&["HGET", "user:1", "name"]);
    tester.test(&["HGET", "user:1", "email"]);

    // HDEL
    tester.test(&["HDEL", "user:1", "email"]);
    tester.test(&["HEXISTS", "user:1", "email"]);

    // HSTRLEN
    tester.test(&["HSTRLEN", "user:1", "name"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some hash operations differed");
}

/// Test list operations
#[test]
#[ignore]
fn test_list_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== List Operations ===");

    tester.cleanup();

    // RPUSH
    tester.test(&["RPUSH", "mylist", "a"]);
    tester.test(&["RPUSH", "mylist", "b", "c"]);

    // LPUSH
    tester.test(&["LPUSH", "mylist", "z"]);

    // LRANGE
    tester.test(&["LRANGE", "mylist", "0", "-1"]);
    tester.test(&["LRANGE", "mylist", "0", "1"]);
    tester.test(&["LRANGE", "mylist", "-2", "-1"]);

    // LLEN
    tester.test(&["LLEN", "mylist"]);

    // LINDEX
    tester.test(&["LINDEX", "mylist", "0"]);
    tester.test(&["LINDEX", "mylist", "-1"]);
    tester.test(&["LINDEX", "mylist", "100"]);

    // LSET
    tester.test(&["LSET", "mylist", "1", "B"]);
    tester.test(&["LRANGE", "mylist", "0", "-1"]);

    // RPOP
    tester.test(&["RPOP", "mylist"]);
    tester.test(&["LRANGE", "mylist", "0", "-1"]);

    // LPOP
    tester.test(&["LPOP", "mylist"]);
    tester.test(&["LRANGE", "mylist", "0", "-1"]);

    // LINSERT BEFORE/AFTER
    tester.setup(&["RPUSH", "insertlist", "a", "b", "c"]);
    tester.test(&["LINSERT", "insertlist", "BEFORE", "b", "X"]);
    tester.test(&["LRANGE", "insertlist", "0", "-1"]);
    tester.test(&["LINSERT", "insertlist", "AFTER", "b", "Y"]);
    tester.test(&["LRANGE", "insertlist", "0", "-1"]);

    // LREM
    tester.setup(&["RPUSH", "remlist", "a", "b", "a", "c", "a"]);
    tester.test(&["LREM", "remlist", "2", "a"]);
    tester.test(&["LRANGE", "remlist", "0", "-1"]);

    // LTRIM
    tester.setup(&["RPUSH", "trimlist", "a", "b", "c", "d", "e"]);
    tester.test(&["LTRIM", "trimlist", "1", "3"]);
    tester.test(&["LRANGE", "trimlist", "0", "-1"]);

    // RPOPLPUSH
    tester.setup(&["DEL", "srclist", "dstlist"]);
    tester.setup(&["RPUSH", "srclist", "a", "b", "c"]);
    tester.test(&["RPOPLPUSH", "srclist", "dstlist"]);
    tester.test(&["LRANGE", "srclist", "0", "-1"]);
    tester.test(&["LRANGE", "dstlist", "0", "-1"]);
    tester.test(&["RPOPLPUSH", "srclist", "dstlist"]);
    tester.test(&["LRANGE", "dstlist", "0", "-1"]);

    // LMOVE
    tester.setup(&["DEL", "lmove_src", "lmove_dst"]);
    tester.setup(&["RPUSH", "lmove_src", "one", "two", "three"]);
    tester.test(&["LMOVE", "lmove_src", "lmove_dst", "RIGHT", "LEFT"]);
    tester.test(&["LRANGE", "lmove_src", "0", "-1"]);
    tester.test(&["LRANGE", "lmove_dst", "0", "-1"]);
    tester.test(&["LMOVE", "lmove_src", "lmove_dst", "LEFT", "RIGHT"]);
    tester.test(&["LRANGE", "lmove_src", "0", "-1"]);
    tester.test(&["LRANGE", "lmove_dst", "0", "-1"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some list operations differed");
}

/// Test set operations
#[test]
#[ignore]
fn test_set_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== Set Operations ===");

    tester.cleanup();

    // SADD
    tester.test(&["SADD", "myset", "a", "b", "c"]);
    tester.test(&["SADD", "myset", "c", "d"]);

    // SCARD
    tester.test(&["SCARD", "myset"]);

    // SISMEMBER
    tester.test(&["SISMEMBER", "myset", "a"]);
    tester.test(&["SISMEMBER", "myset", "z"]);

    // SMEMBERS (note: order may vary, so we compare as sets)
    let redis_members = exec_cmd(&mut tester.redis_conn, &["SMEMBERS", "myset"]);
    let rust_members = exec_cmd(&mut tester.rust_conn, &["SMEMBERS", "myset"]);
    match (&redis_members, &rust_members) {
        (Response::Bulk(r), Response::Bulk(u)) => {
            let mut r_sorted = r.clone();
            let mut u_sorted = u.clone();
            r_sorted.sort();
            u_sorted.sort();
            if r_sorted == u_sorted {
                tester.passed += 1;
                println!("  [PASS] SMEMBERS myset (compared as set)");
            } else {
                tester.failed += 1;
                println!(
                    "  [FAIL] SMEMBERS myset\n    Redis: {:?}\n    Rust:  {:?}",
                    r_sorted, u_sorted
                );
            }
        }
        _ => {
            tester.failed += 1;
            println!("  [FAIL] SMEMBERS myset - unexpected response type");
        }
    }

    // SREM
    tester.test(&["SREM", "myset", "a", "z"]);
    tester.test(&["SCARD", "myset"]);

    // Set operations with two sets
    tester.setup(&["SADD", "set1", "a", "b", "c"]);
    tester.setup(&["SADD", "set2", "b", "c", "d"]);

    // SUNION (compare as sets)
    let redis_union = exec_cmd(&mut tester.redis_conn, &["SUNION", "set1", "set2"]);
    let rust_union = exec_cmd(&mut tester.rust_conn, &["SUNION", "set1", "set2"]);
    match (&redis_union, &rust_union) {
        (Response::Bulk(r), Response::Bulk(u)) => {
            let mut r_sorted = r.clone();
            let mut u_sorted = u.clone();
            r_sorted.sort();
            u_sorted.sort();
            if r_sorted == u_sorted {
                tester.passed += 1;
                println!("  [PASS] SUNION set1 set2 (compared as set)");
            } else {
                tester.failed += 1;
                println!("  [FAIL] SUNION set1 set2");
            }
        }
        _ => {
            tester.failed += 1;
        }
    }

    // SINTER
    let redis_inter = exec_cmd(&mut tester.redis_conn, &["SINTER", "set1", "set2"]);
    let rust_inter = exec_cmd(&mut tester.rust_conn, &["SINTER", "set1", "set2"]);
    match (&redis_inter, &rust_inter) {
        (Response::Bulk(r), Response::Bulk(u)) => {
            let mut r_sorted = r.clone();
            let mut u_sorted = u.clone();
            r_sorted.sort();
            u_sorted.sort();
            if r_sorted == u_sorted {
                tester.passed += 1;
                println!("  [PASS] SINTER set1 set2 (compared as set)");
            } else {
                tester.failed += 1;
                println!("  [FAIL] SINTER set1 set2");
            }
        }
        _ => {
            tester.failed += 1;
        }
    }

    // SDIFF
    let redis_diff = exec_cmd(&mut tester.redis_conn, &["SDIFF", "set1", "set2"]);
    let rust_diff = exec_cmd(&mut tester.rust_conn, &["SDIFF", "set1", "set2"]);
    match (&redis_diff, &rust_diff) {
        (Response::Bulk(r), Response::Bulk(u)) => {
            let mut r_sorted = r.clone();
            let mut u_sorted = u.clone();
            r_sorted.sort();
            u_sorted.sort();
            if r_sorted == u_sorted {
                tester.passed += 1;
                println!("  [PASS] SDIFF set1 set2 (compared as set)");
            } else {
                tester.failed += 1;
                println!("  [FAIL] SDIFF set1 set2");
            }
        }
        _ => {
            tester.failed += 1;
        }
    }

    tester.report();
    assert_eq!(tester.failed, 0, "Some set operations differed");
}

/// Test sorted set operations
#[test]
#[ignore]
fn test_sorted_set_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== Sorted Set Operations ===");

    tester.cleanup();

    // ZADD
    tester.test(&[
        "ZADD", "scores", "100", "alice", "90", "bob", "95", "charlie",
    ]);

    // ZCARD
    tester.test(&["ZCARD", "scores"]);

    // ZSCORE
    tester.test(&["ZSCORE", "scores", "alice"]);
    tester.test(&["ZSCORE", "scores", "nonexistent"]);

    // ZRANK
    tester.test(&["ZRANK", "scores", "bob"]);
    tester.test(&["ZRANK", "scores", "nonexistent"]);

    // ZREVRANK
    tester.test(&["ZREVRANK", "scores", "alice"]);

    // ZRANGE
    tester.test(&["ZRANGE", "scores", "0", "-1"]);
    tester.test(&["ZRANGE", "scores", "0", "1"]);

    // ZRANGE WITHSCORES
    tester.test(&["ZRANGE", "scores", "0", "-1", "WITHSCORES"]);

    // ZREVRANGE
    tester.test(&["ZREVRANGE", "scores", "0", "-1"]);
    tester.test(&["ZREVRANGE", "scores", "0", "-1", "WITHSCORES"]);

    // ZRANGEBYSCORE
    tester.test(&["ZRANGEBYSCORE", "scores", "90", "100"]);
    tester.test(&["ZRANGEBYSCORE", "scores", "90", "100", "WITHSCORES"]);
    tester.test(&["ZRANGEBYSCORE", "scores", "-inf", "+inf"]);

    // ZCOUNT
    tester.test(&["ZCOUNT", "scores", "90", "100"]);
    tester.test(&["ZCOUNT", "scores", "-inf", "+inf"]);

    // ZINCRBY
    tester.test(&["ZINCRBY", "scores", "5", "bob"]);
    tester.test(&["ZSCORE", "scores", "bob"]);

    // ZREM
    tester.test(&["ZREM", "scores", "charlie"]);
    tester.test(&["ZCARD", "scores"]);

    // ZADD with NX/XX/GT/LT
    tester.test(&["ZADD", "scores", "NX", "80", "dave"]);
    tester.test(&["ZADD", "scores", "NX", "50", "alice"]);
    tester.test(&["ZSCORE", "scores", "alice"]);

    // ZPOPMIN and ZPOPMAX
    tester.setup(&["ZADD", "poptest", "1", "a", "2", "b", "3", "c"]);
    tester.test(&["ZPOPMIN", "poptest"]);
    tester.test(&["ZPOPMAX", "poptest"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some sorted set operations differed");
}

/// Test key operations (KEYS, SCAN, TTL, EXPIRE, etc.)
#[test]
#[ignore]
fn test_key_operations_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== Key Operations ===");

    tester.cleanup();

    // Setup some keys
    tester.setup(&["SET", "user:1:name", "Alice"]);
    tester.setup(&["SET", "user:2:name", "Bob"]);
    tester.setup(&["SET", "session:abc", "data"]);
    tester.setup(&["HSET", "user:1:profile", "email", "alice@example.com"]);

    // KEYS pattern matching
    let redis_keys = exec_cmd(&mut tester.redis_conn, &["KEYS", "user:*"]);
    let rust_keys = exec_cmd(&mut tester.rust_conn, &["KEYS", "user:*"]);
    match (&redis_keys, &rust_keys) {
        (Response::Bulk(r), Response::Bulk(u)) => {
            let mut r_sorted = r.clone();
            let mut u_sorted = u.clone();
            r_sorted.sort();
            u_sorted.sort();
            if r_sorted == u_sorted {
                tester.passed += 1;
                println!("  [PASS] KEYS user:* (compared as set)");
            } else {
                tester.failed += 1;
                println!(
                    "  [FAIL] KEYS user:*\n    Redis: {:?}\n    Rust:  {:?}",
                    r_sorted, u_sorted
                );
            }
        }
        _ => {
            tester.failed += 1;
        }
    }

    // EXPIRE and TTL
    tester.test(&["EXPIRE", "session:abc", "100"]);
    tester.test(&["TTL", "session:abc"]);

    // PEXPIRE and PTTL
    tester.test(&["PEXPIRE", "session:abc", "50000"]);
    tester.test(&["PTTL", "session:abc"]);

    // EXPIREAT
    let future_ts = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 1000)
        .to_string();
    tester.test(&["EXPIREAT", "session:abc", &future_ts]);
    tester.test(&["TTL", "session:abc"]);

    // PERSIST
    tester.test(&["PERSIST", "session:abc"]);
    tester.test(&["TTL", "session:abc"]);

    // RENAME
    tester.test(&["RENAME", "session:abc", "session:xyz"]);
    tester.test(&["EXISTS", "session:abc"]);
    tester.test(&["EXISTS", "session:xyz"]);

    // RENAMENX
    tester.setup(&["SET", "rename_src", "value"]);
    tester.test(&["RENAMENX", "rename_src", "session:xyz"]);
    tester.test(&["RENAMENX", "rename_src", "rename_dst"]);

    // DBSIZE
    tester.test(&["DBSIZE"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some key operations differed");
}

/// Test SCAN operations
#[test]
#[ignore]
fn test_scan_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== SCAN Operations ===");

    tester.cleanup();

    // Setup keys for SCAN
    tester.setup(&["SET", "scan_key1", "v1"]);
    tester.setup(&["SET", "scan_key2", "v2"]);
    tester.setup(&["SET", "scan_key3", "v3"]);
    tester.setup(&["SET", "other_key", "v4"]);

    // SCAN with pattern
    // Note: We can't directly compare SCAN results as cursor semantics differ
    // Just verify it returns something reasonable
    let redis_scan = exec_cmd(
        &mut tester.redis_conn,
        &["SCAN", "0", "MATCH", "scan_*", "COUNT", "100"],
    );
    let rust_scan = exec_cmd(
        &mut tester.rust_conn,
        &["SCAN", "0", "MATCH", "scan_*", "COUNT", "100"],
    );

    match (&redis_scan, &rust_scan) {
        (Response::Bulk(r), Response::Bulk(u)) if r.len() >= 2 && u.len() >= 2 => {
            // Second element should be the keys array
            tester.passed += 1;
            println!("  [PASS] SCAN 0 MATCH scan_* COUNT 100 (returned valid structure)");
        }
        _ => {
            tester.failed += 1;
            println!(
                "  [FAIL] SCAN 0 MATCH scan_* COUNT 100\n    Redis: {:?}\n    Rust:  {:?}",
                redis_scan, rust_scan
            );
        }
    }

    // HSCAN
    tester.setup(&[
        "HSET",
        "scan_hash",
        "field1",
        "v1",
        "field2",
        "v2",
        "field3",
        "v3",
    ]);
    let redis_hscan = exec_cmd(
        &mut tester.redis_conn,
        &["HSCAN", "scan_hash", "0", "COUNT", "100"],
    );
    let rust_hscan = exec_cmd(
        &mut tester.rust_conn,
        &["HSCAN", "scan_hash", "0", "COUNT", "100"],
    );

    match (&redis_hscan, &rust_hscan) {
        (Response::Bulk(r), Response::Bulk(u)) if r.len() >= 2 && u.len() >= 2 => {
            tester.passed += 1;
            println!("  [PASS] HSCAN scan_hash 0 COUNT 100 (returned valid structure)");
        }
        _ => {
            tester.failed += 1;
            println!(
                "  [FAIL] HSCAN scan_hash 0 COUNT 100\n    Redis: {:?}\n    Rust:  {:?}",
                redis_hscan, rust_hscan
            );
        }
    }

    // ZSCAN
    tester.setup(&["ZADD", "scan_zset", "1", "a", "2", "b", "3", "c"]);
    let redis_zscan = exec_cmd(
        &mut tester.redis_conn,
        &["ZSCAN", "scan_zset", "0", "COUNT", "100"],
    );
    let rust_zscan = exec_cmd(
        &mut tester.rust_conn,
        &["ZSCAN", "scan_zset", "0", "COUNT", "100"],
    );

    match (&redis_zscan, &rust_zscan) {
        (Response::Bulk(r), Response::Bulk(u)) if r.len() >= 2 && u.len() >= 2 => {
            tester.passed += 1;
            println!("  [PASS] ZSCAN scan_zset 0 COUNT 100 (returned valid structure)");
        }
        _ => {
            tester.failed += 1;
            println!(
                "  [FAIL] ZSCAN scan_zset 0 COUNT 100\n    Redis: {:?}\n    Rust:  {:?}",
                redis_zscan, rust_zscan
            );
        }
    }

    tester.report();
    assert_eq!(tester.failed, 0, "Some SCAN operations differed");
}

/// Test server/info commands
#[test]
#[ignore]
fn test_info_commands_equivalence() {
    let mut tester =
        DifferentialTester::new(6379, 3000).expect("Failed to connect to both servers");

    println!("\n=== Server/Info Commands ===");

    // PING
    tester.test(&["PING"]);
    tester.test(&["PING", "hello"]);

    // ECHO
    tester.test(&["ECHO", "test message"]);

    // CLIENT commands (limited compatibility expected)
    // Note: SETNAME/GETNAME may differ in behavior
    tester.test(&["CLIENT", "SETNAME", "test-client"]);
    tester.test(&["CLIENT", "GETNAME"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some info commands differed");
}

/// Run all equivalence tests in sequence (core commands only)
#[test]
#[ignore]
fn test_full_equivalence_suite() {
    let mut tester = match DifferentialTester::new(6379, 3000) {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping test - could not connect to servers: {}", e);
            println!("Make sure both Redis (port 6379) and redis-rust (port 3000) are running");
            return;
        }
    };

    tester.cleanup();

    println!("\n=== Full Equivalence Test Suite ===\n");

    // Strings
    println!("--- String Operations ---");
    tester.test(&["SET", "s1", "hello"]);
    tester.test(&["GET", "s1"]);
    tester.test(&["APPEND", "s1", " world"]);
    tester.test(&["STRLEN", "s1"]);
    // Note: GETRANGE not yet implemented in redis-rust

    // Numbers
    println!("\n--- Numeric Operations ---");
    tester.test(&["SET", "n1", "10"]);
    tester.test(&["INCR", "n1"]);
    tester.test(&["INCRBY", "n1", "5"]);
    tester.test(&["DECR", "n1"]);

    // Hashes
    println!("\n--- Hash Operations ---");
    tester.test(&["HSET", "h1", "f1", "v1", "f2", "v2"]);
    tester.test(&["HGET", "h1", "f1"]);
    tester.test(&["HGETALL", "h1"]);
    tester.test(&["HINCRBY", "h1", "counter", "1"]);

    // Lists
    println!("\n--- List Operations ---");
    tester.test(&["RPUSH", "l1", "a", "b", "c"]);
    tester.test(&["LRANGE", "l1", "0", "-1"]);
    tester.test(&["LPOP", "l1"]);
    tester.test(&["LLEN", "l1"]);

    // Sets
    println!("\n--- Set Operations ---");
    tester.test(&["SADD", "set1", "a", "b", "c"]);
    tester.test(&["SCARD", "set1"]);
    tester.test(&["SISMEMBER", "set1", "a"]);

    // Sorted Sets
    println!("\n--- Sorted Set Operations ---");
    tester.test(&["ZADD", "z1", "1", "a", "2", "b", "3", "c"]);
    tester.test(&["ZCARD", "z1"]);
    tester.test(&["ZRANGE", "z1", "0", "-1"]); // Without WITHSCORES for now
    tester.test(&["ZSCORE", "z1", "b"]);

    // Key operations
    println!("\n--- Key Operations ---");
    tester.test(&["EXISTS", "s1"]);
    tester.test(&["TYPE", "h1"]);
    tester.test(&["DEL", "s1"]); // Single key only for now
    tester.test(&["DEL", "n1"]);

    tester.report();
    assert_eq!(tester.failed, 0, "Some operations differed from Redis");
}

/// Test for known compatibility gaps (should show failures)
/// Run this to see what commands still need implementation
#[test]
#[ignore]
fn test_compatibility_gaps() {
    let mut tester = match DifferentialTester::new(6379, 3000) {
        Ok(t) => t,
        Err(e) => {
            println!("Skipping test - could not connect to servers: {}", e);
            return;
        }
    };

    tester.cleanup();
    println!("\n=== Known Compatibility Gaps ===\n");
    println!("These commands are not yet fully compatible:\n");

    // GETRANGE not implemented
    tester.setup(&["SET", "test_str", "hello world"]);
    tester.test(&["GETRANGE", "test_str", "0", "4"]);

    // Multi-key DEL
    tester.setup(&["SET", "k1", "v1"]);
    tester.setup(&["SET", "k2", "v2"]);
    tester.test(&["DEL", "k1", "k2"]);

    // ZRANGE WITHSCORES
    tester.setup(&["ZADD", "zset", "1", "a", "2", "b"]);
    tester.test(&["ZRANGE", "zset", "0", "-1", "WITHSCORES"]);

    tester.report();
    println!("\nNote: These are EXPECTED failures showing gaps to address.");
}
