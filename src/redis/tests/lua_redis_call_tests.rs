//! Lua redis.call() tests - Critical DST and semantic fix tests
//!
//! These tests verify that redis.call returns values immediately during
//! script execution (not deferred) and that math.random is deterministic.

use super::super::data::SDS;
use super::super::{Command, CommandExecutor, RespValue};
use crate::simulator::VirtualTime;

#[test]
fn test_redis_call_returns_value_immediately() {
    // This test verifies the critical semantic fix: redis.call must return
    // values immediately during script execution, not after script completion.
    let mut executor = CommandExecutor::new();

    // Script that sets a value and immediately retrieves it
    let cmd = Command::Eval {
        script: r#"
            redis.call("SET", "test_key", "test_value")
            local v = redis.call("GET", "test_key")
            return v
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    // If redis.call executes immediately, we get the value back
    // If it were deferred, we'd get nil
    assert_eq!(
        result,
        RespValue::BulkString(Some(b"test_value".to_vec())),
        "redis.call must return values immediately during script execution"
    );
}

#[test]
fn test_redis_call_incr_returns_new_value() {
    let mut executor = CommandExecutor::new();

    // Script that increments and uses the result
    let cmd = Command::Eval {
        script: r#"
            redis.call("SET", "counter", "10")
            local n1 = redis.call("INCR", "counter")
            local n2 = redis.call("INCR", "counter")
            return n1 + n2
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    // INCR returns 11, then 12, sum should be 23
    assert_eq!(result, RespValue::Integer(23));
}

#[test]
fn test_redis_call_chain_operations() {
    let mut executor = CommandExecutor::new();

    // Complex chain: LPUSH, LLEN, LPOP
    let cmd = Command::Eval {
        script: r#"
            redis.call("LPUSH", "mylist", "a", "b", "c")
            local len = redis.call("LLEN", "mylist")
            local first = redis.call("LPOP", "mylist")
            return {len, first}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    // Should return [3, "c"] (LPUSH adds c,b,a so c is first)
    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::Integer(3));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"c".to_vec())));
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_redis_pcall_returns_error_table() {
    let mut executor = CommandExecutor::new();

    // pcall on invalid operation should return error table, not propagate
    let cmd = Command::Eval {
        script: r#"
            redis.call("SET", "mystring", "hello")
            local result = redis.pcall("LPUSH", "mystring", "value")
            if result.err then
                return "got_error"
            else
                return "no_error"
            end
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    assert_eq!(
        result,
        RespValue::BulkString(Some(b"got_error".to_vec())),
        "redis.pcall must return error as table, not propagate"
    );
}

#[test]
fn test_math_random_deterministic_same_time() {
    // DST: math.random must be deterministic given same current_time
    let mut executor1 = CommandExecutor::new();
    let mut executor2 = CommandExecutor::new();

    // Both executors start at time 0, so should produce same random sequence
    let script = "math.randomseed(0); return math.random(1, 1000000)".to_string();

    let cmd1 = Command::Eval {
        script: script.clone(),
        keys: vec![],
        args: vec![],
    };
    let cmd2 = Command::Eval {
        script,
        keys: vec![],
        args: vec![],
    };

    let result1 = executor1.execute(&cmd1);
    let result2 = executor2.execute(&cmd2);

    assert_eq!(
        result1, result2,
        "Same script with same seed must produce identical results for DST"
    );
}

#[test]
fn test_math_random_deterministic_different_time() {
    // DST: Different times should produce different but reproducible results
    let mut executor1 = CommandExecutor::new();
    let mut executor2 = CommandExecutor::new();

    // Set different times
    executor1.set_time(VirtualTime::from_millis(1000));
    executor2.set_time(VirtualTime::from_millis(2000));

    let script = "return math.random(1, 1000000)".to_string();

    let cmd = Command::Eval {
        script,
        keys: vec![],
        args: vec![],
    };

    let result1 = executor1.execute(&cmd.clone());
    let result2 = executor2.execute(&cmd);

    // Results should be different due to different seeds
    assert_ne!(
        result1, result2,
        "Different times should produce different random values"
    );

    // But if we reset to same time, should match
    let mut executor3 = CommandExecutor::new();
    executor3.set_time(VirtualTime::from_millis(1000));
    let result3 = executor3.execute(&Command::Eval {
        script: "return math.random(1, 1000000)".to_string(),
        keys: vec![],
        args: vec![],
    });

    assert_eq!(
        result1, result3,
        "Same time must produce same random value for DST reproducibility"
    );
}

#[test]
fn test_redis_call_with_keys_argv() {
    let mut executor = CommandExecutor::new();

    // Use KEYS and ARGV with redis.call
    let cmd = Command::Eval {
        script: r#"
            redis.call("SET", KEYS[1], ARGV[1])
            return redis.call("GET", KEYS[1])
        "#
        .to_string(),
        keys: vec!["mykey".to_string()],
        args: vec![SDS::from_str("myvalue")],
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::BulkString(Some(b"myvalue".to_vec())));
}

#[test]
fn test_redis_call_hash_operations() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            redis.call("HSET", "myhash", "field1", "value1", "field2", "value2")
            local v1 = redis.call("HGET", "myhash", "field1")
            local v2 = redis.call("HGET", "myhash", "field2")
            return {v1, v2}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"value1".to_vec())));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"value2".to_vec())));
    } else {
        panic!("Expected array result");
    }
}
