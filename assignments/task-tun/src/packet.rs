use etherparse::{InternetSlice, IpPayloadSlice, SlicedPacket, TransportSlice};

pub fn print_packet_info(sliced: &SlicedPacket, n: usize) {
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

pub fn you_shall_not_pass(you: &[u8], payload: &IpPayloadSlice) -> bool {
    payload
        .payload
        .windows(you.len())
        .any(|window| window.eq_ignore_ascii_case(you))
}

pub fn encrypt(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        *byte = byte.wrapping_add(3);
    }
}

pub fn decrypt(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        *byte = byte.wrapping_sub(3);
    }
}