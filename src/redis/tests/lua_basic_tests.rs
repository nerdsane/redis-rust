//! Lua scripting basic tests - return types, KEYS/ARGV, sandbox
//!
//! These tests verify basic Lua functionality independent of redis.call

use super::super::data::SDS;
use super::super::{Command, CommandExecutor, RespValue};

#[test]
fn test_eval_simple_return() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return 42".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::Integer(42));
}

#[test]
fn test_eval_return_string() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return 'hello'".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::BulkString(Some(b"hello".to_vec())));
}

#[test]
fn test_eval_return_nil() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return nil".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::BulkString(None));
}

#[test]
fn test_eval_return_boolean_true() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return true".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    // Redis convention: true = 1
    assert_eq!(result, RespValue::Integer(1));
}

#[test]
fn test_eval_return_boolean_false() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return false".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    // Redis convention: false = nil
    assert_eq!(result, RespValue::BulkString(None));
}

#[test]
fn test_eval_return_array() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return {1, 2, 3}".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(
        result,
        RespValue::Array(Some(vec![
            RespValue::Integer(1),
            RespValue::Integer(2),
            RespValue::Integer(3),
        ]))
    );
}

#[test]
fn test_eval_keys_access() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return KEYS[1]".to_string(),
        keys: vec!["mykey".to_string()],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::BulkString(Some(b"mykey".to_vec())));
}

#[test]
fn test_eval_argv_access() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return ARGV[1]".to_string(),
        keys: vec![],
        args: vec![SDS::from_str("myarg")],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::BulkString(Some(b"myarg".to_vec())));
}

#[test]
fn test_eval_keys_length() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return #KEYS".to_string(),
        keys: vec!["key1".to_string(), "key2".to_string(), "key3".to_string()],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::Integer(3));
}

#[test]
fn test_eval_arithmetic() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return 10 + 5 * 2".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::Integer(20));
}

#[test]
fn test_eval_concatenation() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return KEYS[1] .. ':' .. ARGV[1]".to_string(),
        keys: vec!["prefix".to_string()],
        args: vec![SDS::from_str("suffix")],
    };
    let result = executor.execute(&cmd);
    assert_eq!(
        result,
        RespValue::BulkString(Some(b"prefix:suffix".to_vec()))
    );
}

#[test]
fn test_eval_script_error() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return undefined_variable".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    // Lua returns nil for undefined variables, not an error
    assert_eq!(result, RespValue::BulkString(None));
}

#[test]
fn test_eval_syntax_error() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "this is not valid lua!!!".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    match result {
        RespValue::Error(e) => assert!(e.contains("ERR")),
        _ => panic!("Expected error for syntax error"),
    }
}

#[test]
fn test_evalsha_noscript() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::EvalSha {
        sha1: "nonexistent_sha1".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    match result {
        RespValue::Error(e) => assert!(e.contains("NOSCRIPT")),
        _ => panic!("Expected NOSCRIPT error"),
    }
}

#[test]
fn test_evalsha_after_eval() {
    let mut executor = CommandExecutor::new();

    // First execute EVAL to cache the script
    let script = "return 42";
    let eval_cmd = Command::Eval {
        script: script.to_string(),
        keys: vec![],
        args: vec![],
    };
    let _ = executor.execute(&eval_cmd);

    // Compute SHA1
    let sha1 = crate::redis::lua::ScriptCache::compute_sha1(script);

    // Now EVALSHA should work
    let evalsha_cmd = Command::EvalSha {
        sha1,
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&evalsha_cmd);
    assert_eq!(result, RespValue::Integer(42));
}

#[test]
fn test_eval_sandbox_no_os() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return os".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    // os should be nil (sandboxed)
    assert_eq!(result, RespValue::BulkString(None));
}

#[test]
fn test_eval_sandbox_no_io() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "return io".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    // io should be nil (sandboxed)
    assert_eq!(result, RespValue::BulkString(None));
}

#[test]
fn test_eval_conditional() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "if ARGV[1] == 'yes' then return 1 else return 0 end".to_string(),
        keys: vec![],
        args: vec![SDS::from_str("yes")],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::Integer(1));

    let cmd2 = Command::Eval {
        script: "if ARGV[1] == 'yes' then return 1 else return 0 end".to_string(),
        keys: vec![],
        args: vec![SDS::from_str("no")],
    };
    let result2 = executor.execute(&cmd2);
    assert_eq!(result2, RespValue::Integer(0));
}

#[test]
fn test_eval_loop() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "local sum = 0; for i = 1, 10 do sum = sum + i end; return sum".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::Integer(55)); // 1+2+...+10 = 55
}

#[test]
fn test_eval_table_functions() {
    let mut executor = CommandExecutor::new();
    let cmd = Command::Eval {
        script: "local t = {'a', 'b', 'c'}; return #t".to_string(),
        keys: vec![],
        args: vec![],
    };
    let result = executor.execute(&cmd);
    assert_eq!(result, RespValue::Integer(3));
}
