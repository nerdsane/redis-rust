//! SCAN family tests - SCAN, HSCAN, ZSCAN

use super::super::{Command, CommandExecutor, RespValue, SDS};

// ============================================
// SCAN Tests
// ============================================

#[test]
fn test_scan_basic() {
    let mut executor = CommandExecutor::new();

    for i in 0..25 {
        executor.execute(&Command::Set {
            key: format!("key:{}", i),
            value: SDS::from_str("value"),
            ex: None,
            px: None,
            nx: false,
            xx: false,
            get: false,
        });
    }

    let cmd = Command::Scan {
        cursor: 0,
        pattern: None,
        count: Some(10),
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        // First element is cursor
        // Second element is array of keys
        if let RespValue::Array(Some(keys)) = &elements[1] {
            assert!(keys.len() <= 11); // count + 1 for cursor logic
        }
    } else {
        panic!("Expected array");
    }
}

#[test]
fn test_scan_with_pattern() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::Set {
        key: "user:1".to_string(),
        value: SDS::from_str("alice"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });
    executor.execute(&Command::Set {
        key: "user:2".to_string(),
        value: SDS::from_str("bob"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });
    executor.execute(&Command::Set {
        key: "session:1".to_string(),
        value: SDS::from_str("data"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });

    let cmd = Command::Scan {
        cursor: 0,
        pattern: Some("user:*".to_string()),
        count: Some(100),
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        if let RespValue::Array(Some(keys)) = &elements[1] {
            assert_eq!(keys.len(), 2);
        }
    } else {
        panic!("Expected array");
    }
}

// ============================================
// HSCAN Tests
// ============================================

#[test]
fn test_hscan_basic() {
    let mut executor = CommandExecutor::new();

    executor.execute(&Command::HSet(
        "myhash".to_string(),
        vec![
            (SDS::from_str("field1"), SDS::from_str("value1")),
            (SDS::from_str("field2"), SDS::from_str("value2")),
            (SDS::from_str("field3"), SDS::from_str("value3")),
        ],
    ));

    let cmd = Command::HScan {
        key: "myhash".to_string(),
        cursor: 0,
        pattern: None,
        count: Some(10),
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        if let RespValue::Array(Some(fields)) = &elements[1] {
            // Returns field/value pairs
            assert_eq!(fields.len(), 6); // 3 fields * 2
        }
    } else {
        panic!("Expected array");
    }
}

// ============================================
// ZSCAN Tests
// ============================================

#[test]
fn test_zscan_basic() {
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

    let cmd = Command::ZScan {
        key: "myzset".to_string(),
        cursor: 0,
        pattern: None,
        count: Some(10),
    };
    let result = executor.execute(&cmd);

    if let RespValue::Array(Some(elements)) = result {
        assert_eq!(elements.len(), 2);
        if let RespValue::Array(Some(members)) = &elements[1] {
            // Returns member/score pairs
            assert_eq!(members.len(), 6); // 3 members * 2
        }
    } else {
        panic!("Expected array");
    }
}
