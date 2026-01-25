use std::borrow::Cow;

/// RESP (Redis Serialization Protocol) values
///
/// Uses Cow<'static, str> for SimpleString and Error to enable zero-allocation
/// responses for static strings like "OK" and "PONG" while still supporting
/// dynamic strings.
#[derive(Debug, Clone, PartialEq)]
pub enum RespValue {
    SimpleString(Cow<'static, str>),
    Error(Cow<'static, str>),
    Integer(i64),
    BulkString(Option<Vec<u8>>),
    Array(Option<Vec<RespValue>>),
}

pub struct RespParser;

impl RespParser {
    pub fn parse(input: &[u8]) -> Result<(RespValue, usize), String> {
        if input.is_empty() {
            return Err("Empty input".to_string());
        }

        match input[0] {
            b'+' => Self::parse_simple_string(input),
            b'-' => Self::parse_error(input),
            b':' => Self::parse_integer(input),
            b'$' => Self::parse_bulk_string(input),
            b'*' => Self::parse_array(input),
            _ => Err(format!("Unknown RESP type: {}", input[0] as char)),
        }
    }

    fn parse_simple_string(input: &[u8]) -> Result<(RespValue, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let s = String::from_utf8_lossy(&input[1..pos]).into_owned();
            Ok((RespValue::SimpleString(Cow::Owned(s)), pos + 2))
        } else {
            Err("No CRLF found".to_string())
        }
    }

    fn parse_error(input: &[u8]) -> Result<(RespValue, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let s = String::from_utf8_lossy(&input[1..pos]).into_owned();
            Ok((RespValue::Error(Cow::Owned(s)), pos + 2))
        } else {
            Err("No CRLF found".to_string())
        }
    }

    fn parse_integer(input: &[u8]) -> Result<(RespValue, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let s = String::from_utf8_lossy(&input[1..pos]);
            let n = s.parse::<i64>().map_err(|e| e.to_string())?;
            Ok((RespValue::Integer(n), pos + 2))
        } else {
            Err("No CRLF found".to_string())
        }
    }

    fn parse_bulk_string(input: &[u8]) -> Result<(RespValue, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let len_str = String::from_utf8_lossy(&input[1..pos]);
            let len = len_str.parse::<i64>().map_err(|e| e.to_string())?;

            if len == -1 {
                return Ok((RespValue::BulkString(None), pos + 2));
            }

            let len = len as usize;
            let start = pos + 2;
            let end = start + len;

            if end + 2 > input.len() {
                return Err("Incomplete bulk string".to_string());
            }

            let data = input[start..end].to_vec();
            Ok((RespValue::BulkString(Some(data)), end + 2))
        } else {
            Err("No CRLF found".to_string())
        }
    }

    fn parse_array(input: &[u8]) -> Result<(RespValue, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let len_str = String::from_utf8_lossy(&input[1..pos]);
            let len = len_str.parse::<i64>().map_err(|e| e.to_string())?;

            if len == -1 {
                return Ok((RespValue::Array(None), pos + 2));
            }

            let mut elements = Vec::new();
            let mut offset = pos + 2;

            for _ in 0..len {
                let (value, consumed) = Self::parse(&input[offset..])?;
                elements.push(value);
                offset += consumed;
            }

            Ok((RespValue::Array(Some(elements)), offset))
        } else {
            Err("No CRLF found".to_string())
        }
    }

    fn find_crlf(input: &[u8]) -> Option<usize> {
        for i in 0..input.len().saturating_sub(1) {
            if input[i] == b'\r' && input[i + 1] == b'\n' {
                return Some(i);
            }
        }
        None
    }

    pub fn encode(value: &RespValue) -> Vec<u8> {
        match value {
            RespValue::SimpleString(s) => format!("+{}\r\n", s).into_bytes(),
            RespValue::Error(s) => format!("-{}\r\n", s).into_bytes(),
            RespValue::Integer(n) => format!(":{}\r\n", n).into_bytes(),
            RespValue::BulkString(None) => b"$-1\r\n".to_vec(),
            RespValue::BulkString(Some(data)) => {
                let mut result = format!("${}\r\n", data.len()).into_bytes();
                result.extend_from_slice(data);
                result.extend_from_slice(b"\r\n");
                result
            }
            RespValue::Array(None) => b"*-1\r\n".to_vec(),
            RespValue::Array(Some(elements)) => {
                let mut result = format!("*{}\r\n", elements.len()).into_bytes();
                for element in elements {
                    result.extend_from_slice(&Self::encode(element));
                }
                result
            }
        }
    }
}

// Static response helpers - zero allocation using Cow::Borrowed
impl RespValue {
    /// Static "OK" response - zero allocation
    #[inline]
    pub fn ok() -> Self {
        RespValue::SimpleString(Cow::Borrowed("OK"))
    }

    /// Static "PONG" response - zero allocation
    #[inline]
    pub fn pong() -> Self {
        RespValue::SimpleString(Cow::Borrowed("PONG"))
    }

    /// Static "QUEUED" response for transactions - zero allocation
    #[inline]
    pub fn queued() -> Self {
        RespValue::SimpleString(Cow::Borrowed("QUEUED"))
    }

    /// Static nil bulk string response
    #[inline]
    pub fn nil() -> Self {
        RespValue::BulkString(None)
    }

    /// Create a SimpleString from an owned String
    #[inline]
    pub fn simple_string(s: String) -> Self {
        RespValue::SimpleString(Cow::Owned(s))
    }

    /// Create an Error from any string type (String, &str, Cow)
    /// Uses Into trait for ergonomic API
    #[inline]
    pub fn err<S: Into<Cow<'static, str>>>(s: S) -> Self {
        RespValue::Error(s.into())
    }

    /// Create a SimpleString from any string type
    #[inline]
    pub fn simple<S: Into<Cow<'static, str>>>(s: S) -> Self {
        RespValue::SimpleString(s.into())
    }

    /// Static empty array response
    #[inline]
    pub fn empty_array() -> Self {
        RespValue::Array(Some(Vec::new()))
    }
}
