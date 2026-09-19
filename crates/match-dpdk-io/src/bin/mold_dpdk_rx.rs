//! `mold_dpdk_rx` — receive MoldUDP64 frames through the real DPDK rx path
//! (net_pcap PMD replaying a pcap file), parse them with match-moldudp64's
//! subscriber, and assert the expected sequence / gap shape.
//!
//! Usage: mold_dpdk_rx <input.pcap>

#[cfg(target_os = "linux")]
mod real {
    use match_dpdk_io::dpdk::port::{rx_burst, setup_port};
    use match_dpdk_io::dpdk::{
        attach_pcap_vdev, eal_cleanup, eal_init, mbuf_pool, rte_mbuf, rte_pktmbuf_free, udp_payload,
    };
    use match_moldudp64::MoldSubscriber;
    use std::net::SocketAddr;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args();
        let _bin = args.next();
        let first = args.next().unwrap_or_else(|| "/data/input.pcap".into());

        let consumed = eal_init(&[0], &[])?;
        eprintln!("EAL initialized (consumed {consumed} args)");

        let pool = mbuf_pool("mold_mb")?;

        // --nic <port>: real physical NIC (PCI PMD). Continuous rx loop,
        // Ctrl-C to stop; env DPDK_EAL_NATIVE=1 must be set by the caller.
        let port: u16;
        let finite: bool;
        let mut scenario = "input".to_string();
        if first == "--nic" {
            port = args.next().unwrap_or_else(|| "0".into()).parse()?;
            finite = false;
            eprintln!("using physical port {port} (native PMD)");
            unsafe { setup_port(port, pool)?; }
        } else {
            let file = first;
            scenario = args.next().unwrap_or_else(|| "input".into());
            port = attach_pcap_vdev("net_pcap0", &format!("rx_pcap={file}"))?;
            finite = true;
            eprintln!("attached net_pcap0 (rx {file}) as port {port}, scenario={scenario}");
            unsafe { setup_port(port, pool)?; }
        }

        // Subscriber bound to a dummy unicast addr (the pcap PMD feeds
        // parse_packet directly; no real socket traffic).
        let mut sub = MoldSubscriber::new_unicast(
            "MATCH_RUST",
            "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            None,
        )?;

        let mut mbufs: Vec<*mut rte_mbuf> = vec![std::ptr::null_mut(); 32];
        let mut packets_seen = 0u32;
        let mut total_msgs = 0u32;
        let mut gaps: Vec<(u64, u64)> = Vec::new();
        let mut seqs: Vec<u64> = Vec::new();

        'outer: loop {
            let nb = unsafe { rx_burst(port, &mut mbufs) };
            if nb == 0 {
                if !finite {
                    // Real NIC: keep polling (DPDK busy-poll, like production).
                    std::hint::spin_loop();
                    continue 'outer;
                }
                break 'outer; // pcap exhausted
            }
            for i in 0..nb as usize {
                let m = mbufs[i];
                let Some((ptr, len)) = (unsafe { udp_payload(m) }) else {
                    unsafe { rte_pktmbuf_free(m) };
                    continue;
                };
                let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
                let out = sub.parse_packet(slice);
                packets_seen += 1;
                if out.is_heartbeat {
                    eprintln!("  [heartbeat] seq={}", out.first_seq);
                } else {
                    eprintln!(
                        "  [data] first_seq={} count={} msgs={} gap={:?}",
                        out.first_seq,
                        out.msg_count,
                        out.messages.len(),
                        out.gap
                    );
                    for (s, _) in &out.messages {
                        seqs.push(*s);
                        total_msgs += 1;
                    }
                    if let Some(g) = out.gap {
                        gaps.push((g.expected, g.lost));
                    }
                }
                unsafe { rte_pktmbuf_free(m) };
            }
        }

        if !finite {
            // Physical-NIC mode: no pcap assertions — print cumulative stats
            // forever (Ctrl-C to stop) so an external publisher can be measured.
            eprintln!(
                "  [running] frames={packets_seen} msgs={total_msgs} last_seq={:?} gaps={gaps:?}",
                sub.last_seq()
            );
            return Ok(());
        }

        // ---- Assertions (input = gen_pcap gap scenario; output = tx round trip) ----
        let mut pass = true;
        macro_rules! check {
            ($cond:expr, $msg:expr) => {{
                if $cond {
                    eprintln!("  [PASS] {msg}", msg = $msg);
                } else {
                    eprintln!("  [FAIL] {msg}", msg = $msg);
                    pass = false;
                }
            }};
        }
        match scenario.as_str() {
            "input" => {
                check!(packets_seen == 5, format!("5 frames received (got {packets_seen})"));
                check!(total_msgs == 5, format!("5 messages parsed (got {total_msgs})"));
                check!(seqs == vec![1, 2, 3, 5, 6], format!("seq sequence correct (got {seqs:?})"));
                check!(gaps == vec![(4, 1)], format!("gap detected at 4, lost 1 (got {gaps:?})"));
                check!(sub.last_seq() == Some(6), format!("last_seq advanced to 6 (got {:?})", sub.last_seq()));
            }
            "output" => {
                check!(packets_seen == 3, format!("3 frames received (got {packets_seen})"));
                check!(total_msgs == 3, format!("3 messages parsed (got {total_msgs})"));
                check!(seqs == vec![1, 2, 3], format!("seq sequence correct (got {seqs:?})"));
                check!(gaps.is_empty(), format!("no gap expected (got {gaps:?})"));
                check!(sub.last_seq() == Some(3), format!("last_seq advanced to 3 (got {:?})", sub.last_seq()));
            }
            other => {
                eprintln!("unknown expect mode: {other}");
                std::process::exit(2);
            }
        }

        unsafe { eal_cleanup() };
        if pass {
            println!("DPDK_RX_VERIFY: PASS");
            Ok(())
        } else {
            println!("DPDK_RX_VERIFY: FAIL");
            std::process::exit(1);
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("rx error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("match-dpdk-io bins are Linux-only");
        std::process::exit(1);
    }
}
