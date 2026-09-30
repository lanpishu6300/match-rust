//! `mold_dpdk_bench` — DPDK receive-path benchmark (Linux only).
//!
//! Generates a large pcap (N frames, one Mold message each, seq 1..=N),
//! replays it through the real DPDK rx path (net_pcap PMD) and measures
//! end-to-end parse throughput. NOTE: net_pcap replays from a file, so the
//! wall-clock throughput is bounded by libpcap file I/O — it is a correctness
//! + CPU-cost benchmark of the DPDK burst/zero-copy path, not a physical-NIC
//! line-rate benchmark. Real NIC numbers need MLX5/i40e-class hardware.
//!
//! Usage: mold_dpdk_bench [N]

#[cfg(target_os = "linux")]
mod real {
    use match_dpdk_io::dpdk::port::{rx_burst, setup_port};
    use match_dpdk_io::dpdk::{
        attach_pcap_vdev, eal_cleanup, eal_init, mbuf_pool, rte_mbuf, rte_pktmbuf_free, udp_payload,
    };
    use match_moldudp64::MoldSubscriber;
    use match_moldudp64::types::DownstreamHeader;
    use std::io::Write;
    use std::net::SocketAddr;
    use std::time::Instant;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let n: usize = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(100_000);
        let path = "/data/bench.pcap";

        // ---- generate N-frame pcap (one Mold message per frame) ----
        let mut out = std::fs::File::create(path)?;
        let mut gh = [0u8; 24];
        gh[..4].copy_from_slice(&0xa1b2c3d4u32.to_le_bytes());
        gh[4..6].copy_from_slice(&2u16.to_le_bytes());
        gh[6..8].copy_from_slice(&4u16.to_le_bytes());
        gh[16..20].copy_from_slice(&262_144u32.to_le_bytes());
        gh[20..24].copy_from_slice(&1u32.to_le_bytes());
        out.write_all(&gh)?;

        let dst_mac = [0x01, 0x00, 0x5e, 0x00, 0xff, 0x01];
        let src_mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let dst_ip = [239, 0, 255, 1];
        let src_ip = [10, 0, 0, 1];
        let session = *b"MATCH_RUST";
        let msg = [0x42u8; 24];

        for i in 0..n {
            let seq = i as u64 + 1;
            let header = DownstreamHeader { session_id: session, seq, msg_count: 1 };
            let mut mold = [0u8; 20];
            header.encode(&mut mold);
            let udp_len = 8 + 20 + 2 + msg.len();
            let mut frame = Vec::with_capacity(14 + 20 + 8 + 20 + 2 + msg.len());
            frame.extend_from_slice(&dst_mac);
            frame.extend_from_slice(&src_mac);
            frame.extend_from_slice(&[0x08, 0x00]);
            frame.push(0x45);
            frame.push(0);
            frame.extend_from_slice(&((20 + udp_len) as u16).to_be_bytes());
            frame.extend_from_slice(&[0, 0, 0, 0]);
            frame.push(64);
            frame.push(17);
            frame.extend_from_slice(&[0, 0]);
            frame.extend_from_slice(&src_ip);
            frame.extend_from_slice(&dst_ip);
            frame.extend_from_slice(&50000u16.to_be_bytes());
            frame.extend_from_slice(&50000u16.to_be_bytes());
            frame.extend_from_slice(&(udp_len as u16).to_be_bytes());
            frame.extend_from_slice(&[0, 0]);
            frame.extend_from_slice(&mold);
            frame.extend_from_slice(&(msg.len() as u16).to_be_bytes());
            frame.extend_from_slice(&msg);

            let mut ph = [0u8; 16];
            ph[..4].copy_from_slice(&(1_700_000_000u32).to_le_bytes());
            ph[4..8].copy_from_slice(&0u32.to_le_bytes());
            ph[8..12].copy_from_slice(&(frame.len() as u32).to_le_bytes());
            ph[12..16].copy_from_slice(&(frame.len() as u32).to_le_bytes());
            out.write_all(&ph)?;
            out.write_all(&frame)?;
        }
        drop(out);

        // ---- DPDK rx + parse ----
        eal_init(&[0], &[])?;
        let pool = mbuf_pool("bench_mb")?;
        let port = attach_pcap_vdev("net_pcap0", &format!("rx_pcap={path}"))?;
        unsafe { setup_port(port, pool)?; }

        let mut sub = MoldSubscriber::new_unicast(
            "MATCH_RUST",
            "127.0.0.1:0".parse::<SocketAddr>()?,
            None,
        )?;

        let mut mbufs: Vec<*mut rte_mbuf> = vec![std::ptr::null_mut(); 32];
        let mut seen = 0usize;
        let t0 = Instant::now();
        'outer: loop {
            let nb = unsafe { rx_burst(port, &mut mbufs) };
            if nb == 0 {
                break 'outer;
            }
            for i in 0..nb as usize {
                let m = mbufs[i];
                if let Some((ptr, len)) = (unsafe { udp_payload(m) }) {
                    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
                    let out = sub.parse_packet(slice);
                    if !out.is_heartbeat {
                        seen += out.messages.len();
                    }
                }
                unsafe { rte_pktmbuf_free(m) };
            }
        }
        let elapsed = t0.elapsed();
        let last_seq = sub.last_seq();

        let secs = elapsed.as_secs_f64();
        println!("== DPDK rx path (net_pcap replay, N={n}) ==");
        println!("frames parsed: {seen}, last_seq={last_seq:?}");
        println!("elapsed: {:.3} s (incl. libpcap file read)", secs);
        println!("throughput: {:.0} msg/s (replay-bounded)", seen as f64 / secs);
        println!(
            "per-frame CPU cost: {:.0} ns",
            elapsed.as_nanos() as f64 / seen.max(1) as f64
        );
        println!("NOTE: net_pcap reads from file — this is I/O-bounded; the CPU-side");
        println!("cost above is the useful number. Physical-NIC latency needs real HW.");

        unsafe { eal_cleanup() };
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("dpdk_bench error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("mold_dpdk_bench is Linux-only");
        std::process::exit(1);
    }
}
