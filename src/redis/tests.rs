#[cfg(test)]
mod resp_parser_tests {
    use super::super::{RespCodec, RespParser, RespValue, RespValueZeroCopy};
    use bytes::BytesMut;

    fn test_parse_equivalence(input: &[u8]) {
        let old_result = RespParser::parse(input);
        let mut buf = BytesMut::from(input);
        let new_result = RespCodec::parse(&mut buf);

        match (old_result, new_result) {
            (Ok((old_val, _)), Ok(Some(new_val))) => {
                assert!(
                    values_equivalent(&old_val, &new_val),
                    "Parsed values differ for input: {:?}",
                    input
                );
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
            (RespValue::err(s1), RespValueZeroCopy::Error(s2)) => s1.as_bytes() == s2.as_ref(),
            (RespValue::Integer(n1), RespValueZeroCopy::Integer(n2)) => n1 == n2,
            (RespValue::BulkString(None), RespValueZeroCopy::BulkString(None)) => true,
            (RespValue::BulkString(Some(d1)), RespValueZeroCopy::BulkString(Some(d2))) => {
                d1.as_slice() == d2.as_ref()
            }
            (RespValue::Array(None), RespValueZeroCopy::Array(None)) => true,
            (RespValue::Array(Some(a1)), RespValueZeroCopy::Array(Some(a2))) => {
                a1.len() == a2.len()
                    && a1
                        .iter()
                        .zip(a2.iter())
                        .all(|(v1, v2)| values_equivalent(v1, v2))
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
        use super::super::data::SDS;
        use super::super::CommandExecutor;

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
        let old_resp = RespValue::Array(Some(vec![RespValue::BulkString(Some(b"PING".to_vec()))]));
        let new_resp = RespValueZeroCopy::Array(Some(vec![RespValueZeroCopy::BulkString(Some(
            Bytes::from_static(b"PING"),
        ))]));

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
            (
                Command::Set {
                    key: k1, value: v1, ..
                },
                Command::Set {
                    key: k2, value: v2, ..
                },
            ) => {
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
            RespValue::err(e) => assert!(e.contains("ERR")),
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
            RespValue::err(e) => assert!(e.contains("NOSCRIPT")),
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

    // ============================================
    // Lua tests for NEW commands from Delancie plan
    // ============================================

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
}

/// Unit tests for new commands (SET options, list operations, sorted sets, transactions, scan)
#[cfg(test)]
mod new_command_tests {
    use super::super::{Command, CommandExecutor, RespValue, SDS};

    // ============================================
    // SET with Options Tests
    // ============================================

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
        use crate::simulator::VirtualTime;

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
        use crate::simulator::VirtualTime;

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
}
