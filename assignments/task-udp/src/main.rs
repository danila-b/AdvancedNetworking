mod congestion;
mod rtt;

use clap::Parser;
use congestion::CongestionControl;
use rtt::RttEstimator;
use std::{
    collections::HashMap,
    error::Error,
    io::{Read, Write},
    net::{SocketAddr, TcpStream, UdpSocket},
    time::{Duration, Instant},
};

const MAX_PAYLOAD: usize = 1200;
const HEADER_SIZE: usize = 6; // 4 bytes seq + 2 bytes payload len
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(180);
const SOCKET_READ_TIMEOUT: Duration = Duration::from_millis(50);
const TCP_PORT: u16 = 12345;
const UDP_PORT: u16 = 20000;
const DUP_ACK_THRESHOLD: u32 = 3;

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
    dup_ack_count: u32,
    rtt: RttEstimator,
    cc: CongestionControl,
}

impl TransmissionState {
    fn new() -> Self {
        Self {
            transmitted: 0,
            next_seq: 1,
            checknum: 0,
            unacked_packets: HashMap::new(),
            last_acked_seq: 0,
            dup_ack_count: 0,
            rtt: RttEstimator::new(),
            cc: CongestionControl::new(),
        }
    }

    fn create_packet(seq: u32, payload_size: usize, character: u8) -> Vec<u8> {
        let mut packet = Vec::with_capacity(HEADER_SIZE + payload_size);
        packet.extend_from_slice(&seq.to_be_bytes());
        packet.extend_from_slice(&(payload_size as u16).to_be_bytes());
        packet.extend(vec![character; payload_size]);
        packet
    }

    fn send_new_packets(
        &mut self,
        socket: &UdpSocket,
        server_addr: SocketAddr,
        size: usize,
        character: u8,
    ) -> Result<(), Box<dyn Error>> {
        while self.transmitted < size && self.unacked_packets.len() < self.cc.window() {
            let payload_size = (size - self.transmitted).min(MAX_PAYLOAD);
            let packet = Self::create_packet(self.next_seq, payload_size, character);
            socket.send_to(&packet, server_addr)?;

            self.unacked_packets.insert(
                self.next_seq,
                PacketInfo { packet, sent_time: Instant::now(), retry_count: 0 },
            );
            self.transmitted += payload_size;
            self.next_seq += 1;
        }
        Ok(())
    }

    /// Returns true if fast retransmit should be triggered
    fn handle_ack(&mut self, acked_seq: u32, checknum: u8) -> bool {
        self.checknum = checknum;

        if acked_seq > self.last_acked_seq {
            // Update RTT from oldest acked packet - we skip retransmitted packets
            if let Some(info) = self.unacked_packets.get(&(self.last_acked_seq + 1)) {
                if info.retry_count == 0 {
                    self.rtt.update(info.sent_time.elapsed().as_secs_f64() * 1000.0);
                }
            }
            // Remove acked packets and grow window
            for seq in (self.last_acked_seq + 1)..=acked_seq {
                self.unacked_packets.remove(&seq);
                self.cc.on_ack();
            }
            self.last_acked_seq = acked_seq;
            self.dup_ack_count = 0;
            false
        } else if acked_seq == self.last_acked_seq {
            self.dup_ack_count += 1;
            if self.dup_ack_count >= DUP_ACK_THRESHOLD {
                self.cc.on_fast_retransmit();
                self.dup_ack_count = 0;
                return true;
            }
            false
        } else {
            false
        }
    }

    fn retransmit_if_needed(
        &mut self,
        socket: &UdpSocket,
        server_addr: SocketAddr,
        force: bool,
    ) -> Result<(), Box<dyn Error>> {
        let next_expected = self.last_acked_seq + 1;
        if let Some(info) = self.unacked_packets.get_mut(&next_expected) {
            if force || info.sent_time.elapsed() > self.rtt.rto {
                if !force {
                    self.cc.on_timeout();
                    self.rtt.backoff();
                }
                socket.send_to(&info.packet, server_addr)?;
                info.sent_time = Instant::now();
                info.retry_count += 1;
            }
        }
        Ok(())
    }

    fn is_complete(&self, size: usize) -> bool {
        self.transmitted >= size && self.unacked_packets.is_empty()
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    println!("Task-UDP starting");
    println!("Connecting to server: {}", args.server);
    println!("Using keyword: {}", args.keyword);

    let start = Instant::now();
    let tcp_address = format!("{}:{}", args.server, TCP_PORT);
    let mut tcp_stream = TcpStream::connect(&tcp_address)?;

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
    let udp_address = format!("{}:{}", tcp_addr.ip(), UDP_PORT);

    let checknum = transmit_loop(&udp_address, size, char_byte)?;
    let duration = start.elapsed();

    println!(
        "Size: {} -- Checknum: {} -- Duration: {:?}",
        size, checknum, duration
    );

    Ok(())
}

fn transmit_loop(address: &str, size: usize, character: u8) -> Result<u8, Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(SOCKET_READ_TIMEOUT))?;
    let server_addr: SocketAddr = address.parse()?;
    let mut state = TransmissionState::new();
    let loop_start = Instant::now();

    while !state.is_complete(size) {
        if loop_start.elapsed() > GLOBAL_TIMEOUT {
            return Err(format!("Timeout after {:?}", GLOBAL_TIMEOUT).into());
        }

        state.send_new_packets(&socket, server_addr, size, character)?;

        let mut ack_buf = [0u8; 5];
        match socket.recv_from(&mut ack_buf) {
            Ok((n, _)) if n >= 5 => {
                let acked_seq = u32::from_be_bytes(ack_buf[0..4].try_into().unwrap());
                if state.handle_ack(acked_seq, ack_buf[4]) {
                    state.retransmit_if_needed(&socket, server_addr, true)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                   || e.kind() == std::io::ErrorKind::TimedOut => {
                state.retransmit_if_needed(&socket, server_addr, false)?;
            }
            _ => {}
        }
    }

    Ok(state.checknum)
}
