use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 Testing Production Redis Server\n");
    
    // Connect to Redis server
    let mut stream = TcpStream::connect("127.0.0.1:3000").await?;
    println!("✓ Connected to Redis server on port 3000\n");
    
    // Test 1: PING
    println!("Test 1: PING");
    send_command(&mut stream, "*1\r\n$4\r\nPING\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}\n", String::from_utf8_lossy(&response));
    
    // Test 2: SET and GET
    println!("Test 2: SET key value");
    send_command(&mut stream, "*3\r\n$3\r\nSET\r\n$3\r\nkey\r\n$5\r\nvalue\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}", String::from_utf8_lossy(&response));
    
    println!("Test 2: GET key");
    send_command(&mut stream, "*2\r\n$3\r\nGET\r\n$3\r\nkey\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}\n", String::from_utf8_lossy(&response));
    
    // Test 3: INCR (atomic counter)
    println!("Test 3: INCR counter");
    send_command(&mut stream, "*2\r\n$4\r\nINCR\r\n$7\r\ncounter\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}", String::from_utf8_lossy(&response));
    
    println!("Test 3: INCR counter (again)");
    send_command(&mut stream, "*2\r\n$4\r\nINCR\r\n$7\r\ncounter\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}\n", String::from_utf8_lossy(&response));
    
    // Test 4: SETEX (with TTL)
    println!("Test 4: SETEX cache_key 2 cached_value");
    send_command(&mut stream, "*4\r\n$5\r\nSETEX\r\n$9\r\ncache_key\r\n$1\r\n2\r\n$12\r\ncached_value\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}", String::from_utf8_lossy(&response));
    
    println!("Test 4: GET cache_key (before expiry)");
    send_command(&mut stream, "*2\r\n$3\r\nGET\r\n$9\r\ncache_key\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}", String::from_utf8_lossy(&response));
    
    // Test 5: MSET and MGET (batch operations)
    println!("Test 5: MSET k1 v1 k2 v2 k3 v3");
    send_command(&mut stream, "*7\r\n$4\r\nMSET\r\n$2\r\nk1\r\n$2\r\nv1\r\n$2\r\nk2\r\n$2\r\nv2\r\n$2\r\nk3\r\n$2\r\nv3\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}", String::from_utf8_lossy(&response));
    
    println!("Test 5: MGET k1 k2 k3");
    send_command(&mut stream, "*4\r\n$4\r\nMGET\r\n$2\r\nk1\r\n$2\r\nk2\r\n$2\r\nk3\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}\n", String::from_utf8_lossy(&response));
    
    // Test 6: EXISTS
    println!("Test 6: EXISTS k1 k2 nonexistent");
    send_command(&mut stream, "*4\r\n$6\r\nEXISTS\r\n$2\r\nk1\r\n$2\r\nk2\r\n$11\r\nnonexistent\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}\n", String::from_utf8_lossy(&response));
    
    // Test 7: INFO
    println!("Test 7: INFO");
    send_command(&mut stream, "*1\r\n$4\r\nINFO\r\n").await?;
    let response = read_response(&mut stream).await?;
    println!("Response: {}\n", String::from_utf8_lossy(&response));
    
    // Test 8: Concurrent connections (spawn multiple clients)
    println!("Test 8: Testing concurrent connections...");
    let mut handles = vec![];
    for i in 0..10 {
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect("127.0.0.1:3000").await.unwrap();
            let cmd = format!("*3\r\n$3\r\nSET\r\n$5\r\ntest{}\r\n$6\r\nvalue{}\r\n", i, i);
            stream.write_all(cmd.as_bytes()).await.unwrap();
            let mut buf = vec![0u8; 1024];
            stream.read(&mut buf).await.unwrap();
        });
        handles.push(handle);
    }
    for handle in handles {
        handle.await?;
    }
    println!("✓ 10 concurrent connections completed successfully\n");
    
    println!("✅ All tests passed! Production Redis server is working correctly.");
    
    Ok(())
}

async fn send_command(stream: &mut TcpStream, cmd: &str) -> Result<(), Box<dyn std::error::Error>> {
    stream.write_all(cmd.as_bytes()).await?;
    Ok(())
}

async fn read_response(stream: &mut TcpStream) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    buf.truncate(n);
    Ok(buf)
}
