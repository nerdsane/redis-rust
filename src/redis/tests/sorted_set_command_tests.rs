//! Sorted set command tests - ZCOUNT, ZRANGEBYSCORE

use super::super::{Command, CommandExecutor, RespValue, SDS};

// ============================================
// ZCOUNT Tests
// ============================================

#[test]
fn test_zcount_basic() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::ZAdd {
        key: "myzset".to_string(),
        pairs: vec![
            (1.0, SDS::from_str("one")),
            (2.0, SDS::from_str("two")),
            (3.0, SDS::from_str("three")),
        ],
        nx: false,
        xx: false,
        gt: false,
        lt: false,
        ch: false,
    });

    let cmd = Command::ZCount("myzset".to_string(), "1".to_string(), "2".to_string());
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::Integer(2));
}

#[test]
fn test_zcount_infinity() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::ZAdd {
        key: "myzset".to_string(),
        pairs: vec![
            (1.0, SDS::from_str("one")),
            (2.0, SDS::from_str("two")),
            (3.0, SDS::from_str("three")),
        ],
        nx: false,
        xx: false,
        gt: false,
        lt: false,
        ch: false,
    });

    let cmd = Command::ZCount("myzset".to_string(), "-inf".to_string(), "+inf".to_string());
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::Integer(3));
}

#[test]
fn test_zcount_exclusive() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::ZAdd {
        key: "myzset".to_string(),
        pairs: vec![
            (1.0, SDS::from_str("one")),
            (2.0, SDS::from_str("two")),
            (3.0, SDS::from_str("three")),
        ],
        nx: false,
        xx: false,
        gt: false,
        lt: false,
        ch: false,
    });

    // Exclusive min: (1 means > 1, not >= 1
    let cmd = Command::ZCount("myzset".to_string(), "(1".to_string(), "3".to_string());
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::Integer(2)); // two and three
}

// ============================================
// ZRANGEBYSCORE Tests
// ============================================

#[test]
fn test_zrangebyscore_basic() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::ZAdd {
        key: "myzset".to_string(),
        pairs: vec![
            (1.0, SDS::from_str("one")),
            (2.0, SDS::from_str("two")),
            (3.0, SDS::from_str("three")),
        ],
        nx: false,
        xx: false,
        gt: false,
        lt: false,
        ch: false,
    });

    let cmd = Command::ZRangeByScore {
        key: "myzset".to_string(),
        min: "1".to_string(),
        max: "2".to_string(),
        with_scores: false,
        limit: None,
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"one".to_vec())));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"two".to_vec())));
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_zrangebyscore_with_scores() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::ZAdd {
        key: "myzset".to_string(),
        pairs: vec![(1.5, SDS::from_str("one")), (2.5, SDS::from_str("two"))],
        nx: false,
        xx: false,
        gt: false,
        lt: false,
        ch: false,
    });

    let cmd = Command::ZRangeByScore {
        key: "myzset".to_string(),
        min: "-inf".to_string(),
        max: "+inf".to_string(),
        with_scores: true,
        limit: None,
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 4); // 2 members * 2 (member + score)
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_zrangebyscore_with_limit() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::ZAdd {
        key: "myzset".to_string(),
        pairs: vec![
            (1.0, SDS::from_str("a")),
            (2.0, SDS::from_str("b")),
            (3.0, SDS::from_str("c")),
            (4.0, SDS::from_str("d")),
        ],
        nx: false,
        xx: false,
        gt: false,
        lt: false,
        ch: false,
    });

    let cmd = Command::ZRangeByScore {
        key: "myzset".to_string(),
        min: "-inf".to_string(),
        max: "+inf".to_string(),
        with_scores: false,
        limit: Some((1, 2)), // skip 1, take 2
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], RespValue::BulkString(Some(b"b".to_vec())));
        assert_eq!(elements[1], RespValue::BulkString(Some(b"c".to_vec())));
    } else {
        panic!("Expected array");
    }
}
