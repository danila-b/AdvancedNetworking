use std::time::Duration;

use anyhow::Context as _;
use aya::maps::Array;
use aya::programs::{Xdp, XdpFlags};
use clap::Parser;
use tokio::{signal, time};

#[derive(Debug, Parser)]
struct Opt {
    #[clap(short, long, default_value = "veth0")]
    iface: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opt = Opt::parse();

    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
    if ret != 0 {
        eprintln!("Failed to remove limit on locked memory, ret is: {ret}");
    }

    let mut ebpf = aya::Ebpf::load(aya::include_bytes_aligned!(concat!(
        env!("OUT_DIR"),
        "/task-ebpf"
    )))?;

    let program: &mut Xdp = ebpf.program_mut("task_ebpf").unwrap().try_into()?;
    program.load()?;
    program
        .attach(&opt.iface, XdpFlags::SKB_MODE)
        .context("failed to attach XDP program")?;

    println!("Attached XDP on {}. Press Ctrl-C to stop.", opt.iface);

    let counters: Array<_, u64> = Array::try_from(ebpf.map_mut("COUNTERS").unwrap())?;
    let mut interval = time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => break,
            _ = interval.tick() => {
                let tcp_443 = counters.get(&0, 0).unwrap_or(0);
                let udp_443 = counters.get(&1, 0).unwrap_or(0);
                let icmp = counters.get(&2, 0).unwrap_or(0);
                let tcp_80 = counters.get(&3, 0).unwrap_or(0);
                println!(
                    "TCP/443={tcp_443}  UDP/443={udp_443}  ICMP={icmp}  dropped:TCP/80={tcp_80}"
                );
            }
        }
    }

    println!("Exiting...");
    Ok(())
}
