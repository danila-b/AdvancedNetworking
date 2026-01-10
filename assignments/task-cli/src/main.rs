/*  You may start from this template when implementing Task 1,
    or use entirely own code.
 */

use std::{
    error::Error,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

const AGENT_SERVER: &str = "10.0.0.1:12345";
const KEYWORD: &[u8] = b"TASK-001 demo";

// Timeouts prevent the program from hanging forever if the server is unresponsive
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

fn main() -> Result<(), Box<dyn Error>> {
    println!("Task-CLI starting");

    // Start clock to measure the time it takes to finish transmission
    let start = Instant::now();

    // Converts Agent server address into an actual socket address.
    // We do this separately so we can use connect_timeout() which needs a SocketAddr.
    let addr = AGENT_SERVER
        .to_socket_addrs()?              
        .next()                          
        .ok_or("Failed to resolve server address")?;

    let mut socket = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("Connection to {} failed: {}", AGENT_SERVER, e))?;

    println!("Connected to {}", AGENT_SERVER);

    socket.set_read_timeout(Some(READ_TIMEOUT))?;
    socket.write_all(KEYWORD)?;


    let mut all_data = Vec::new();
    socket.read_to_end(&mut all_data)?;

    let duration = start.elapsed();
    let total_size = all_data.len();

    // Get the last 8 bytes or all of it if we received less data
    let last_bytes: String = if total_size >= 8 {
        String::from_utf8_lossy(&all_data[total_size - 8..]).to_string()
    } else {
        String::from_utf8_lossy(&all_data).to_string()
    };

    println!(
        "Total size: {} bytes -- Last 8 bytes: {:?} -- Duration: {:.2?}",
        total_size, last_bytes, duration
    );

    Ok(())
}