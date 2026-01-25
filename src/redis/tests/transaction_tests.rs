//! Transaction tests - MULTI/EXEC/DISCARD/WATCH and HINCRBY error handling

use super::super::{Command, CommandExecutor, RespValue, SDS};

// ============================================
// Transaction Tests (MULTI/EXEC/DISCARD)
// ============================================

#[test]
fn test_multi_exec_basic() {
    let mut executor = CommandExecutor::new();

    // Start transaction
    executor.execute(&Command::Multi);

    // Queue commands
    let r1 = executor.execute(&Command::Set {
        key: "foo".to_string(),
        value: SDS::from_str("bar"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });
    assert_eq!(r1, RespValue::simple("QUEUED"));

    let r2 = executor.execute(&Command::Incr("counter".to_string()));
    assert_eq!(r2, RespValue::simple("QUEUED"));

    // Execute transaction
    let result = executor.execute(&Command::Exec);

    if let RespValue::Array(Some(results)) = result {
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], RespValue::simple("OK"));
        assert_eq!(results[1], RespValue::Integer(1));
    } else {
        panic!("Expected array result from EXEC");
    }

    // Values should be set
    assert_eq!(
        executor.execute(&Command::Get("foo".to_string())),
        RespValue::BulkString(Some(b"bar".to_vec()))
    );
}

#[test]
fn test_multi_discard() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::Multi);
    executor.execute(&Command::Set {
        key: "foo".to_string(),
        value: SDS::from_str("bar"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });

    let result = executor.execute(&Command::Discard);
    assert_eq!(result, RespValue::simple("OK"));

    // Value should NOT be set
    assert_eq!(
        executor.execute(&Command::Get("foo".to_string())),
        RespValue::BulkString(None)
    );
}

#[test]
fn test_exec_without_multi() {
    let mut executor = CommandExecutor::new();

    let result = executor.execute(&Command::Exec);
    assert!(matches!(result, RespValue::Error(_)));
}

#[test]
fn test_nested_multi_error() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::Multi);
    let result = executor.execute(&Command::Multi);
    assert!(matches!(result, RespValue::Error(_)));
}

// ============================================
// WATCH Tests
// ============================================

#[test]
fn test_watch_unmodified_key() {
    let mut executor = CommandExecutor::new();

    // Set initial value
    executor.execute(&Command::Set {
        key: "watched".to_string(),
        value: SDS::from_str("initial"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });

    // Watch the key
    executor.execute(&Command::Watch(vec!["watched".to_string()]));

    // Start transaction
    executor.execute(&Command::Multi);
    executor.execute(&Command::Set {
        key: "watched".to_string(),
        value: SDS::from_str("updated"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });

    // Execute - should succeed since key wasn't modified
    let result = executor.execute(&Command::Exec);
    assert!(matches!(result, RespValue::Array(Some(_))));

    // Value should be updated
    assert_eq!(
        executor.execute(&Command::Get("watched".to_string())),
        RespValue::BulkString(Some(b"updated".to_vec()))
    );
}

#[test]
fn test_unwatch() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::Watch(vec!["key".to_string()]));
    let result = executor.execute(&Command::Unwatch);
    assert_eq!(result, RespValue::simple("OK"));
}

// ============================================
// HINCRBY Error Handling Tests
// ============================================

#[test]
fn test_hincrby_non_integer_value() {
    let mut executor = CommandExecutor::new();

    // Set hash field to non-integer string
    executor.execute(&Command::HSet(
        "myhash".to_string(),
        vec![(SDS::from_str("field"), SDS::from_str("notanumber"))],
    ));

    // HINCRBY should fail
    let result = executor.execute(&Command::HIncrBy(
        "myhash".to_string(),
        SDS::from_str("field"),
        1,
    ));
    assert!(matches!(result, RespValue::Error(_)));
}

#[test]
fn test_hincrby_overflow() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::HSet(
        "myhash".to_string(),
        vec![(SDS::from_str("field"), SDS::from_str(&i64::MAX.to_string()))],
    ));

    // This should overflow
    let result = executor.execute(&Command::HIncrBy(
        "myhash".to_string(),
        SDS::from_str("field"),
        1,
    ));
    assert!(matches!(result, RespValue::Error(_)));
}
