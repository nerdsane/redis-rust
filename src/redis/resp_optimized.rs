use bytes::{Bytes, BytesMut, Buf};

#[derive(Debug, Clone, PartialEq)]
pub enum RespValueZeroCopy {
    SimpleString(Bytes),
    Error(Bytes),
    Integer(i64),
    BulkString(Option<Bytes>),
    Array(Option<Vec<RespValueZeroCopy>>),
}

pub struct RespCodec;

impl RespCodec {
    pub fn parse(input: &mut BytesMut) -> Result<Option<RespValueZeroCopy>, String> {
        if input.is_empty() {
            return Ok(None);
        }

        match Self::try_parse(input) {
            Ok((value, consumed)) => {
                input.advance(consumed);
                Ok(Some(value))
            }
            Err(e) if e == "Incomplete" => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn try_parse(input: &[u8]) -> Result<(RespValueZeroCopy, usize), String> {
        if input.is_empty() {
            return Err("Incomplete".to_string());
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

    fn parse_simple_string(input: &[u8]) -> Result<(RespValueZeroCopy, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let data = Bytes::copy_from_slice(&input[1..pos]);
            Ok((RespValueZeroCopy::SimpleString(data), pos + 2))
        } else {
            Err("Incomplete".to_string())
        }
    }

    fn parse_error(input: &[u8]) -> Result<(RespValueZeroCopy, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let data = Bytes::copy_from_slice(&input[1..pos]);
            Ok((RespValueZeroCopy::Error(data), pos + 2))
        } else {
            Err("Incomplete".to_string())
        }
    }

    fn parse_integer(input: &[u8]) -> Result<(RespValueZeroCopy, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let s = std::str::from_utf8(&input[1..pos]).map_err(|e| e.to_string())?;
            let n = s.parse::<i64>().map_err(|e| e.to_string())?;
            Ok((RespValueZeroCopy::Integer(n), pos + 2))
        } else {
            Err("Incomplete".to_string())
        }
    }

    fn parse_bulk_string(input: &[u8]) -> Result<(RespValueZeroCopy, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let len_str = std::str::from_utf8(&input[1..pos]).map_err(|e| e.to_string())?;
            let len = len_str.parse::<i64>().map_err(|e| e.to_string())?;

            if len == -1 {
                return Ok((RespValueZeroCopy::BulkString(None), pos + 2));
            }

            let len = len as usize;
            let start = pos + 2;
            let end = start + len;

            if end + 2 > input.len() {
                return Err("Incomplete".to_string());
            }

            let data = Bytes::copy_from_slice(&input[start..end]);
            Ok((RespValueZeroCopy::BulkString(Some(data)), end + 2))
        } else {
            Err("Incomplete".to_string())
        }
    }

    fn parse_array(input: &[u8]) -> Result<(RespValueZeroCopy, usize), String> {
        if let Some(pos) = Self::find_crlf(input) {
            let len_str = std::str::from_utf8(&input[1..pos]).map_err(|e| e.to_string())?;
            let len = len_str.parse::<i64>().map_err(|e| e.to_string())?;

            if len == -1 {
                return Ok((RespValueZeroCopy::Array(None), pos + 2));
            }

            let mut elements = Vec::with_capacity(len as usize);
            let mut offset = pos + 2;

            for _ in 0..len {
                if offset >= input.len() {
                    return Err("Incomplete".to_string());
                }
                let (value, consumed) = Self::try_parse(&input[offset..])?;
                elements.push(value);
                offset += consumed;
            }

            Ok((RespValueZeroCopy::Array(Some(elements)), offset))
        } else {
            Err("Incomplete".to_string())
        }
    }

    #[inline]
    fn find_crlf(input: &[u8]) -> Option<usize> {
        memchr::memchr(b'\r', input).and_then(|pos| {
            if pos + 1 < input.len() && input[pos + 1] == b'\n' {
                Some(pos)
            } else {
                None
            }
        })
    }

    pub fn encode(value: &RespValueZeroCopy) -> BytesMut {
        let mut buf = BytesMut::with_capacity(256);
        Self::encode_into(value, &mut buf);
        buf
    }

    fn encode_into(value: &RespValueZeroCopy, buf: &mut BytesMut) {
        use bytes::BufMut;
        match value {
            RespValueZeroCopy::SimpleString(s) => {
                buf.put_u8(b'+');
                buf.extend_from_slice(s);
                buf.extend_from_slice(b"\r\n");
            }
            RespValueZeroCopy::Error(s) => {
                buf.put_u8(b'-');
                buf.extend_from_slice(s);
                buf.extend_from_slice(b"\r\n");
            }
            RespValueZeroCopy::Integer(n) => {
                buf.put_u8(b':');
                buf.extend_from_slice(n.to_string().as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            RespValueZeroCopy::BulkString(None) => {
                buf.extend_from_slice(b"$-1\r\n");
            }
            RespValueZeroCopy::BulkString(Some(data)) => {
                buf.put_u8(b'$');
                buf.extend_from_slice(data.len().to_string().as_bytes());
                buf.extend_from_slice(b"\r\n");
                buf.extend_from_slice(data);
                buf.extend_from_slice(b"\r\n");
            }
            RespValueZeroCopy::Array(None) => {
                buf.extend_from_slice(b"*-1\r\n");
            }
            RespValueZeroCopy::Array(Some(elements)) => {
                buf.put_u8(b'*');
                buf.extend_from_slice(elements.len().to_string().as_bytes());
                buf.extend_from_slice(b"\r\n");
                for elem in elements {
                    Self::encode_into(elem, buf);
                }
            }
        }
    }
}

pub struct BufferPool {
    pool: crossbeam::queue::ArrayQueue<BytesMut>,
    capacity: usize,
}

impl BufferPool {
    pub fn new(size: usize, buffer_capacity: usize) -> Self {
        let pool = crossbeam::queue::ArrayQueue::new(size);
        for _ in 0..size {
            let _ = pool.push(BytesMut::with_capacity(buffer_capacity));
        }
        BufferPool { pool, capacity: buffer_capacity }
    }

    pub fn acquire(&self) -> BytesMut {
        self.pool.pop().unwrap_or_else(|| BytesMut::with_capacity(self.capacity))
    }

    pub fn release(&self, mut buf: BytesMut) {
        buf.clear();
        let _ = self.pool.push(buf);
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(256, 4096)
    }
}
