//! `mold_dpdk_tx` — publish MoldUDP64 messages through the real DPDK tx path
//! (net_pcap PMD capturing to a pcap file), then the rx bin reads the file
//! back to verify the full publish→wire→parse round trip.
//!
//! Usage: mold_dpdk_tx <output.pcap>

#[cfg(target_os = "linux")]
mod real {
    use match_dpdk_io::dpdk::frames::eth_ip_udp_header;
    use match_dpdk_io::dpdk::port::{setup_port, tx_frame};
    use match_dpdk_io::dpdk::{attach_pcap_vdev, eal_cleanup, eal_init, mbuf_pool};
    use match_moldudp64::types::{DownstreamHeader, MOLD_DOWNSTREAM_HEADER_LEN};

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let file = std::env::args().nth(1).unwrap_or_else(|| "/data/output.pcap".into());

        eal_init(&[0], &[])?;
        let pool = mbuf_pool("mold_tx_mb")?;
        let port = attach_pcap_vdev("net_pcap0", &format!("tx_pcap={file}"))?;
        eprintln!("attached net_pcap0 (tx {file}) as port {port}");
        unsafe { setup_port(port, pool)?; }

        // Two data packets (seq 1: "a","b"; seq 3: "c") and one heartbeat
        // advertising seq 4.
        let session = *b"MATCH_RUST";
        let mk_frame = |first_seq: u64, msgs: &[&[u8]]| -> Vec<u8> {
            let hdr = DownstreamHeader {
                session_id: session,
                seq: first_seq,
                msg_count: msgs.len() as u16,
            };
            let mut mold = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
            hdr.encode(&mut mold);
            let mut body = Vec::new();
            for m in msgs {
                body.extend_from_slice(&(m.len() as u16).to_be_bytes());
                body.extend_from_slice(m);
            }
            let mut frame = eth_ip_udp_header(
                [0x01, 0x00, 0x5e, 0x00, 0xff, 0x01],
                [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
                [239, 0, 255, 1],
                [10, 0, 0, 1],
                50000,
                50000,
                mold.len() + body.len(),
            );
            frame.extend_from_slice(&mold);
            frame.extend_from_slice(&body);
            frame
        };

        let frames = [
            mk_frame(1, &[b"a", b"b"]),
            mk_frame(3, &[b"c"]),
            mk_frame(4, &[]), // heartbeat
        ];

        let mut sent = 0u32;
        for f in &frames {
            unsafe { tx_frame(port, pool, f)? };
            sent += 1;
        }
        unsafe { eal_cleanup() };
        println!("DPDK_TX_VERIFY: sent {sent} frames -> {file}");
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("tx error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("match-dpdk-io bins are Linux-only");
        std::process::exit(1);
    }
}
