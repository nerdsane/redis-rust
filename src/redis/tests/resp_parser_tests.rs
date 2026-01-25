//! RESP parser tests - verify zero-copy parser equivalence with original

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
        (RespValue::Error(s1), RespValueZeroCopy::Error(s2)) => s1.as_bytes() == s2.as_ref(),
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
