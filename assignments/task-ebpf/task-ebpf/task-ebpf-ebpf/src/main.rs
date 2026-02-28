#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::xdp_action,
    macros::{map, xdp},
    maps::Array,
    programs::XdpContext,
};
use core::mem;
use network_types::{
    eth::{EthHdr, EtherType},
    ip::{Ipv4Hdr, IpProto},
    tcp::TcpHdr,
    udp::UdpHdr,
};

const TCP_443: u32 = 0;
const UDP_443: u32 = 1;
const ICMP: u32 = 2;
const TCP_80: u32 = 3;

#[map]
static COUNTERS: Array<u64> = Array::with_max_entries(4, 0);

#[inline(always)]
fn ptr_at<T>(ctx: &XdpContext, offset: usize) -> Result<*const T, ()> {
    let start = ctx.data();
    let end = ctx.data_end();
    if start + offset + mem::size_of::<T>() > end {
        return Err(());
    }
    Ok((start + offset) as *const T)
}

fn increment(idx: u32) {
    if let Some(cnt) = COUNTERS.get_ptr_mut(idx) {
        unsafe { *cnt += 1 };
    }
}

#[xdp]
pub fn task_ebpf(ctx: XdpContext) -> u32 {
    match try_task_ebpf(ctx) {
        Ok(ret) => ret,
        Err(_) => xdp_action::XDP_ABORTED,
    }
}

fn try_task_ebpf(ctx: XdpContext) -> Result<u32, ()> {
    let eth: *const EthHdr = ptr_at(&ctx, 0)?;
    if unsafe { (*eth).ether_type } != EtherType::Ipv4 {
        return Ok(xdp_action::XDP_PASS);
    }

    let ip: *const Ipv4Hdr = ptr_at(&ctx, EthHdr::LEN)?;
    let proto = unsafe { (*ip).proto };
    let transport_offset = EthHdr::LEN + Ipv4Hdr::LEN;

    match proto {
        IpProto::Tcp => {
            let tcp: *const TcpHdr = ptr_at(&ctx, transport_offset)?;
            let dest = u16::from_be(unsafe { (*tcp).dest });
            match dest {
                443 => {
                    increment(TCP_443);
                    Ok(xdp_action::XDP_PASS)
                }
                80 => {
                    increment(TCP_80);
                    Ok(xdp_action::XDP_DROP)
                }
                _ => Ok(xdp_action::XDP_PASS),
            }
        }
        IpProto::Udp => {
            let udp: *const UdpHdr = ptr_at(&ctx, transport_offset)?;
            let dest = u16::from_be(unsafe { (*udp).dest });
            if dest == 443 {
                increment(UDP_443);
            }
            Ok(xdp_action::XDP_PASS)
        }
        IpProto::Icmp => {
            increment(ICMP);
            Ok(xdp_action::XDP_PASS)
        }
        _ => Ok(xdp_action::XDP_PASS),
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
