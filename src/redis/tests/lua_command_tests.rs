//! Lua tests for new commands from Delancie plan
//!
//! Tests verifying new command implementations work correctly from Lua scripts.

use super::super::{Command, CommandExecutor, RespValue};

#[test]
fn test_lua_set_with_nx_option() {
    let mut executor = CommandExecutor::new();

    // SET NX should succeed when key doesn't exist
    let cmd = Command::Eval {
        script: r#"
            local result1 = redis.call("SET", "lock_key", "owner1", "NX")
            local result2 = redis.call("SET", "lock_key", "owner2", "NX")
            local value = redis.call("GET", "lock_key")
            -- Check if first succeeded (not nil) and second failed (nil)
            local first_ok = result1 ~= nil
            local second_nil = result2 == nil
            return {first_ok, second_nil, value}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 3);
        // First NX should succeed (returns true = 1)
        assert_eq!(elements[0], RespValue::Integer(1));
        // Second NX should return nil (key exists) -> true = 1
        assert_eq!(elements[1], RespValue::Integer(1));
        // Value should be from first SET
        assert_eq!(elements[2], RespValue::BulkString(Some(b"owner1".to_vec())));
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_set_with_xx_option() {
    let mut executor = CommandExecutor::new();

    // SET XX should fail when key doesn't exist, succeed when it does
    let cmd = Command::Eval {
        script: r#"
            local result1 = redis.call("SET", "xx_key", "value1", "XX")
            redis.call("SET", "xx_key", "initial")
            local result2 = redis.call("SET", "xx_key", "updated", "XX")
            local value = redis.call("GET", "xx_key")
            -- Check if first failed (nil) and second succeeded (not nil)
            local first_nil = result1 == nil
            local second_ok = result2 ~= nil
            return {first_nil, second_ok, value}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 3);
        // First XX should return nil (key doesn't exist) -> true = 1
        assert_eq!(elements[0], RespValue::Integer(1));
        // Second XX should succeed -> true = 1
        assert_eq!(elements[1], RespValue::Integer(1));
        // Value should be updated
        assert_eq!(
            elements[2],
            RespValue::BulkString(Some(b"updated".to_vec()))
        );
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_hincrby() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            redis.call("HSET", "stats", "views", "100")
            local new_val = redis.call("HINCRBY", "stats", "views", 10)
            local final_val = redis.call("HGET", "stats", "views")
            return {new_val, final_val}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::Integer(110));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"110".to_vec())));
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_rpoplpush() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            redis.call("RPUSH", "queue", "job1", "job2", "job3")
            local job = redis.call("RPOPLPUSH", "queue", "processing")
            local queue_len = redis.call("LLEN", "queue")
            local proc_len = redis.call("LLEN", "processing")
            return {job, queue_len, proc_len}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"job3".to_vec())));
        assert_eq!(elements[1], RespValue::Integer(2));
        assert_eq!(elements[2], RespValue::Integer(1));
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_lmove() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            redis.call("RPUSH", "src", "a", "b", "c")
            local moved = redis.call("LMOVE", "src", "dst", "RIGHT", "LEFT")
            local src_items = redis.call("LRANGE", "src", 0, -1)
            local dst_items = redis.call("LRANGE", "dst", 0, -1)
            return {moved, #src_items, #dst_items}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"c".to_vec())));
        assert_eq!(elements[1], RespValue::Integer(2)); // src has 2 items
        assert_eq!(elements[2], RespValue::Integer(1)); // dst has 1 item
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_zrangebyscore() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            redis.call("ZADD", "scores", 10, "alice", 20, "bob", 30, "charlie", 40, "dave")
            local results = redis.call("ZRANGEBYSCORE", "scores", "15", "35")
            return results
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"bob".to_vec())));
        assert_eq!(
            elements[1],
            RespValue::BulkString(Some(b"charlie".to_vec()))
        );
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_zcount() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            redis.call("ZADD", "scores", 10, "a", 20, "b", 30, "c", 40, "d", 50, "e")
            local count1 = redis.call("ZCOUNT", "scores", "20", "40")
            local count2 = redis.call("ZCOUNT", "scores", "-inf", "+inf")
            return {count1, count2}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::Integer(3)); // b, c, d
        assert_eq!(elements[1], RespValue::Integer(5)); // all
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_multi_key_del() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            redis.call("SET", "k1", "v1")
            redis.call("SET", "k2", "v2")
            redis.call("SET", "k3", "v3")
            local deleted = redis.call("DEL", "k1", "k2", "nonexistent")
            local exists_k3 = redis.call("EXISTS", "k3")
            return {deleted, exists_k3}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::Integer(2)); // k1 and k2 deleted
        assert_eq!(elements[1], RespValue::Integer(1)); // k3 still exists
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}

#[test]
fn test_lua_delancie_job_queue_pattern() {
    // Test a realistic Delancie-like job queue pattern
    let mut executor = CommandExecutor::new();

    let cmd = Command::Eval {
        script: r#"
            -- Simulate Delancie job processing pattern
            -- Add jobs to queue
            redis.call("RPUSH", "jobs:pending", "job:1", "job:2", "job:3")

            -- Atomically move job from pending to processing
            local job = redis.call("RPOPLPUSH", "jobs:pending", "jobs:processing")

            -- Track job metadata in hash
            redis.call("HSET", job, "status", "processing", "started_at", "12345")

            -- Increment job counter
            local job_count = redis.call("HINCRBY", "stats", "processed", 1)

            -- Get job status
            local status = redis.call("HGET", job, "status")

            return {job, status, job_count}
        "#
        .to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"job:3".to_vec())));
        assert_eq!(
            elements[1],
            RespValue::BulkString(Some(b"processing".to_vec()))
        );
        assert_eq!(elements[2], RespValue::Integer(1));
    } else {
        panic!("Expected array result, got {:?}", result);
    }
}
