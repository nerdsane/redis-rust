//! SET command option tests - NX, XX, GET, EX, PX options

use super::super::{Command, CommandExecutor, RespValue, SDS};
use crate::simulator::VirtualTime;

#[test]
fn test_set_nx_when_key_not_exists() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("myvalue"),
        ex: None,
        px: None,
        nx: true,
        xx: false,
        get: false,
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::simple("OK"));

    // Verify the value was set
    let get_cmd = Command::Get("mykey".to_string());
    assert_eq!(
        executor.execute(&get_cmd),
        RespValue::BulkString(Some(b"myvalue".to_vec()))
    );
}

#[test]
fn test_set_nx_when_key_exists() {
    let mut executor = CommandExecutor::new();

    // First set the key
    executor.execute(&Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("original"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });

    // NX should fail when key exists
    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("newvalue"),
        ex: None,
        px: None,
        nx: true,
        xx: false,
        get: false,
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::BulkString(None));

    // Value should be unchanged
    let get_cmd = Command::Get("mykey".to_string());
    assert_eq!(
        executor.execute(&get_cmd),
        RespValue::BulkString(Some(b"original".to_vec()))
    );
}

#[test]
fn test_set_xx_when_key_exists() {
    let mut executor = CommandExecutor::new();

    // First set the key
    executor.execute(&Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("original"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });

    // XX should succeed when key exists
    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("newvalue"),
        ex: None,
        px: None,
        nx: false,
        xx: true,
        get: false,
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::simple("OK"));

    // Value should be updated
    let get_cmd = Command::Get("mykey".to_string());
    assert_eq!(
        executor.execute(&get_cmd),
        RespValue::BulkString(Some(b"newvalue".to_vec()))
    );
}

#[test]
fn test_set_xx_when_key_not_exists() {
    let mut executor = CommandExecutor::new();

    // XX should fail when key doesn't exist
    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("myvalue"),
        ex: None,
        px: None,
        nx: false,
        xx: true,
        get: false,
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::BulkString(None));

    // Key should not exist
    let get_cmd = Command::Get("mykey".to_string());
    assert_eq!(executor.execute(&get_cmd), RespValue::BulkString(None));
}

#[test]
fn test_set_get_returns_old_value() {
    let mut executor = CommandExecutor::new();

    // First set the key
    executor.execute(&Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("original"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: false,
    });

    // SET with GET should return old value
    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("newvalue"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: true,
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::BulkString(Some(b"original".to_vec())));

    // Value should be updated
    let get_cmd = Command::Get("mykey".to_string());
    assert_eq!(
        executor.execute(&get_cmd),
        RespValue::BulkString(Some(b"newvalue".to_vec()))
    );
}

#[test]
fn test_set_get_returns_nil_when_key_not_exists() {
    let mut executor = CommandExecutor::new();

    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("myvalue"),
        ex: None,
        px: None,
        nx: false,
        xx: false,
        get: true,
    };
    let result = executor.execute(&cmd);

    assert_eq!(result, RespValue::BulkString(None));
}

#[test]
fn test_set_ex_sets_expiration() {
    let mut executor = CommandExecutor::new();
    executor.set_time(VirtualTime::from_millis(0));

    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("myvalue"),
        ex: Some(10), // 10 seconds
        px: None,
        nx: false,
        xx: false,
        get: false,
    };
    executor.execute(&cmd);

    // Key should exist immediately
    let get_cmd = Command::Get("mykey".to_string());
    assert_eq!(
        executor.execute(&get_cmd),
        RespValue::BulkString(Some(b"myvalue".to_vec()))
    );

    // Advance time past expiration
    executor.set_time(VirtualTime::from_millis(11000));

    // Key should be expired
    assert_eq!(executor.execute(&get_cmd), RespValue::BulkString(None));
}

#[test]
fn test_set_px_sets_expiration_milliseconds() {
    let mut executor = CommandExecutor::new();
    executor.set_time(VirtualTime::from_millis(0));

    let cmd = Command::Set {
        key: "mykey".to_string(),
        value: SDS::from_str("myvalue"),
        ex: None,
        px: Some(500), // 500 milliseconds
        nx: false,
        xx: false,
        get: false,
    };
    executor.execute(&cmd);

    // Key should exist at 400ms
    executor.set_time(VirtualTime::from_millis(400));
    let get_cmd = Command::Get("mykey".to_string());
    assert_eq!(
        executor.execute(&get_cmd),
        RespValue::BulkString(Some(b"myvalue".to_vec()))
    );

    // Key should be expired at 600ms
    executor.set_time(VirtualTime::from_millis(600));
    assert_eq!(executor.execute(&get_cmd), RespValue::BulkString(None));
}
