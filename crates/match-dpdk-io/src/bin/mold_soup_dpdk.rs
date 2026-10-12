//! `mold_soup_dpdk` — SoupBinTCP over the DPDK rx path: generate a pcap whose
//! frames carry complete SoupBinTCP packets (UDP-encapsulated, as a TCP-stack
//! offload would present them), replay them through the real DPDK net_pcap
//! PMD, then parse with match-soupbintcp and assert sequence continuity.
//!
//! Honest scope: DPDK receives frames; the SoupBinTCP parser consumes the
//! byte stream. A real deployment pairs DPDK with a user-space TCP stack
//! (or kernel TCP for the order-entry path); the session layer itself is
//! transport-agnostic and verified here end-to-end.
//!
//! Usage: mold_soup_dpdk [frames] [messages-per-frame]

#[cfg(target_os = "linux")]
mod real {
    use match_dpdk_io::dpdk::port::{rx_burst, setup_port};
    use match_dpdk_io::dpdk::{
        attach_pcap_vdev, eal_cleanup, eal_init, mbuf_pool, rte_mbuf, rte_pktmbuf_free, udp_payload,
    };
    use match_soupbintcp::packet::{self, Packet};
    use match_soupbintcp::session::ClientSession;
    use std::net::SocketAddr;
    use std::time::{SystemTime, UNIX_EPOCH};

    const SESSION: &str = "SESSION-A";

    fn now_ns() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    /// A canned Accepted report payload (OUCH 'A').
    fn accepted_report(i: u64) -> Vec<u8> {
        use match_soupbintcp::ouch::{self, Accepted};
        let m = Accepted {
            timestamp: now_ns(),
            token: format!("ORD-{i:06}"),
            side: b'B',
            shares: 100,
            stock: "AAPL".into(),
            price: 195_5000,
            time_in_force: 99998,
            firm: "ABCD".into(),
            display: b'Y',
            ref_number: 1_000_000 + i,
            order_state: b'L',
        };
        ouch::encode_accepted(&m)
    }

    /// Build a UDP datagram whose payload is one SoupBinTCP packet.
    fn soup_datagram(pkt: &Packet, src_port: u16, dst_port: u16) -> Vec<u8> {
        // 14B eth + 20B IPv4 + 8B UDP + soup packet
        let soup = pkt.to_bytes();
        let mut f = Vec::with_capacity(42 + soup.len());
        // Ethernet: dst 00:11:22:33:44:55, src 66:77:88:99:aa:bb, type 0x0800
        f.extend_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0x08, 0x00]);
        let total_len = (20 + 8 + soup.len()) as u16;
        // IPv4 header (20B, no options)
        f.push(0x45);
        f.push(0x00);
        f.extend_from_slice(&total_len.to_be_bytes());
        f.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // id/flags
        f.push(64); // ttl
        f.push(17); // udp
        f.extend_from_slice(&[0x00, 0x00]); // checksum (0, offload)
        f.extend_from_slice(&[10, 0, 0, 1]);
        f.extend_from_slice(&[10, 0, 0, 2]);
        // UDP header
        f.extend_from_slice(&src_port.to_be_bytes());
        f.extend_from_slice(&dst_port.to_be_bytes());
        f.extend_from_slice(&((8 + soup.len()) as u16).to_be_bytes());
        f.extend_from_slice(&[0x00, 0x00]);
        f.extend_from_slice(&soup);
        f
    }

    /// Write a global pcap file (little-endian record headers).
    fn write_pcap(path: &str, frames: &[Vec<u8>]) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(path)?;
        f.write_all(&0xa1b2c3d4u32.to_le_bytes())?;
        f.write_all(&2u16.to_le_bytes())?;
        f.write_all(&4u16.to_le_bytes())?;
        f.write_all(&0u32.to_le_bytes())?;
        f.write_all(&0u32.to_le_bytes())?;
        f.write_all(&65535u32.to_le_bytes())?;
        f.write_all(&1u32.to_le_bytes())?; // LINKTYPE_ETHERNET
        for frame in frames {
            let n = frame.len() as u32;
            f.write_all(&n.to_le_bytes())?; // ts_sec
            f.write_all(&n.to_le_bytes())?; // ts_usec
            f.write_all(&n.to_le_bytes())?; // incl_len
            f.write_all(&n.to_le_bytes())?; // orig_len
            f.write_all(frame)?;
        }
        Ok(())
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let frames_n: usize = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000);
        let per_frame: usize = std::env::args()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        // ---- build pcap: Login Accepted first, then N Sequenced reports ----
        let mut frames: Vec<Vec<u8>> = Vec::with_capacity(frames_n + 1);
        let mut expected_msgs = 0u64;
        let mut seq = 1u64;
        let mut session = ClientSession::new(SESSION);

        // Frame 0: Login Accepted (start_seq=1)
        let login = packet::login_accepted(SESSION, seq);
        frames.push(soup_datagram(&login, 10000, 20000));
        session.handle_packet(&login, now_ns())?;

        let mut total_reports = 0u64;
        for f in 0..frames_n {
            // Each frame carries one Sequenced Data packet with one report.
            let report = accepted_report(total_reports);
            let sp = Packet::new(packet::SEQUENCED_DATA, report);
            let _ = per_frame;
            frames.push(soup_datagram(&sp, 10000, 20000));
            session.handle_packet(&sp, now_ns())?;
            seq += 1;
            total_reports += 1;
            expected_msgs += 1;
        }

        let pcap = "/data/soup-input.pcap";
        write_pcap(pcap, &frames)?;

        // ---- DPDK rx path ----
        let consumed = eal_init(&[0], &[])?;
        eprintln!("EAL initialized (consumed {consumed} args)");
        let pool = mbuf_pool("soup_mb")?;
        let port = attach_pcap_vdev("net_pcap0", &format!("rx_pcap={pcap}"))?;
        unsafe { setup_port(port, pool)?; }

        // Fresh parser: replay the frames through DPDK and parse.
        let mut client = ClientSession::new(SESSION);
        let mut mbufs: Vec<*mut rte_mbuf> = vec![std::ptr::null_mut(); 32];
        let mut packets_seen = 0u64;
        let mut seq_reports = 0u64;
        let mut login_ok = false;
        'outer: loop {
            let nb = unsafe { rx_burst(port, &mut mbufs) };
            if nb == 0 {
                break 'outer;
            }
            for i in 0..nb as usize {
                let m = mbufs[i];
                let Some((ptr, len)) = (unsafe { udp_payload(m) }) else {
                    unsafe { rte_pktmbuf_free(m) };
                    continue;
                };
                let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
                let (pkts, _consumed, _) = packet::parse_stream(slice);
                for pkt in pkts {
                    packets_seen += 1;
                    match pkt.packet_type {
                        packet::LOGIN_ACCEPTED => {
                            let acc = pkt.parse_login_accepted();
                            login_ok = acc.sequence == 1;
                            client.handle_packet(&pkt, now_ns())?;
                        }
                        packet::SEQUENCED_DATA => {
                            client.handle_packet(&pkt, now_ns())?;
                            seq_reports += 1;
                        }
                        _ => {}
                    }
                }
                unsafe { rte_pktmbuf_free(m) };
            }
        }

        println!("== SoupBinTCP over DPDK rx (net_pcap replay, {frames_n} frames) ==");
        println!("frames parsed: {packets_seen} (login + {seq_reports} sequenced)");
        println!(
            "expected_seq after replay: {} (login_ok={login_ok})",
            client.expected_seq
        );
        let expect_end = 1 + total_reports;
        println!(
            "PASS: login_ok={login_ok}, seq={} == {expect_end}, reports={seq_reports}",
            client.expected_seq
        );
        assert!(login_ok, "login accepted with start_seq 1");
        assert_eq!(client.expected_seq, expect_end, "sequence continuity");
        assert_eq!(seq_reports, total_reports, "all reports parsed");

        unsafe { eal_cleanup() };
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("mold_soup_dpdk error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("mold_soup_dpdk is Linux-only");
        std::process::exit(1);
    }
}
