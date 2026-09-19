//! `mold_mcast_bench` — unicast vs multicast latency comparison (Linux only).
//!
//! Same ping-pong shape as mold_udp_bench's RTT section, but compares:
//!   U. unicast 127.0.0.1 (UdpSocket -> UdpSocket)
//!   M. multicast 239.255.0.1 with IP_ADD_MEMBERSHIP on lo (send -> join -> echo)
//!
//! Latency = RTT/2 (loopback, no Mold parse). Multicast on Docker's default
//! bridge/lo may or may not deliver; the result is reported honestly.
//!
//! Usage: mold_mcast_bench [ITERS]

#[cfg(target_os = "linux")]
mod real {
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const MCAST: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 9);
    const MCAST_PORT: u16 = 55009;

    fn now_ns() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    fn rtt_round(a: &UdpSocket, b: &UdpSocket, b_addr: SocketAddr, ts: &mut [u8; 16]) -> std::io::Result<u128> {
        let t = now_ns() as u64;
        ts[..8].copy_from_slice(&t.to_le_bytes());
        a.send_to(&ts[..], b_addr)?;
        let _ = b.recv_from(&mut ts[..])?;
        b.send_to(&ts[..], a.local_addr()?)?;
        let _ = a.recv_from(&mut ts[..])?;
        let t2 = now_ns() as u64;
        Ok((t2 - u64::from_le_bytes(ts[..8].try_into().unwrap())) as u128)
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let iters: usize = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(2_000);

        let mut ts = [0u8; 16];

        // ---- U. unicast loopback ----
        let a = UdpSocket::bind("127.0.0.1:0")?;
        let b = UdpSocket::bind("127.0.0.1:0")?;
        a.set_read_timeout(Some(Duration::from_secs(2)))?;
        b.set_read_timeout(Some(Duration::from_secs(2)))?;
        // warmup
        let _ = rtt_round(&a, &b, b.local_addr()?, &mut ts);

        let mut u_rtt: Vec<u128> = Vec::with_capacity(iters);
        for _ in 0..iters {
            u_rtt.push(rtt_round(&a, &b, b.local_addr()?, &mut ts)?);
        }
        u_rtt.sort_unstable();
        let (u50, u99, um) = (
            u_rtt[iters / 2],
            u_rtt[(iters as f64 * 0.99) as usize],
            u_rtt.iter().sum::<u128>() / iters as u128,
        );

        // ---- M. multicast (join on lo, send to 239.255.0.9) ----
        let mut m_ok = false;
        let mut m_rtt: Vec<u128> = Vec::with_capacity(iters);
        let send = UdpSocket::bind("127.0.0.1:0")?;
        send.set_multicast_loop_v4(true)?;
        send.set_multicast_ttl_v4(8)?;

        let rcvr = UdpSocket::bind(format!("0.0.0.0:{MCAST_PORT}"))?;
        rcvr.set_read_timeout(Some(Duration::from_millis(500)))?;
        match rcvr.join_multicast_v4(&MCAST, &Ipv4Addr::LOCALHOST) {
            Ok(()) => {
                // warmup
                let mcast_addr: SocketAddr = (MCAST, MCAST_PORT).into();
                let _ = rtt_round(&send, &rcvr, mcast_addr, &mut ts);
                for _ in 0..iters {
                    match rtt_round(&send, &rcvr, mcast_addr, &mut ts) {
                        Ok(v) => m_rtt.push(v),
                        Err(_) => break,
                    }
                }
                m_ok = m_rtt.len() >= iters / 2;
            }
            Err(e) => {
                println!("multicast join failed: {e}");
            }
        }

        println!("== unicast vs multicast RTT (loopback, {iters} iters) ==");
        println!(
            "unicast  : RTT p50 {:>6} ns  p99 {:>6} ns  mean {:>6} ns  | one-way p50 {:.0} ns p99 {:.0} ns",
            u50, u99, um,
            u50 as f64 / 2.0,
            u99 as f64 / 2.0
        );
        if m_ok {
            m_rtt.sort_unstable();
            let (m50, m99, mm) = (
                m_rtt[m_rtt.len() / 2],
                m_rtt[(m_rtt.len() as f64 * 0.99) as usize],
                m_rtt.iter().sum::<u128>() / m_rtt.len() as u128,
            );
            println!(
                "multicast: RTT p50 {:>6} ns  p99 {:>6} ns  mean {:>6} ns  | one-way p50 {:.0} ns p99 {:.0} ns",
                m50, m99, mm,
                m50 as f64 / 2.0,
                m99 as f64 / 2.0
            );
            println!(
                "multicast vs unicast one-way p50 delta: {:.0} ns",
                (m50 as f64 - u50 as f64) / 2.0
            );
        } else {
            println!("multicast: NOT DELIVERED in container (Docker bridge/lo limitation)");
            println!("          -> unicast/multicast delta needs a real NIC / host networking");
        }
        Ok(())
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = real::run() {
            eprintln!("mcast_bench error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("mold_mcast_bench is Linux-only");
        std::process::exit(1);
    }
}
