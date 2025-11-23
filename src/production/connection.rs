use super::SharedRedisState;
use crate::redis::{Command, RespParser, RespValue};
use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{info, warn, error};

pub struct ConnectionHandler {
    stream: TcpStream,
    state: SharedRedisState,
    buffer: BytesMut,
    client_addr: String,
}

impl ConnectionHandler {
    pub fn new(stream: TcpStream, state: SharedRedisState, client_addr: String) -> Self {
        ConnectionHandler {
            stream,
            state,
            buffer: BytesMut::with_capacity(4096),
            client_addr,
        }
    }
    
    pub async fn run(mut self) {
        info!("Client connected: {}", self.client_addr);
        
        loop {
            // Read from socket
            let mut read_buf = vec![0u8; 4096];
            match self.stream.read(&mut read_buf).await {
                Ok(0) => {
                    info!("Client disconnected: {}", self.client_addr);
                    break;
                }
                Ok(n) => {
                    self.buffer.extend_from_slice(&read_buf[..n]);
                    
                    // Try to parse and execute commands
                    while let Some(response) = self.try_execute_command() {
                        if let Err(e) = self.stream.write_all(&response).await {
                            error!("Failed to write response: {}", e);
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("Error reading from client {}: {}", self.client_addr, e);
                    break;
                }
            }
        }
    }
    
    fn try_execute_command(&mut self) -> Option<Vec<u8>> {
        // Try to parse a RESP command from buffer
        match RespParser::parse(&self.buffer) {
            Ok((resp_value, bytes_consumed)) => {
                // Remove parsed bytes from buffer
                self.buffer.advance(bytes_consumed);
                
                // Parse command
                match Command::from_resp(&resp_value) {
                    Ok(cmd) => {
                        // Execute command with shared state
                        let response = self.state.with_lock(|executor| {
                            executor.execute(&cmd)
                        });
                        
                        // Encode response
                        Some(RespParser::encode(&response))
                    }
                    Err(e) => {
                        warn!("Invalid command from {}: {}", self.client_addr, e);
                        let error = RespValue::Error(format!("ERR {}", e));
                        Some(RespParser::encode(&error))
                    }
                }
            }
            Err(_) => {
                // Not enough data yet, wait for more
                None
            }
        }
    }
}

// Extension trait for BytesMut
trait BytesMutExt {
    fn advance(&mut self, cnt: usize);
}

impl BytesMutExt for BytesMut {
    fn advance(&mut self, cnt: usize) {
        let _ = self.split_to(cnt);
    }
}
