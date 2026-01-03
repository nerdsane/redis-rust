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
            (Command::Set(k1, v1), Command::Set(k2, v2)) => {
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
