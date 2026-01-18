//! UDP Data Transfer Client
//!
//! A straightforward implementation using a small fixed window with
//! single-packet retransmission on timeout.

use clap::Parser;
use std::{
    collections::HashMap,
    error::Error,
    io::{Read, Write},
    net::{SocketAddr, TcpStream, UdpSocket},
    time::{Duration, Instant},
};

const MAX_PAYLOAD: usize = 1200;
const HEADER_SIZE: usize = 6; // 4 bytes seq + 2 bytes payload len
const INITIAL_RTO_MS: u64 = 1000;
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(180);
const SOCKET_READ_TIMEOUT: Duration = Duration::from_millis(500);
const WINDOW_SIZE: usize = 3;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    server: String,

    #[arg(short, long)]
    keyword: String,
}

struct PacketInfo {
    packet: Vec<u8>,
    sent_time: Instant,
    retry_count: u32,
}

struct TransmissionState {
    transmitted: usize,
    next_seq: u32,
    checknum: u8,
    unacked_packets: HashMap<u32, PacketInfo>,
    last_acked_seq: u32,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    println!("Task-UDP starting");
    println!("Connecting to server: {}", args.server);
    println!("Using keyword: {}", args.keyword);

    let start = Instant::now();
    let mut tcp_stream = TcpStream::connect(&args.server)?;

    let command = format!("TASK-UDP {}\n", args.keyword);
    tcp_stream.write_all(command.as_bytes())?;

    let mut buf = [0u8; 256];
    let n = tcp_stream.read(&mut buf)?;
    let resp: Vec<&str> = std::str::from_utf8(&buf[..n])?.split_whitespace().collect();

    if resp.len() < 2 {
        return Err("Invalid server response format".into());
    }

    let size: usize = resp[0].parse()?;
    let character = resp[1];

    if character.len() != 1 {
        return Err("Invalid character in server response".into());
    }

    let char_byte = character.as_bytes()[0];
    println!("Starting to transmit {} bytes of '{}'.", size, character);

    let tcp_addr = tcp_stream.peer_addr()?;
    let udp_address = format!("{}:20000", tcp_addr.ip());

    let checknum = transmit_loop(&udp_address, size, char_byte)?;
    let duration = start.elapsed();

    println!(
        "Size: {} -- Checknum: {} -- Duration: {:?}",
        size, checknum, duration
    );

    Ok(())
}

fn create_packet(seq: u32, payload_size: usize, character: u8) -> Vec<u8> {
    let mut packet = Vec::with_capacity(HEADER_SIZE + payload_size);
    packet.extend_from_slice(&seq.to_be_bytes());
    packet.extend_from_slice(&(payload_size as u16).to_be_bytes());
    packet.extend(vec![character; payload_size]);
    packet
}

fn calculate_rto(retry_count: u32) -> Duration {
    let base_ms = INITIAL_RTO_MS as f64;
    let multiplier = if retry_count == 0 {
        1.0
    } else {
        (retry_count as f64 + 1.0).ln() / 2.0_f64.ln()
    };
    let timeout_ms = (base_ms * multiplier).min(30000.0);
    Duration::from_millis(timeout_ms as u64)
}

fn send_new_packets(
    socket: &UdpSocket,
    server_addr: SocketAddr,
    state: &mut TransmissionState,
    size: usize,
    character: u8,
) -> Result<(), Box<dyn Error>> {
    while state.transmitted < size && state.unacked_packets.len() < WINDOW_SIZE {
        let remaining = size - state.transmitted;
        let payload_size = remaining.min(MAX_PAYLOAD);

        let packet = create_packet(state.next_seq, payload_size, character);
        socket.send_to(&packet, server_addr)?;

        state.unacked_packets.insert(
            state.next_seq,
            PacketInfo {
                packet,
                sent_time: Instant::now(),
                retry_count: 0,
            },
        );

        state.transmitted += payload_size;
        state.next_seq += 1;
    }
    Ok(())
}

fn handle_ack(state: &mut TransmissionState, acked_seq: u32, checknum: u8) {
    state.checknum = checknum;

    if acked_seq > state.last_acked_seq {
        for seq in (state.last_acked_seq + 1)..=acked_seq {
            state.unacked_packets.remove(&seq);
        }
        state.last_acked_seq = acked_seq;
        println!(
            "ACK received: seq {}, unacked: {}",
            acked_seq,
            state.unacked_packets.len()
        );
    }
}

fn retransmit_next_expected(
    socket: &UdpSocket,
    server_addr: SocketAddr,
    state: &mut TransmissionState,
) -> Result<(), Box<dyn Error>> {
    let next_expected = state.last_acked_seq + 1;

    if let Some(info) = state.unacked_packets.get_mut(&next_expected) {
        let rto = calculate_rto(info.retry_count);
        if info.sent_time.elapsed() > rto {
            socket.send_to(&info.packet, server_addr)?;
            info.sent_time = Instant::now();
            info.retry_count += 1;
            println!(
                "Retransmitting seq {} (retry {})",
                next_expected, info.retry_count
            );
        }
    }
    Ok(())
}

fn transmit_loop(address: &str, size: usize, character: u8) -> Result<u8, Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(SOCKET_READ_TIMEOUT))?;

    let server_addr: SocketAddr = address.parse()?;

    let mut state = TransmissionState {
        transmitted: 0,
        next_seq: 1,
        checknum: 0,
        unacked_packets: HashMap::new(),
        last_acked_seq: 0,
    };

    let loop_start = Instant::now();
    println!("Starting transmission loop...");

    while state.transmitted < size || !state.unacked_packets.is_empty() {
        if loop_start.elapsed() > GLOBAL_TIMEOUT {
            return Err(format!(
                "Global timeout exceeded after {:?}. Remaining unacked: {}",
                GLOBAL_TIMEOUT,
                state.unacked_packets.len()
            )
            .into());
        }

        // Send new packets up to window size
        send_new_packets(&socket, server_addr, &mut state, size, character)?;

        // Try to receive ACK
        let mut ack_buf = [0u8; 5];
        match socket.recv_from(&mut ack_buf) {
            Ok((n, _)) => {
                if n >= 5 {
                    let acked_seq =
                        u32::from_be_bytes([ack_buf[0], ack_buf[1], ack_buf[2], ack_buf[3]]);
                    let checknum = ack_buf[4];
                    handle_ack(&mut state, acked_seq, checknum);
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut
                {
                    // Timeout - retransmit only the next expected packet
                    retransmit_next_expected(&socket, server_addr, &mut state)?;
                }
            }
        }
    }

    println!(
        "All packets acknowledged. Last checknum: {}",
        state.checknum
    );
    Ok(state.checknum)
}
