//! # Task-UDP: Reliable Data Transfer over UDP
//!
//! This module implements a reliable data transfer protocol over UDP with the following features:
//!
//! ## Protocol Design
//!
//! ### Packet Format
//! Each UDP datagram consists of:
//! - **4 bytes**: Sequence number (big-endian, network byte order)
//! - **2 bytes**: Payload length (big-endian, network byte order)
//! - **N bytes**: Payload data (repeated character, max 1200 bytes)
//!
//! ### Acknowledgment Mechanism
//! The server uses cumulative acknowledgments (similar to TCP):
//! - **4 bytes**: Highest consecutive sequence number received
//! - **1 byte**: Check number (deterministic pseudorandom value)
//!
//! The server only advances its cumulative ACK when it receives the next expected packet
//! in sequence. Out-of-order packets are buffered but don't advance the ACK.
//!
//! **Server Limitation Workaround**: The server implementation has a limitation where it
//! only processes one consecutive out-of-order packet at a time when a gap is filled.
//! For example, if packets 2, 3, 4, 5 arrive out-of-order and packet 1 arrives later,
//! the server will only process packets 1 and 2, leaving 3, 4, 5 buffered but unprocessed.
//! To work around this, the client proactively retransmits the next few unacked packets
//! after receiving an ACK that only advances by a small amount, helping to "unlock" the
//! server's buffer.
//!
//! ## Transmission Strategy
//!
//! ### Sliding Window
//! - Starts with a window size of 10 packets
//! - Increases window size on successful ACK (up to 50)
//! - Reduces window size on timeout (halves, minimum 1)
//! - **Anti-flooding**: When 5+ unacked packets are detected, enters recovery mode
//!   (see the Anti-Flooding Mechanism section below for details)
//!
//! ### Retransmission Strategy
//! The retransmission logic prioritizes the **next expected packet** (last_acked_seq + 1)
//! because:
//! 1. The server uses cumulative ACKs - it can only advance when receiving the next
//!    consecutive packet
//! 2. Once the missing packet arrives, the server can acknowledge all consecutive
//!    packets up to the next gap
//! 3. This prevents getting stuck when packets are lost
//!
//! **Retransmission behavior:**
//! - Always prioritizes retransmitting `last_acked_seq + 1` if it's timed out
//! - When window size ≤ 2: retransmits only 1 packet at a time
//! - When window size > 2: retransmits up to 3 oldest packets
//! - Packets are sorted by sequence number to ensure oldest-first retransmission
//! - **Recovery mode** (triggered when 5+ unacked packets are detected):
//!   - Uses shorter timeout (500ms) for faster retransmission
//!   - Retransmits up to 5 packets at once
//!   - May retransmit oldest packet even if not fully timed out
//!   - See the Anti-Flooding Mechanism section for complete details
//!
//! ### Retransmission Timeout (RTO)
//! Uses logarithmic backoff to prevent network flooding:
//! - Initial RTO: 1000ms
//! - Formula: `RTO = base * log2(retry_count + 1)`
//! - Maximum RTO: 30 seconds
//! - This gives more time between retries as failures accumulate
//!
//! ### Global Timeout
//! The entire transmission has a 3-minute global timeout to prevent infinite hangs.
//!
//! ## Anti-Flooding Mechanism
//!
//! To prevent overwhelming the server with packets, the implementation includes an
//! anti-flooding mechanism that activates when too many packets are unacknowledged:
//!
//! ### Threshold Detection
//! When the number of unacknowledged packets reaches **5 or more**, the client enters
//! **recovery mode**:
//!
//! 1. **Immediate packet sending halt**: The `send_new_packets()` function immediately
//!    stops issuing new packets, preventing further flooding of the server.
//!
//! 2. **Window size reduction**: The transmission window is reduced to 1 packet to
//!    minimize the number of in-flight packets.
//!
//! 3. **Aggressive retransmission**: The client switches to aggressive retransmission
//!    mode with the following characteristics:
//!    - Uses a shorter timeout (500ms instead of the normal RTO calculation)
//!    - Retransmits up to 5 packets simultaneously (vs. 3 in normal mode)
//!    - May retransmit the oldest unacked packet even if it hasn't fully timed out
//!    - Prioritizes the next expected packet (last_acked_seq + 1) to unblock the server
//!
//! ### Recovery
//! Once the number of unacknowledged packets drops **below 5**, the client exits
//! recovery mode and resumes normal operation:
//! - New packets can be sent again (subject to window size constraints)
//! - Window size can grow again on successful ACKs (up to 50)
//! - Normal retransmission timeout calculations resume
//!
//! This mechanism ensures that the client backs off when the server appears to be
//! struggling, while still making progress through aggressive retransmission of
//! critical packets.
//!
//! ## Server Limitation Workaround
//!
//! The server implementation has a limitation where it only processes one consecutive
//! out-of-order packet at a time when a gap is filled. For example:
//! - If packets 2, 3, 4, 5 arrive out-of-order and are buffered
//! - When packet 1 arrives, the server processes packets 1 and 2
//! - But packets 3, 4, 5 remain buffered and unprocessed
//! - The server sends ACK for sequence 2 (not 5)
//!
//! To work around this, the client implements **proactive retransmission**:
//! - When an ACK is received that only advances by 1-3 packets
//! - And there are more unacked packets immediately following
//! - The client proactively retransmits the next few consecutive unacked packets
//! - This helps "unlock" the server's buffer by resending packets it has but
//!   hasn't processed yet
//!
//! This workaround prevents hangs that would otherwise occur when multiple packets
//! arrive out-of-order at the server.
//!
//! ## Error Handling
//! - All network byte conversions use big-endian (network byte order)
//! - Handles duplicate ACKs gracefully (ignores them)
//! - Logs retransmissions and ACK progress for debugging

use clap::Parser;
use std::{
    collections::HashMap,
    error::Error,
    io::{Read, Write},
    net::{TcpStream, UdpSocket, SocketAddr},
    time::{Duration, Instant},
};

const MAX_PAYLOAD: usize = 1200;
const HEADER_SIZE: usize = 6; // 4 bytes seq + 2 bytes payload len
const INITIAL_RTO_MS: u64 = 1000; // Initial retransmission timeout in milliseconds
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(180); // 3 minutes
const SOCKET_READ_TIMEOUT: Duration = Duration::from_secs(5);
const FLOOD_THRESHOLD: usize = 5; // Stop sending new packets if we have this many unacked

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
    window_size: usize,
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
    let resp: Vec<&str> = std::str::from_utf8(&buf[..n])?
        .split_whitespace()
        .collect();
    
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
    
    println!("Size: {} -- Checknum: {} -- Duration: {:?}", size, checknum, duration);
    
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
    // Logarithmically increasing timeout: base * log2(retry_count + 1)
    // Using log2 approximation: log2(n) ≈ log(n) / log(2)
    let base_ms = INITIAL_RTO_MS as f64;
    let multiplier = if retry_count == 0 {
        1.0
    } else {
        (retry_count as f64 + 1.0).ln() / 2.0_f64.ln()
    };
    let timeout_ms = (base_ms * multiplier).min(30000.0); // Cap at 30 seconds
    Duration::from_millis(timeout_ms as u64)
}

fn send_new_packets(
    socket: &UdpSocket,
    server_addr: SocketAddr,
    state: &mut TransmissionState,
    size: usize,
    character: u8,
) -> Result<(), Box<dyn Error>> {
    // Stop sending new packets if we have too many unacked (anti-flooding)
    if state.unacked_packets.len() >= FLOOD_THRESHOLD {
        return Ok(());
    }
    
    while state.transmitted < size && state.unacked_packets.len() < state.window_size {
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
        
        if state.transmitted % 10000 == 0 || state.transmitted == size {
            println!(
                "Transmitted {} / {} bytes (in flight: {})",
                state.transmitted,
                size,
                state.unacked_packets.len()
            );
        }
    }
    Ok(())
}

fn parse_ack(ack_buf: &[u8]) -> Option<(u32, u8)> {
    if ack_buf.len() < 5 {
        return None;
    }
    let acked_seq = u32::from_be_bytes([ack_buf[0], ack_buf[1], ack_buf[2], ack_buf[3]]);
    let checknum = ack_buf[4];
    Some((acked_seq, checknum))
}

fn handle_ack(
    state: &mut TransmissionState,
    acked_seq: u32,
    checknum: u8,
) {
    state.checknum = checknum;
    
    if acked_seq > state.last_acked_seq {
        let mut acked_count = 0;
        for seq in (state.last_acked_seq + 1)..=acked_seq {
            if state.unacked_packets.remove(&seq).is_some() {
                acked_count += 1;
            }
        }
        state.last_acked_seq = acked_seq;
        
        if acked_count > 0 {
            println!("ACK received: seq {}, unacked: {}", acked_seq, state.unacked_packets.len());
        }
        
        // Increase window on successful ACK
        if state.window_size < 50 {
            state.window_size += 1;
        }
    } else if acked_seq == state.last_acked_seq {
        // Duplicate ACK - server is responding but can't advance
        // This is normal when waiting for the next expected packet
        // No action needed, but confirms server is alive
    }
}

/// Proactively retransmits the next few unacked packets to work around server limitation.
///
/// The server has a bug where it only processes one consecutive out-of-order packet
/// at a time. When packets arrive out-of-order (e.g., 2, 3, 4, 5 arrive before 1),
/// and then packet 1 arrives, the server will only process packets 1 and 2, leaving
/// 3, 4, 5 buffered but unprocessed. The ACK will be for sequence 2, not 5.
///
/// This function detects when an ACK only advanced by a small amount (indicating the
/// server might have more packets buffered) and proactively retransmits the next
/// few unacked packets to help "unlock" the server's buffer.
///
/// Returns a vector of sequence numbers that should be retransmitted immediately.
fn get_proactive_retransmits(
    state: &TransmissionState,
    ack_advance: u32,
) -> Vec<u32> {
    // Only proactively retransmit if ACK advanced by a small amount (1-3 packets)
    // and we have more unacked packets immediately following
    if ack_advance > 0 && ack_advance <= 3 && !state.unacked_packets.is_empty() {
        let next_expected = state.last_acked_seq + 1;
        let mut to_retransmit = Vec::new();
        
        // Retransmit the next few consecutive unacked packets
        // This helps unlock the server's buffer if it has them buffered
        let mut seq = next_expected;
        let max_proactive = (ack_advance * 2).min(5); // Retransmit up to 5 packets
        
        while to_retransmit.len() < max_proactive as usize {
            if state.unacked_packets.contains_key(&seq) {
                to_retransmit.push(seq);
            } else {
                // Stop at first gap
                break;
            }
            seq += 1;
        }
        
        if !to_retransmit.is_empty() {
            println!(
                "Proactive retransmit: ACK advanced by {}, retransmitting next {} packet(s) to unlock server buffer",
                ack_advance,
                to_retransmit.len()
            );
        }
        
        to_retransmit
    } else {
        Vec::new()
    }
}

fn check_and_retransmit(
    socket: &UdpSocket,
    server_addr: SocketAddr,
    state: &mut TransmissionState,
) -> Result<(), Box<dyn Error>> {
    let now = Instant::now();
    let next_expected = state.last_acked_seq + 1;
    let mut to_retransmit = Vec::new();
    
    // In recovery mode (5+ unacked), retransmit more aggressively
    let in_recovery = state.unacked_packets.len() >= FLOOD_THRESHOLD;
    
    // Always prioritize the next expected packet if it exists and is timed out
    if let Some(info) = state.unacked_packets.get(&next_expected) {
        let rto = if in_recovery {
            // In recovery mode, use a shorter timeout to retransmit faster
            Duration::from_millis(INITIAL_RTO_MS / 2)
        } else {
            calculate_rto(info.retry_count)
        };
        if now.duration_since(info.sent_time) > rto {
            to_retransmit.push(next_expected);
        }
    }
    
    // If next expected is not timed out yet, or doesn't exist, find other timed-out packets
    if to_retransmit.is_empty() {
        let mut timed_out_packets: Vec<(u32, u32)> = Vec::new();
        
        for (seq, info) in &state.unacked_packets {
            let rto = if in_recovery {
                // In recovery mode, use a shorter timeout to retransmit faster
                Duration::from_millis(INITIAL_RTO_MS / 2)
            } else {
                calculate_rto(info.retry_count)
            };
            if now.duration_since(info.sent_time) > rto {
                timed_out_packets.push((*seq, info.retry_count));
            }
        }
        
        if timed_out_packets.is_empty() {
            // In recovery mode, if nothing is timed out yet, retransmit the oldest packet anyway
            if in_recovery && !state.unacked_packets.is_empty() {
                let mut seqs: Vec<u32> = state.unacked_packets.keys().copied().collect();
                seqs.sort();
                if let Some(oldest_seq) = seqs.first() {
                    to_retransmit.push(*oldest_seq);
                }
            } else {
                return Ok(());
            }
        } else {
            // Sort by sequence number to prioritize the lowest
            timed_out_packets.sort_by_key(|(seq, _)| *seq);
            
            // In recovery mode, retransmit more packets aggressively
            let max_retransmit = if in_recovery {
                timed_out_packets.len().min(5) // Retransmit up to 5 packets in recovery
            } else if state.window_size <= 2 {
                1
            } else {
                timed_out_packets.len().min(3)
            };
            
            to_retransmit = timed_out_packets
                .into_iter()
                .take(max_retransmit)
                .map(|(seq, _)| seq)
                .collect();
        }
    } else {
        // Next expected packet is timed out - retransmit it and maybe a couple more
        let mut timed_out_packets: Vec<(u32, u32)> = Vec::new();
        
        for (seq, info) in &state.unacked_packets {
            if *seq == next_expected {
                continue; // Already added
            }
            let rto = if in_recovery {
                Duration::from_millis(INITIAL_RTO_MS / 2)
            } else {
                calculate_rto(info.retry_count)
            };
            if now.duration_since(info.sent_time) > rto {
                timed_out_packets.push((*seq, info.retry_count));
            }
        }
        
        timed_out_packets.sort_by_key(|(seq, _)| *seq);
        
        // In recovery mode, retransmit more packets
        if in_recovery {
            let additional: Vec<u32> = timed_out_packets
                .into_iter()
                .take(4) // Retransmit up to 4 more packets in recovery
                .map(|(seq, _)| seq)
                .collect();
            to_retransmit.extend(additional);
        } else if state.window_size > 1 {
            let additional: Vec<u32> = timed_out_packets
                .into_iter()
                .take(2)
                .map(|(seq, _)| seq)
                .collect();
            to_retransmit.extend(additional);
        }
    }
    
    if to_retransmit.is_empty() {
        return Ok(());
    }
    
    println!(
        "Retransmitting {} packet(s) (next expected: {}, window: {}, recovery: {})",
        to_retransmit.len(),
        next_expected,
        state.window_size,
        in_recovery
    );
    
    for seq in &to_retransmit {
        if let Some(info) = state.unacked_packets.get_mut(seq) {
            socket.send_to(&info.packet, server_addr)?;
            info.sent_time = Instant::now();
            info.retry_count += 1;
        }
    }
    
    // Reduce window size on timeout (but don't go below 1)
    // Don't reduce further if already in recovery mode
    if !in_recovery {
        state.window_size = (state.window_size / 2).max(1);
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
        window_size: 10,
    };
    
    let loop_start = Instant::now();
    println!("Starting transmission loop...");
    
    while state.transmitted < size || !state.unacked_packets.is_empty() {
        // Check global timeout
        if loop_start.elapsed() > GLOBAL_TIMEOUT {
            return Err(format!(
                "Global timeout exceeded after {:?}. Remaining unacked packets: {}",
                GLOBAL_TIMEOUT,
                state.unacked_packets.len()
            ).into());
        }
        
        // Anti-flooding: if we have 5+ unacked packets, stop sending and reduce window
        if state.unacked_packets.len() >= FLOOD_THRESHOLD {
            if state.window_size > 1 {
                println!(
                    "Flood threshold reached ({} unacked packets). Reducing window to 1 and entering recovery mode.",
                    state.unacked_packets.len()
                );
                state.window_size = 1;
            }
            // Force retransmission in recovery mode
            check_and_retransmit(&socket, server_addr, &mut state)?;
        } else {
            // Normal operation: send new packets if window allows
            send_new_packets(&socket, server_addr, &mut state, size, character)?;
        }
        
        // Log status periodically
        if state.transmitted >= size && !state.unacked_packets.is_empty() {
            if state.unacked_packets.len() % 10 == 0 || state.unacked_packets.len() < 5 {
                println!("Waiting for {} unacked packets...", state.unacked_packets.len());
            }
        }
        
        // Try to receive ACK
        let mut ack_buf = [0u8; 5];
        match socket.recv_from(&mut ack_buf) {
            Ok((n, _)) => {
                if let Some((acked_seq, checknum)) = parse_ack(&ack_buf[..n]) {
                    let old_acked = state.last_acked_seq;
                    handle_ack(&mut state, acked_seq, checknum);
                    
                    // Workaround for server limitation: proactively retransmit next packets
                    // if ACK only advanced by a small amount (server might have more buffered)
                    let ack_advance = acked_seq.saturating_sub(old_acked);
                    let proactive_retransmits = get_proactive_retransmits(&state, ack_advance);
                    
                    for seq in proactive_retransmits {
                        if let Some(info) = state.unacked_packets.get_mut(&seq) {
                            socket.send_to(&info.packet, server_addr)?;
                            info.sent_time = Instant::now();
                            // Don't increment retry_count for proactive retransmits
                            // as these are not timeout-based retransmissions
                        }
                    }
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock || 
                   e.kind() == std::io::ErrorKind::TimedOut {
                    check_and_retransmit(&socket, server_addr, &mut state)?;
                } else {
                    eprintln!("Unexpected error receiving ACK: {}", e);
                }
            }
        }
    }
    
    println!("All packets acknowledged. Last checknum: {}", state.checknum);
    Ok(state.checknum)
}
