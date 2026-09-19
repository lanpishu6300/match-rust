//! `mold_dpdk_gen_pcap` — generate the verification pcap for the DPDK rx test.
//!
//! Builds a pcap file containing real MoldUDP64 frames (Eth/IPv4/UDP):
//!   packet 1: seq 1   (1 message "m1")
//!   packet 2: seq 2   (1 message "m2")
//!   packet 3: seq 3   (1 message "m3")
//!   packet 4: heartbeat advertising seq 4
//!   packet 5: seq 5   (2 messages "m5","m6")   ← gap at seq 4
//!
//! The rx bin asserts exactly this shape, exercising parse + gap detection
//! through the real DPDK receive path (net_pcap PMD).

#[cfg(target_os = "linux")]
mod real {
    use match_moldudp64::types::{
        DownstreamHeader, MOLD_DOWNSTREAM_HEADER_LEN,
    };
    use std::io::Write;

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::args().nth(1).unwrap_or_else(|| "/data/input.pcap".into());
        let mut out = std::fs::File::create(&path)?;

        // pcap global header (24 bytes: magic, ver_major, ver_minor, thiszone,
        // sigfigs, snaplen, linktype=1 Ethernet).
        let mut gh = [0u8; 24];
        gh[..4].copy_from_slice(&0xa1b2c3d4u32.to_le_bytes()); // magic
        gh[4..6].copy_from_slice(&2u16.to_le_bytes()); // version major
        gh[6..8].copy_from_slice(&4u16.to_le_bytes()); // version minor
        gh[16..20].copy_from_slice(&262_144u32.to_le_bytes()); // snaplen
        gh[20..24].copy_from_slice(&1u32.to_le_bytes()); // LINKTYPE_ETHERNET
        out.write_all(&gh)?;

        let dst_mac = [0x01, 0x00, 0x5e, 0x00, 0xff, 0x01]; // 239.0.255.1
        let src_mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let dst_ip = [239, 0, 255, 1];
        let src_ip = [10, 0, 0, 1];

        let session = *b"MATCH_RUST";
        let packet = |first_seq: u64, msgs: &[&[u8]]| -> Vec<u8> {
            let header = DownstreamHeader {
                session_id: session,
                seq: first_seq,
                msg_count: msgs.len() as u16,
            };
            let mut mold = [0u8; MOLD_DOWNSTREAM_HEADER_LEN];
            header.encode(&mut mold);
            let mut body = Vec::new();
            for m in msgs {
                body.extend_from_slice(&(m.len() as u16).to_be_bytes());
                body.extend_from_slice(m);
            }
            let udp_len = 8 + mold.len() + body.len();
            let mut frame = Vec::new();
            frame.extend_from_slice(&dst_mac);
            frame.extend_from_slice(&src_mac);
            frame.extend_from_slice(&[0x08, 0x00]);
            // IP header
            frame.push(0x45);
            frame.push(0);
            frame.extend_from_slice(&((20 + udp_len) as u16).to_be_bytes());
            frame.extend_from_slice(&[0, 0, 0, 0]);
            frame.push(64);
            frame.push(17);
            frame.extend_from_slice(&[0, 0]);
            frame.extend_from_slice(&src_ip);
            frame.extend_from_slice(&dst_ip);
            // UDP
            frame.extend_from_slice(&50000u16.to_be_bytes());
            frame.extend_from_slice(&50000u16.to_be_bytes());
            frame.extend_from_slice(&(udp_len as u16).to_be_bytes());
            frame.extend_from_slice(&[0, 0]);
            // Mold
            frame.extend_from_slice(&mold);
            frame.extend_from_slice(&body);
            frame
        };

        let frames = [
            packet(1, &[b"m1"]),
            packet(2, &[b"m2"]),
            packet(3, &[b"m3"]),
            packet(4, &[]), // heartbeat, advertises next seq 4
            packet(5, &[b"m5", b"m6"]),
        ];

        let now = 1_700_000_000u64;
        for (i, f) in frames.iter().enumerate() {
            let mut ph = [0u8; 16];
            ph[..8].copy_from_slice(&(now + i as u64).to_le_bytes()); // ts_sec
            ph[8..12].copy_from_slice(&0u32.to_le_bytes()); // ts_usec
            ph[12..16].copy_from_slice(&(f.len() as u32).to_le_bytes()); // incl_len
            out.write_all(&ph)?;
            out.write_all(f)?;
        }
        println!("gen_pcap: wrote {} frames -> {path}", frames.len());
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("gen_pcap error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("match-dpdk-io bins are Linux-only");
        std::process::exit(1);
    }
}
