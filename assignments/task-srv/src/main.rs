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

/// Command line arguments parser for this application.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Keyword for the task
    #[arg(short, long)]
    keyword: String,

    /// IP address to bind to
    #[arg(short, long, default_value = "0.0.0.0")]
    ip: String,

    /// Port to bind to (must be between 1024 and 49151)
    #[arg(short, long)]
    port: u16,

    /// Agent server address to send control message to
    #[arg(short, long, default_value = "10.0.0.3:12345")]
    agent: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("Task-SRV starting");

    // Parse command line arguments
    let args = Args::parse();

    // Validate port range
    if args.port < 1024 || args.port > 49151 {
        return Err("Port must be between 1024 and 49151".into());
    }

    // Construct bind address
    let bind_addr = format!("{}:{}", args.ip, args.port);
    println!("Binding to {}", bind_addr);

    // Create a passive server socket and bind to address
    let server = TcpListener::bind(&bind_addr).await?;
    println!("Listening on {}", bind_addr);

    // Send control message to adnet-agent server
    let control_message = format!("TASK-SRV {} {}:{}", args.keyword, args.ip, args.port);
    println!("Connecting to agent server at {}...", args.agent);
    
    // Connect and send message with timeout
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

        // Spawn a new tokio task to handle communication with the client
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

async fn send_control_message(agent: &str, control_message: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut agent_socket = TcpStream::connect(agent).await?;
    agent_socket.write_all(control_message.as_bytes()).await?;
    Ok(())
}

// This function is started in spawned async task
async fn process_client(mut socket: TcpStream, address: SocketAddr) {
    loop {
        // Read 4 bytes for the transfer length (32-bit unsigned integer in network byte order)
        let mut length_bytes = [0u8; 4];
        if let Err(e) = socket.read_exact(&mut length_bytes).await {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                println!("Client {} closed connection", address);
            } else {
                println!("Error reading length from {}: {}", address, e);
            }
            return;
        }

        // Convert from network byte order (big-endian) to u32
        let total = u32::from_be_bytes(length_bytes);

        // Read 1 byte for the byte value to send
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

        // Write the requested number of bytes
        // Single write call might not be enough, so we write in a loop
        let mut written: u32 = 0;
        let buffer = [byte; 8192]; // Use a buffer to write multiple bytes at once

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