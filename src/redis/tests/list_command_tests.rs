//! List command tests - LSET, LTRIM, RPOPLPUSH, LMOVE, multi-key DEL

use super::super::{Command, CommandExecutor, RespValue, SDS};

// ============================================
// Multi-key DEL Tests
// ============================================

#[test]
fn test_del_multiple_keys() {
    let mut executor = CommandExecutor::new();

    // Set multiple keys
    for key in ["key1", "key2", "key3"] {
        executor.execute(&Command::Set {
            key: key.to_string(),
            value: SDS::from_str("value"),
            ex: None,
            px: None,
            nx: false,
            xx: false,
            get: false,
        });
    }

    // Delete all at once
    let cmd = Command::Del(vec![
        "key1".to_string(),
        "key2".to_string(),
        "key3".to_string(),
        "nonexistent".to_string(),
    ]);
    let result = executor.execute(&cmd);

    // Should return count of deleted keys (3, not 4)
    assert_eq!(result, RespValue::Integer(3));

    // All keys should be gone
    for key in ["key1", "key2", "key3"] {
        assert_eq!(
            executor.execute(&Command::Get(key.to_string())),
            RespValue::BulkString(None)
        );
    }
}

// ============================================
// LSET Tests
// ============================================

#[test]
fn test_lset_positive_index() {
    let mut executor = CommandExecutor::new();

    // Create list
    executor.execute(&Command::RPush(
        "mylist".to_string(),
        vec![SDS::from_str("a"), SDS::from_str("b"), SDS::from_str("c")],
    ));

    // Set element at index 1
    let cmd = Command::LSet("mylist".to_string(), 1, SDS::from_str("B"));
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::simple("OK"));

    // Verify
    let lrange = Command::LRange("mylist".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements[1], RespValue::BulkString(Some(b"B".to_vec())));
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_lset_negative_index() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "mylist".to_string(),
        vec![SDS::from_str("a"), SDS::from_str("b"), SDS::from_str("c")],
    ));

    // Set last element using negative index
    let cmd = Command::LSet("mylist".to_string(), -1, SDS::from_str("C"));
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::simple("OK"));

    let lrange = Command::LRange("mylist".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements[2], RespValue::BulkString(Some(b"C".to_vec())));
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_lset_out_of_range() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "mylist".to_string(),
        vec![SDS::from_str("a")],
    ));

    let cmd = Command::LSet("mylist".to_string(), 5, SDS::from_str("X"));
    let result = executor.execute(&cmd);

    assert!(matches!(result, RespValue::Error(_)));
}

// ============================================
// LTRIM Tests
// ============================================

#[test]
fn test_ltrim_basic() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "mylist".to_string(),
        vec![
            SDS::from_str("a"),
            SDS::from_str("b"),
            SDS::from_str("c"),
            SDS::from_str("d"),
            SDS::from_str("e"),
        ],
    ));

    // Keep only elements 1-3 (b, c, d)
    let cmd = Command::LTrim("mylist".to_string(), 1, 3);
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::simple("OK"));

    let lrange = Command::LRange("mylist".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"b".to_vec())));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"c".to_vec())));
        assert_eq!(elements[2], RespValue::BulkString(Some(b"d".to_vec())));
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_ltrim_negative_indices() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "mylist".to_string(),
        vec![SDS::from_str("a"), SDS::from_str("b"), SDS::from_str("c")],
    ));

    // Keep last 2 elements
    let cmd = Command::LTrim("mylist".to_string(), -2, -1);
    executor.execute(&cmd);

    let lrange = Command::LRange("mylist".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"b".to_vec())));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"c".to_vec())));
    } else {
        panic!("Expected array");
    }
}

// ============================================
// RPOPLPUSH Tests
// ============================================

#[test]
fn test_rpoplpush_basic() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "source".to_string(),
        vec![SDS::from_str("a"), SDS::from_str("b"), SDS::from_str("c")],
    ));

    let cmd = Command::RPopLPush("source".to_string(), "dest".to_string());
    let result = executor.execute(&cmd);

    // Should return the popped element
    assert_eq!(result, RespValue::BulkString(Some(b"c".to_vec())));

    // Source should have [a, b]
    let lrange = Command::LRange("source".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements.len(), 2);
    } else {
        panic!("Expected array");
    }

    // Dest should have [c]
    let lrange = Command::LRange("dest".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"c".to_vec())));
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_rpoplpush_same_list() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "mylist".to_string(),
        vec![SDS::from_str("a"), SDS::from_str("b"), SDS::from_str("c")],
    ));

    // Rotate: pop from right, push to left
    let cmd = Command::RPopLPush("mylist".to_string(), "mylist".to_string());
    executor.execute(&cmd);

    let lrange = Command::LRange("mylist".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"c".to_vec())));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"a".to_vec())));
        assert_eq!(elements[2], RespValue::BulkString(Some(b"b".to_vec())));
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_rpoplpush_empty_source() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::RPopLPush("empty".to_string(), "dest".to_string());
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::BulkString(None));
}

// ============================================
// LMOVE Tests
// ============================================

#[test]
fn test_lmove_left_left() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "src".to_string(),
        vec![SDS::from_str("a"), SDS::from_str("b"), SDS::from_str("c")],
    ));

    let cmd = Command::LMove {
        source: "src".to_string(),
        dest: "dst".to_string(),
        wherefrom: "LEFT".to_string(),
        whereto: "LEFT".to_string(),
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::BulkString(Some(b"a".to_vec())));

    // src should be [b, c]
    let llen = Command::LLen("src".to_string());
    assert_eq!(executor.execute(&llen), RespValue::Integer(2));

    // dst should be [a]
    let lrange = Command::LRange("dst".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements[0], RespValue::BulkString(Some(b"a".to_vec())));
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_lmove_right_right() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::RPush(
        "src".to_string(),
        vec![SDS::from_str("a"), SDS::from_str("b"), SDS::from_str("c")],
    ));
    executor.execute(&Command::RPush("dst".to_string(), vec![SDS::from_str("x")]));

    let cmd = Command::LMove {
        source: "src".to_string(),
        dest: "dst".to_string(),
        wherefrom: "RIGHT".to_string(),
        whereto: "RIGHT".to_string(),
    };
    executor.execute(&cmd);

    // dst should be [x, c]
    let lrange = Command::LRange("dst".to_string(), 0, -1);
    if let RespValue::Array(Some(elements)) = executor.execute(&lrange) {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[1], RespValue::BulkString(Some(b"c".to_vec())));
    } else {
        panic!("Expected array");
    }
}
