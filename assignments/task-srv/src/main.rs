/*  TCP server that handles incoming connections and sends requested bytes.
    Each client is handled in a separate async task using Tokio.
 */

use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task,
    time,
};

// Timeout for connecting to and sending message to adnet-agent server
const AGENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
// Timeout for handling each client connection
const CLIENT_HANDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    keyword: String,

    #[arg(short, long, default_value = "0.0.0.0")]
    ip: String,

    #[arg(short, long)]
    port: u16,

    #[arg(short, long, default_value = "10.0.0.3:12345")]
    agent: String,
}

/// Main entry point for the TCP server.
///
/// Parses arguments, binds to the specified address, sends a control message to the agent server,
/// and then listens for incoming client connections pretty much indefinitely.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("Task-SRV starting");

    let args = Args::parse();

    // Validate port range
    if args.port < 1024 || args.port > 49151 {
        return Err("Port must be between 1024 and 49151".into());
    }

    let bind_addr = format!("{}:{}", args.ip, args.port);
    println!("Binding to {}", bind_addr);

    let server = TcpListener::bind(&bind_addr).await?;
    println!("Listening on {}", bind_addr);

    // Send control message to adnet-agent server
    let control_message = format!("TASK-SRV {} {}:{}", args.keyword, args.ip, args.port);
    println!("Connecting to agent server at {}...", args.agent);
    
    let control_message_result = time::timeout(AGENT_CONNECT_TIMEOUT, send_control_message(&args.agent, &control_message)).await;
    match control_message_result {
        Ok(Ok(_)) => {
            println!("Sent control message: {}", control_message);
        }
        Ok(Err(e)) => {
            return Err(format!("Failed to connect or send message to agent server at {}: {}", args.agent, e).into());
        }
        Err(_) => {
            return Err(format!("Timeout connecting to agent server at {} within {:?}", args.agent, AGENT_CONNECT_TIMEOUT).into());
        }
    }

    loop {
        // Wait until new connection request comes in
        let (socket, address) = server.accept().await?;
        println!("Accepting connection from {}", address);

        // Spawn a new tokio task to handle communication with the client with a timeout.
        task::spawn(async move {
            match time::timeout(CLIENT_HANDLE_TIMEOUT, process_client(socket, address)).await {
                Ok(_) => {
                    // Client handling completed normally
                }
                Err(_) => {
                    println!("Client {} connection timed out after {:?}", address, CLIENT_HANDLE_TIMEOUT);
                }
            }
        });
    }
}


/// Connects to the agent server and sends the control message.
async fn send_control_message(agent: &str, control_message: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut agent_socket = TcpStream::connect(agent).await?;
    agent_socket.write_all(control_message.as_bytes()).await?;
    Ok(())
}

/// Handles communication with a single client connection.
///
/// Reads 5-byte requests (4 bytes for length, 1 byte for value) and sends the requested number of bytes. 
/// Continues until the client closes the connection (so can hang unless the timeout is set on the caller side).
async fn process_client(mut socket: TcpStream, address: SocketAddr) {
    loop {
        let mut length_bytes = [0u8; 4];
        if let Err(e) = socket.read_exact(&mut length_bytes).await {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                println!("Client {} closed connection", address);
            } else {
                println!("Error reading length from {}: {}", address, e);
            }
            return;
        }

        let total = u32::from_be_bytes(length_bytes);

        let mut byte_value = [0u8; 1];
        if let Err(e) = socket.read_exact(&mut byte_value).await {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                println!("Client {} closed connection", address);
            } else {
                println!("Error reading byte value from {}: {}", address, e);
            }
            return;
        }

        let byte = byte_value[0];

        let mut written: u32 = 0;
        let buffer = [byte; 8192];

        while written < total {
            let remaining = total - written;
            let to_write = remaining.min(buffer.len() as u32) as usize;

            if let Err(e) = socket.write_all(&buffer[..to_write]).await {
                println!("Error writing to client {}: {}", address, e);
                return;
            }

            written += to_write as u32;
        }

        println!("Wrote {} bytes of byte {}", written, byte);
    }
}