#[cfg(test)]
mod resp_parser_tests {
    use super::super::{RespParser, RespValue, RespCodec, RespValueZeroCopy};
    use bytes::BytesMut;

    fn test_parse_equivalence(input: &[u8]) {
        let old_result = RespParser::parse(input);
        let mut buf = BytesMut::from(input);
        let new_result = RespCodec::parse(&mut buf);

        match (old_result, new_result) {
            (Ok((old_val, _)), Ok(Some(new_val))) => {
                assert!(values_equivalent(&old_val, &new_val), 
                    "Parsed values differ for input: {:?}", input);
            }
            (Err(_), Ok(None)) | (Err(_), Err(_)) => {}
            (Ok(_), Ok(None)) => panic!("New parser incomplete where old succeeded"),
            (Ok(_), Err(e)) => panic!("New parser error where old succeeded: {}", e),
            (Err(e), Ok(Some(_))) => panic!("Old parser error where new succeeded: {}", e),
        }
    }

    fn values_equivalent(old: &RespValue, new: &RespValueZeroCopy) -> bool {
        match (old, new) {
            (RespValue::SimpleString(s1), RespValueZeroCopy::SimpleString(s2)) => {
                s1.as_bytes() == s2.as_ref()
            }
            (RespValue::Error(s1), RespValueZeroCopy::Error(s2)) => {
                s1.as_bytes() == s2.as_ref()
            }
            (RespValue::Integer(n1), RespValueZeroCopy::Integer(n2)) => n1 == n2,
            (RespValue::BulkString(None), RespValueZeroCopy::BulkString(None)) => true,
            (RespValue::BulkString(Some(d1)), RespValueZeroCopy::BulkString(Some(d2))) => {
                d1.as_slice() == d2.as_ref()
            }
            (RespValue::Array(None), RespValueZeroCopy::Array(None)) => true,
            (RespValue::Array(Some(a1)), RespValueZeroCopy::Array(Some(a2))) => {
                a1.len() == a2.len() && 
                a1.iter().zip(a2.iter()).all(|(v1, v2)| values_equivalent(v1, v2))
            }
            _ => false,
        }
    }

    #[test]
    fn test_simple_string() {
        test_parse_equivalence(b"+OK\r\n");
        test_parse_equivalence(b"+PONG\r\n");
    }

    #[test]
    fn test_error() {
        test_parse_equivalence(b"-ERR unknown command\r\n");
    }

    #[test]
    fn test_integer() {
        test_parse_equivalence(b":0\r\n");
        test_parse_equivalence(b":1000\r\n");
        test_parse_equivalence(b":-1\r\n");
    }

    #[test]
    fn test_bulk_string() {
        test_parse_equivalence(b"$5\r\nhello\r\n");
        test_parse_equivalence(b"$0\r\n\r\n");
        test_parse_equivalence(b"$-1\r\n");
    }

    #[test]
    fn test_array() {
        test_parse_equivalence(b"*2\r\n$3\r\nGET\r\n$3\r\nfoo\r\n");
        test_parse_equivalence(b"*3\r\n$3\r\nSET\r\n$3\r\nkey\r\n$5\r\nvalue\r\n");
        test_parse_equivalence(b"*0\r\n");
        test_parse_equivalence(b"*-1\r\n");
    }

    #[test]
    fn test_ping_command() {
        test_parse_equivalence(b"*1\r\n$4\r\nPING\r\n");
    }

    #[test]
    fn test_set_get_commands() {
        test_parse_equivalence(b"*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$7\r\nmyvalue\r\n");
        test_parse_equivalence(b"*2\r\n$3\r\nGET\r\n$5\r\nmykey\r\n");
    }
}

#[cfg(test)]
mod command_parser_tests {
    use super::super::{Command, RespValue, RespValueZeroCopy};
    use bytes::Bytes;

    #[test]
    fn test_zrevrange_without_scores() {
        let old_resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"ZREVRANGE".to_vec())),
            RespValue::BulkString(Some(b"myzset".to_vec())),
            RespValue::BulkString(Some(b"0".to_vec())),
            RespValue::BulkString(Some(b"-1".to_vec())),
        ]));
        let new_resp = RespValueZeroCopy::Array(Some(vec![
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"ZREVRANGE"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"myzset"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"0"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"-1"))),
        ]));

        let old_cmd = Command::from_resp(&old_resp).unwrap();
        let new_cmd = Command::from_resp_zero_copy(&new_resp).unwrap();

        match (old_cmd, new_cmd) {
            (Command::ZRevRange(k1, s1, e1, ws1), Command::ZRevRange(k2, s2, e2, ws2)) => {
                assert_eq!(k1, k2);
                assert_eq!(k1, "myzset");
                assert_eq!(s1, s2);
                assert_eq!(s1, 0);
                assert_eq!(e1, e2);
                assert_eq!(e1, -1);
                assert!(!ws1);
                assert!(!ws2);
            }
            _ => panic!("Commands don't match"),
        }
    }

    #[test]
    fn test_zrevrange_with_scores() {
        let old_resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"ZREVRANGE".to_vec())),
            RespValue::BulkString(Some(b"leaderboard".to_vec())),
            RespValue::BulkString(Some(b"0".to_vec())),
            RespValue::BulkString(Some(b"9".to_vec())),
            RespValue::BulkString(Some(b"WITHSCORES".to_vec())),
        ]));
        let new_resp = RespValueZeroCopy::Array(Some(vec![
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"ZREVRANGE"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"leaderboard"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"0"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"9"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"WITHSCORES"))),
        ]));

        let old_cmd = Command::from_resp(&old_resp).unwrap();
        let new_cmd = Command::from_resp_zero_copy(&new_resp).unwrap();

        match (old_cmd, new_cmd) {
            (Command::ZRevRange(k1, s1, e1, ws1), Command::ZRevRange(k2, s2, e2, ws2)) => {
                assert_eq!(k1, k2);
                assert_eq!(k1, "leaderboard");
                assert_eq!(s1, 0);
                assert_eq!(e1, 9);
                assert!(ws1);
                assert!(ws2);
            }
            _ => panic!("Commands don't match"),
        }
    }

    #[test]
    fn test_zrevrange_case_insensitive_withscores() {
        // Test lowercase withscores
        let resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"ZREVRANGE".to_vec())),
            RespValue::BulkString(Some(b"key".to_vec())),
            RespValue::BulkString(Some(b"0".to_vec())),
            RespValue::BulkString(Some(b"-1".to_vec())),
            RespValue::BulkString(Some(b"withscores".to_vec())),
        ]));

        let cmd = Command::from_resp(&resp).unwrap();
        match cmd {
            Command::ZRevRange(_, _, _, with_scores) => {
                assert!(with_scores);
            }
            _ => panic!("Expected ZRevRange"),
        }
    }

    #[test]
    fn test_zrevrange_negative_indices() {
        let resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"ZREVRANGE".to_vec())),
            RespValue::BulkString(Some(b"mykey".to_vec())),
            RespValue::BulkString(Some(b"-3".to_vec())),
            RespValue::BulkString(Some(b"-1".to_vec())),
        ]));

        let cmd = Command::from_resp(&resp).unwrap();
        match cmd {
            Command::ZRevRange(key, start, stop, _) => {
                assert_eq!(key, "mykey");
                assert_eq!(start, -3);
                assert_eq!(stop, -1);
            }
            _ => panic!("Expected ZRevRange"),
        }
    }

    #[test]
    fn test_hset_multi_field() {
        let resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"HSET".to_vec())),
            RespValue::BulkString(Some(b"myhash".to_vec())),
            RespValue::BulkString(Some(b"field1".to_vec())),
            RespValue::BulkString(Some(b"value1".to_vec())),
            RespValue::BulkString(Some(b"field2".to_vec())),
            RespValue::BulkString(Some(b"value2".to_vec())),
        ]));

        let cmd = Command::from_resp(&resp).unwrap();
        match cmd {
            Command::HSet(key, pairs) => {
                assert_eq!(key, "myhash");
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0.to_string(), "field1");
                assert_eq!(pairs[0].1.to_string(), "value1");
                assert_eq!(pairs[1].0.to_string(), "field2");
                assert_eq!(pairs[1].1.to_string(), "value2");
            }
            _ => panic!("Expected HSet"),
        }
    }

    #[test]
    fn test_hincrby_parsing() {
        let resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"HINCRBY".to_vec())),
            RespValue::BulkString(Some(b"myhash".to_vec())),
            RespValue::BulkString(Some(b"field1".to_vec())),
            RespValue::BulkString(Some(b"5".to_vec())),
        ]));

        let cmd = Command::from_resp(&resp).unwrap();
        match cmd {
            Command::HIncrBy(key, field, increment) => {
                assert_eq!(key, "myhash");
                assert_eq!(field.to_string(), "field1");
                assert_eq!(increment, 5);
            }
            _ => panic!("Expected HIncrBy"),
        }
    }

    #[test]
    fn test_hincrby_negative() {
        let resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"HINCRBY".to_vec())),
            RespValue::BulkString(Some(b"counter".to_vec())),
            RespValue::BulkString(Some(b"hits".to_vec())),
            RespValue::BulkString(Some(b"-3".to_vec())),
        ]));

        let cmd = Command::from_resp(&resp).unwrap();
        match cmd {
            Command::HIncrBy(key, field, increment) => {
                assert_eq!(key, "counter");
                assert_eq!(field.to_string(), "hits");
                assert_eq!(increment, -3);
            }
            _ => panic!("Expected HIncrBy"),
        }
    }

    #[test]
    fn test_hincrby_execution() {
        use super::super::CommandExecutor;
        use super::super::data::SDS;

        let mut executor = CommandExecutor::new();

        // First HINCRBY creates the field with the increment value
        let cmd = Command::HIncrBy("myhash".to_string(), SDS::from_str("counter"), 5);
        let result = executor.execute(&cmd);
        assert_eq!(result, RespValue::Integer(5));

        // Second HINCRBY increments existing value
        let cmd = Command::HIncrBy("myhash".to_string(), SDS::from_str("counter"), 3);
        let result = executor.execute(&cmd);
        assert_eq!(result, RespValue::Integer(8));

        // Negative increment (decrement)
        let cmd = Command::HIncrBy("myhash".to_string(), SDS::from_str("counter"), -2);
        let result = executor.execute(&cmd);
        assert_eq!(result, RespValue::Integer(6));
    }

    #[test]
    fn test_ping_from_both_parsers() {
        let old_resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"PING".to_vec()))
        ]));
        let new_resp = RespValueZeroCopy::Array(Some(vec![
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"PING")))
        ]));

        let old_cmd = Command::from_resp(&old_resp).unwrap();
        let new_cmd = Command::from_resp_zero_copy(&new_resp).unwrap();

        assert!(matches!(old_cmd, Command::Ping));
        assert!(matches!(new_cmd, Command::Ping));
    }

    #[test]
    fn test_set_from_both_parsers() {
        let old_resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"SET".to_vec())),
            RespValue::BulkString(Some(b"key".to_vec())),
            RespValue::BulkString(Some(b"value".to_vec())),
        ]));
        let new_resp = RespValueZeroCopy::Array(Some(vec![
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"SET"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"key"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"value"))),
        ]));

        let old_cmd = Command::from_resp(&old_resp).unwrap();
        let new_cmd = Command::from_resp_zero_copy(&new_resp).unwrap();

        match (old_cmd, new_cmd) {
            (Command::Set { key: k1, value: v1, .. }, Command::Set { key: k2, value: v2, .. }) => {
                assert_eq!(k1, k2);
                assert_eq!(v1.as_bytes(), v2.as_bytes());
            }
            _ => panic!("Commands don't match"),
        }
    }

    #[test]
    fn test_get_from_both_parsers() {
        let old_resp = RespValue::Array(Some(vec![
            RespValue::BulkString(Some(b"GET".to_vec())),
            RespValue::BulkString(Some(b"mykey".to_vec())),
        ]));
        let new_resp = RespValueZeroCopy::Array(Some(vec![
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"GET"))),
            RespValueZeroCopy::BulkString(Some(Bytes::from_static(b"mykey"))),
        ]));

        let old_cmd = Command::from_resp(&old_resp).unwrap();
        let new_cmd = Command::from_resp_zero_copy(&new_resp).unwrap();

        match (old_cmd, new_cmd) {
            (Command::Get(k1), Command::Get(k2)) => {
                assert_eq!(k1, k2);
            }
            _ => panic!("Commands don't match"),
        }
    }
}

#[cfg(test)]
#[cfg(feature = "lua")]
mod lua_scripting_tests {
    use super::super::{Command, CommandExecutor, RespValue};
    use super::super::data::SDS;

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
        assert_eq!(result, RespValue::BulkString(Some(b"prefix:suffix".to_vec())));
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
        let sha1 = super::super::lua::ScriptCache::compute_sha1(script);

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

    // === Critical DST and Semantic Fix Tests ===

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
            "#.to_string(),
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
            "#.to_string(),
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
            "#.to_string(),
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
            "#.to_string(),
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
        use crate::simulator::VirtualTime;

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
            "#.to_string(),
            keys: vec!["mykey".to_string()],
            args: vec![SDS::from_str("myvalue")],
        };
        let result = executor.execute(&cmd);

        assert_eq!(
            result,
            RespValue::BulkString(Some(b"myvalue".to_vec()))
        );
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
            "#.to_string(),
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
}
