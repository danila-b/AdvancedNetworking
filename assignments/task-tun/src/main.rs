use clap::Parser;
use etherparse::{InternetSlice, IpPayloadSlice, SlicedPacket, TransportSlice};
use mio::{net::UdpSocket, unix::SourceFd, Events, Interest, Poll, Token};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::io::AsRawFd;

const TUN_TOKEN: Token = Token(0);
const SOCKET_TOKEN: Token = Token(1);
const TAYLOR: &[u8; 6] = b"taylor";
const ELVIS: &[u8; 5] = b"elvis";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    address: Ipv4Addr,

    #[arg(short, long)]
    destination: Ipv4Addr,

    #[arg(short='b', long)]
    udpbind: SocketAddr,

    #[arg(short='u', long)]
    udpdest: SocketAddr,
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    // Create and configure the TUN device
    let mut config = tun::Configuration::default();
    config
        .tun_name("tun0") // Interface name
        .address(args.address) // Local TUN address (10.100.0.x)
        .destination(args.destination) // Peer TUN address (10.100.0.x)
        .netmask("255.255.255.0") // Subnet mask
        .up(); // Bring interface up

    #[cfg(target_os = "linux")]
    config.platform_config(|config| {
        // requiring root privilege to acquire complete functions
        config.ensure_root_privileges(true);
    });

    let mut dev = tun::create(&config).expect("Failed to create TUN device");
    let mut socket = UdpSocket::bind(args.udpbind)?;
    let udp_dest = args.udpdest;

    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(128);

    let raw_fd = dev.as_raw_fd();
    let mut tun_source = SourceFd(&raw_fd);

    poll.registry()
        .register(&mut tun_source, TUN_TOKEN, Interest::READABLE)?;
    poll.registry()
        .register(&mut socket, SOCKET_TOKEN, Interest::READABLE)?;

    loop {
        poll.poll(&mut events, None)?;

        for event in events.iter() {
            match event.token() {
                TUN_TOKEN if event.is_readable() => {
                    handle_tun_event(&mut dev, &mut socket, udp_dest)?;
                }
                SOCKET_TOKEN if event.is_readable() => {
                    handle_socket_event(&mut dev, &mut socket)?;
                }
                _ => {}
            }
        }
    }
}

/// If we receive a packet from the TUN device, we need to parse it and send it to the UDP socket.
fn handle_tun_event(
    dev: &mut tun::Device,
    socket: &mut UdpSocket,
    udp_dest: SocketAddr,
) -> std::io::Result<()> {
    let mut buf = [0u8; 1500];
    let n = dev.read(&mut buf)?;

    if n == 0 {
        return Ok(());
    }

    let mut drop_packet = false;
    let mut duplicate = false;

    match SlicedPacket::from_ip(&buf[..n]) {
        Ok(sliced) => {
            print_packet_info(&sliced, n);

            if let Some(InternetSlice::Ipv4(ipv4)) = sliced.net {
                let payload = ipv4.payload();

                if you_shall_not_pass(TAYLOR, payload) {
                    println!("Packet from TUN contains 'taylor', dropping");
                    drop_packet = true;
                } else if you_shall_not_pass(ELVIS, payload) {
                    println!("Packet from TUN contains 'elvis', duplicating");
                    duplicate = true;
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to parse tunneled IP packet: {}", e);
        }
    }

    if drop_packet {
        // Do not forward
        return Ok(());
    }

    if duplicate {
        socket.send_to(&buf[..n], udp_dest)?;
        socket.send_to(&buf[..n], udp_dest)?;
    } else {
        socket.send_to(&buf[..n], udp_dest)?;
    }

    Ok(())
}

/// If we receive a packet from the UDP socket, we need to parse it and send it to the TUN device.
fn handle_socket_event(dev: &mut tun::Device, socket: &mut UdpSocket) -> std::io::Result<()> {
    let mut buf = [0u8; 1500];

    let (n, _src) = socket.recv_from(&mut buf)?;
    if n == 0 {
        return Ok(());
    }

    let mut drop_packet = false;

    match SlicedPacket::from_ip(&buf[..n]) {
        Ok(sliced) => {
            print_packet_info(&sliced, n);

            if let Some(InternetSlice::Ipv4(ipv4)) = sliced.net {
                let payload = ipv4.payload();
                if you_shall_not_pass(TAYLOR, payload) {
                    println!("Packet from UDP socket contains 'taylor', dropping");
                    drop_packet = true;
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to parse tunneled IP packet: {}", e);
        }
    }

    if !drop_packet {
        dev.write_all(&buf[..n])?;
    }

    Ok(())
}



fn print_packet_info(sliced: &SlicedPacket, n: usize) {
    if let Some(InternetSlice::Ipv4(ipv4)) = &sliced.net {
        // Use the IPv4 header slice to access destination and protocol
        let header = ipv4.header();
        let dst = header.destination_addr();
        let proto = header.protocol();

        match &sliced.transport {
            Some(TransportSlice::Udp(udp)) => {
                println!(
                    "dst_ip={:?} proto={:?} dst_port={:?} len={:?}",
                    dst,
                    proto,
                    udp.destination_port(),
                    n
                );
            }
            Some(TransportSlice::Tcp(tcp)) => {
                println!(
                    "dst_ip={:?} proto={:?} dst_port={:?} len={:?}",
                    dst,
                    proto,
                    tcp.destination_port(),
                    n
                );
            }
            _ => {
                println!("dst_ip={:?} proto={:?} len={:?}", dst, proto, n);
            }
        }
    } else {
        println!("Non-IPv4 packet over tunnel");
    }
}

fn you_shall_not_pass(you: &[u8], payload: &IpPayloadSlice) -> bool  {
    payload.payload.windows(you.len()).any(|window| window.eq_ignore_ascii_case(you))
}